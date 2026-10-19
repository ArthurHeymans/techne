//! A supervisor: deployments served by a worker process
//! ([`crate::worker`]), sandboxed, in a cgroup of its own where one can be
//! had (`techne_sandbox::cgroup`), for a front world of the supervisor's
//! (`host-fetch`, as [`crate::Host::install`] has it).
//!
//! The supervisor trusts nothing the worker says (`wire`): a reply must be
//! to a request it sent that worker, still waiting, and anything else (a
//! malformed frame, a reply to no request) kills the worker. It bounds
//! what it waits for too: a request the worker has not answered a little
//! past its wall time kills the worker. A worker that ends, killed or for
//! a native that held its thread past a kill (`worker::STUCK`) or past its
//! cgroup's memory, has its requests answered for, with 500, reported;
//! another is started, at most a few a minute, and given the last
//! deployment it acknowledged. Requests are not run again.

use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    ffi::OsString,
    os::{fd::AsRawFd, unix::process::ExitStatusExt},
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use techne_sandbox::cgroup::{self, Cgroups};
use techne_vm::{
    data::Data,
    tasks::Progress,
    vm::{Error, Vm},
};
use tokio::{
    io::AsyncWriteExt,
    net::UnixStream,
    sync::{Notify, mpsc, oneshot},
};

use crate::{
    Failure, Printed, REQUEST_BYTES,
    wire::{BUNDLE_BYTES, FromWorker, ToWorker, read_frame, write_frame},
    worker,
};

/// How long past a request's wall time the supervisor waits for the worker
/// to answer it (the worker answers 504 at its wall time).
const GRACE: Duration = Duration::from_secs(2);

/// Workers started a minute, at most: past them, requests are refused.
const RESTARTS: usize = 5;

/// How long a worker may take to be ready.
const START: Duration = Duration::from_secs(10);

/// Whether workers are sandboxed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sandboxing {
    /// Always; a worker that cannot be is an error.
    Required,
    /// Where the kernel allows it; elsewhere (a dev machine without user
    /// namespaces), not, said once.
    IfAvailable,
    Off,
}

/// How the supervisor starts its workers.
#[derive(Clone, Debug)]
pub struct Options {
    /// The worker's program, and its arguments before `--fd N`.
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// Its environment, beside the runtime's settings (`TECHNE_*`) the
    /// supervisor has.
    pub env: Vec<(OsString, OsString)>,
    pub sandboxing: Sandboxing,
    /// What a worker's cgroup bounds, where one can be had.
    pub cgroup: cgroup::Limits,
    /// A request's wall time, the deployments' own.
    pub wall: Duration,
}

/// What a deployment's worker is given: its name, entry, and files.
#[derive(Clone)]
struct Bundle {
    name: String,
    entry: String,
    files: Vec<(String, Vec<u8>)>,
}

struct Pending {
    name: String,
    reply: oneshot::Sender<Data>,
    deadline: Instant,
}

/// The worker running now.
struct Running {
    generation: u64,
    send: mpsc::UnboundedSender<ToWorker>,
    pid: u32,
    cgroup: Option<cgroup::Worker>,
}

struct State {
    worker: Option<Running>,
    generation: u64,
    next_rid: u64,
    pending: HashMap<u64, Pending>,
    next_deploy: u64,
    deploys: HashMap<u64, (Bundle, oneshot::Sender<Result<String, String>>)>,
    /// The last deployment a worker acknowledged, given to the next.
    last: Option<Bundle>,
    sandbox: bool,
    started: VecDeque<Instant>,
    on_failure: Box<dyn FnMut(&Failure)>,
    on_output: Box<dyn FnMut(&Printed)>,
}

/// Serves deployments with a worker process: see the module.
#[derive(Clone)]
pub struct Supervisor {
    options: Rc<Options>,
    cgroups: Rc<Option<Cgroups>>,
    state: Rc<RefCell<State>>,
    /// Wakes the front world's driver.
    wake: Arc<Notify>,
    /// Wakes the watcher of deadlines.
    changed: Rc<Notify>,
}

impl Supervisor {
    /// A supervisor with its first worker started and ready. Call it on a
    /// current-thread runtime within a `LocalSet`.
    pub async fn start(options: Options) -> std::io::Result<Supervisor> {
        let cgroups = match Cgroups::adopt() {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("techne cloud: workers' memory and CPU are not bounded: {e}");
                None
            }
        };
        let sandbox = options.sandboxing != Sandboxing::Off;
        let s = Supervisor {
            options: Rc::new(options),
            cgroups: Rc::new(cgroups),
            state: Rc::new(RefCell::new(State {
                worker: None,
                generation: 0,
                next_rid: 0,
                pending: HashMap::new(),
                next_deploy: 0,
                deploys: HashMap::new(),
                last: None,
                sandbox,
                started: VecDeque::new(),
                on_failure: Box::new(|f| eprintln!("techne cloud: {} #{}: {} {}", f.deployment, f.request, f.status, f.message)),
                on_output: Box::new(|p| {
                    for line in p.output.text.lines() {
                        eprintln!("techne cloud: {} #{}: {line}", p.deployment, p.request);
                    }
                    if p.output.truncated {
                        eprintln!("techne cloud: {} #{}: (output cut short)", p.deployment, p.request);
                    }
                }),
            })),
            wake: Arc::new(Notify::new()),
            changed: Rc::new(Notify::new()),
        };
        s.start_worker().await?;
        tokio::task::spawn_local(s.clone().watch());
        Ok(s)
    }

    /// Call `f` with each failure answered for, the worker's or its own,
    /// in place of writing it to standard error (as
    /// [`crate::Host::on_failure`]; the request's number is the worker's,
    /// 0 for one it never had).
    pub fn on_failure(&self, f: impl FnMut(&Failure) + 'static) {
        self.state.borrow_mut().on_failure = Box::new(f);
    }

    /// Call `f` with what each request wrote, in place of writing it to
    /// standard error (as [`crate::Host::on_output`]).
    pub fn on_output(&self, f: impl FnMut(&Printed) + 'static) {
        self.state.borrow_mut().on_output = Box::new(f);
    }

    /// Start a worker, and wait until it is ready: in its cgroup, then
    /// let through its gate, sandboxed, or not where the kernel does not
    /// allow it and sandboxing is not required.
    async fn start_worker(&self) -> std::io::Result<()> {
        loop {
            let sandbox = self.state.borrow().sandbox;
            match self.spawn(sandbox).await {
                Err(e) if e.kind() == std::io::ErrorKind::Unsupported && self.options.sandboxing == Sandboxing::IfAvailable => {
                    eprintln!("techne cloud: workers are not sandboxed here: {e}");
                    self.state.borrow_mut().sandbox = false;
                }
                r => return r,
            }
        }
    }

    async fn spawn(&self, sandbox: bool) -> std::io::Result<()> {
        use std::os::unix::process::CommandExt as _;
        let generation = {
            let mut s = self.state.borrow_mut();
            s.generation += 1;
            s.started.push_back(Instant::now());
            s.generation
        };
        let (ours, theirs) = std::os::unix::net::UnixStream::pair()?;
        let fd = theirs.as_raw_fd();
        let mut command = std::process::Command::new(&self.options.program);
        command.args(&self.options.args).arg("--fd").arg(fd.to_string());
        if !sandbox {
            command.arg("--unsandboxed");
        }
        // Its environment the runtime's settings, no more.
        command.env_clear().envs(std::env::vars_os().filter(|(k, _)| k.to_string_lossy().starts_with("TECHNE_") || k == "RUST_BACKTRACE"));
        command.envs(self.options.env.iter().map(|(k, v)| (k, v)));
        command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null());
        // SAFETY: one system call between fork and exec: the worker keeps
        // its end of the socket.
        unsafe {
            command.pre_exec(move || {
                libc::fcntl(fd, libc::F_SETFD, 0);
                Ok(())
            })
        };
        let mut child = tokio::process::Command::from(command).kill_on_drop(true).spawn()?;
        drop(theirs);
        let pid = child.id().unwrap_or(0);
        let cgroup = match &*self.cgroups {
            Some(c) => {
                let w = c.worker(&format!("w{generation}"), &self.options.cgroup)?;
                w.add(pid)?;
                Some(w)
            }
            None => None,
        };
        ours.set_nonblocking(true)?;
        let mut stream = UnixStream::from_std(ours)?;
        stream.write_all(&[1]).await?;
        let ready = tokio::time::timeout(START, read_frame(&mut stream)).await;
        match ready.map(|r| r.map(|d| d.map(FromWorker::from_data))) {
            Ok(Ok(Some(Ok(FromWorker::Ready)))) => {}
            _ => {
                let status = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
                return Err(match status {
                    Ok(Ok(s)) if s.code() == Some(worker::UNSANDBOXED.into()) => {
                        std::io::Error::new(std::io::ErrorKind::Unsupported, "the kernel does not let it sandbox itself")
                    }
                    _ => std::io::Error::other("the worker did not start"),
                });
            }
        }
        let (mut read, mut write) = stream.into_split();
        let (send, mut outgoing) = mpsc::unbounded_channel::<ToWorker>();
        let me = self.clone();
        tokio::task::spawn_local(async move {
            while let Some(m) = outgoing.recv().await {
                if let Err(e) = write_frame(&mut write, &m.to_data()).await {
                    me.end_worker(generation, &format!("the worker is not reachable: {e}"));
                    break;
                }
            }
        });
        let me = self.clone();
        tokio::task::spawn_local(async move {
            loop {
                let message = match read_frame(&mut read).await {
                    Ok(Some(d)) => FromWorker::from_data(d),
                    // Ending, likely: why, its exit says, if it comes.
                    Ok(None) => {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        me.end_worker(generation, "the worker closed its socket");
                        break;
                    }
                    Err(e) => Err(e),
                };
                let fault = match message {
                    Ok(m) => me.heard(generation, m),
                    Err(e) => Some(e.to_string()),
                };
                if let Some(why) = fault {
                    me.end_worker(generation, &format!("the worker misbehaved: {why}"));
                    break;
                }
            }
        });
        let me = self.clone();
        tokio::task::spawn_local(async move {
            let status = child.wait().await;
            let why = match status {
                Ok(s) if s.code() == Some(worker::STUCK.into()) => "a native ran on past a kill".to_string(),
                Ok(s) if s.signal() == Some(libc::SIGKILL) => "the worker was killed (past its memory?)".to_string(),
                Ok(s) => format!("the worker ended ({s})"),
                Err(e) => format!("the worker is gone: {e}"),
            };
            me.end_worker(generation, &why);
        });
        self.state.borrow_mut().worker = Some(Running { generation, send, pid, cgroup });
        Ok(())
    }

    /// What worker `generation` said: why it is at fault, if it is.
    fn heard(&self, generation: u64, message: FromWorker) -> Option<String> {
        let mut s = self.state.borrow_mut();
        // Of a worker gone: nothing it says counts.
        if s.worker.as_ref().is_none_or(|w| w.generation != generation) {
            return None;
        }
        match message {
            FromWorker::Response { rid, response } => match s.pending.remove(&rid) {
                Some(p) => {
                    let _ = p.reply.send(response);
                    None
                }
                None => Some(format!("a response to no request ({rid})")),
            },
            FromWorker::Deployed { id, error, output } => match s.deploys.remove(&id) {
                Some((bundle, reply)) => {
                    if error.is_none() {
                        s.last = Some(bundle);
                    }
                    let _ = reply.send(error.map_or(Ok(output), Err));
                    None
                }
                None => Some(format!("a deployment it was not given ({id})")),
            },
            FromWorker::Failure { deployment, request, status, message } => {
                (s.on_failure)(&Failure { deployment, request, status, message });
                None
            }
            FromWorker::Output { deployment, request, text, truncated } => {
                let mut output = techne_vm::vm::Output::default();
                (output.text, output.truncated) = (text, truncated);
                (s.on_output)(&Printed { deployment, request, output });
                None
            }
            FromWorker::Ready => Some("ready again".into()),
        }
    }

    /// Worker `generation` is done for, for `why`: kill it, answer for its
    /// requests, and start another with the last deployment.
    fn end_worker(&self, generation: u64, why: &str) {
        let (requests, deploys) = {
            let mut s = self.state.borrow_mut();
            let Some(w) = s.worker.take_if(|w| w.generation == generation) else { return };
            match &w.cgroup {
                Some(c) => {
                    let _ = c.kill();
                }
                // SAFETY: signalling the worker's waiter, whose death ends
                // the worker.
                None => unsafe {
                    libc::kill(w.pid as i32, libc::SIGKILL);
                },
            }
            (std::mem::take(&mut s.pending), std::mem::take(&mut s.deploys))
        };
        for (_, p) in requests {
            self.answer(p.reply, &p.name, 500, why);
        }
        for (_, (_, reply)) in deploys {
            let _ = reply.send(Err(why.into()));
        }
        let me = self.clone();
        tokio::task::spawn_local(async move { me.restart().await });
    }

    /// Start another worker, if not too many were this minute, and give
    /// it the last deployment acknowledged.
    async fn restart(&self) {
        let wait = {
            let mut s = self.state.borrow_mut();
            let now = Instant::now();
            while s.started.front().is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60)) {
                s.started.pop_front();
            }
            match s.started.len() >= RESTARTS {
                true => s.started.front().map(|t| Duration::from_secs(60).saturating_sub(now.duration_since(*t))),
                false => None,
            }
        };
        if let Some(wait) = wait {
            eprintln!("techne cloud: workers keep ending; the next starts in {}s", wait.as_secs());
            tokio::time::sleep(wait).await;
        }
        if let Err(e) = self.start_worker().await {
            eprintln!("techne cloud: no worker: {e}");
            return;
        }
        let last = self.state.borrow().last.clone();
        if let Some(b) = last {
            match self.send_deploy(b).await {
                Ok(_) => eprintln!("techne cloud: a new worker serves the deployment"),
                Err(e) => eprintln!("techne cloud: the new worker did not load the deployment: {e}"),
            }
        }
    }

    /// Answer `reply` for deployment `name` with `status`, and report it.
    fn answer(&self, reply: oneshot::Sender<Data>, name: &str, status: u16, why: &str) {
        let failure = Failure { deployment: name.into(), request: 0, status, message: why.into() };
        (self.state.borrow_mut().on_failure)(&failure);
        let reason = match status {
            503 => "Service Unavailable",
            504 => "Gateway Timeout",
            _ => "Internal Server Error",
        };
        let _ = reply.send(Data::Dict(vec![
            (Data::Symbol("status".into()), Data::Int(status.into())),
            (Data::Symbol("body".into()), Data::String(reason.into())),
        ]));
    }

    /// Kill the worker over a request past its wall time and grace, and
    /// cancel requests the front abandoned; for ever.
    async fn watch(self) {
        loop {
            let (next, late, abandoned, generation) = {
                let mut s = self.state.borrow_mut();
                let now = Instant::now();
                let abandoned: Vec<u64> = s.pending.iter().filter(|(_, p)| p.reply.is_closed()).map(|(r, _)| *r).collect();
                for rid in &abandoned {
                    s.pending.remove(rid);
                }
                let late = s.pending.values().any(|p| p.deadline <= now);
                let next = s.pending.values().map(|p| p.deadline).min();
                (next, late, abandoned, s.worker.as_ref().map(|w| (w.generation, w.send.clone())))
            };
            if let Some((_, send)) = &generation {
                for rid in abandoned {
                    let _ = send.send(ToWorker::Cancel { rid });
                }
            }
            if late && let Some((g, _)) = generation {
                self.end_worker(g, "the worker did not answer a request past its wall time");
            }
            // Abandoned requests are looked for now and then.
            let until =
                next.map_or(Duration::from_millis(250), |t| t.saturating_duration_since(Instant::now())).min(Duration::from_millis(250));
            let _ = tokio::time::timeout(until, self.changed.notified()).await;
        }
    }

    /// Ask the deployment published as `name` to answer `request`, as
    /// [`crate::Host::fetch`] does.
    pub fn fetch(&self, name: &str, request: Data) -> oneshot::Receiver<Data> {
        let (tx, rx) = oneshot::channel();
        let mut s = self.state.borrow_mut();
        let Some(send) = s.worker.as_ref().map(|w| w.send.clone()) else {
            drop(s);
            self.answer(tx, name, 503, "no worker");
            return rx;
        };
        s.next_rid += 1;
        let rid = s.next_rid;
        let deadline = Instant::now() + self.options.wall + GRACE;
        s.pending.insert(rid, Pending { name: name.into(), reply: tx, deadline });
        drop(s);
        let _ = send.send(ToWorker::Request { rid, name: name.into(), request });
        self.changed.notify_one();
        rx
    }

    /// Have the worker serve the deployment whose entry module is `entry`
    /// as `name`, its files those under the entry's directory (but in
    /// hidden directories, `target` and `node_modules`): what loading it
    /// wrote, or why it was not.
    pub async fn deploy(&self, name: &str, entry: &Path) -> Result<String, String> {
        let dir = entry.parent().ok_or("an entry in no directory")?;
        let files = bundle(dir)?;
        let entry = entry.strip_prefix(dir).map_err(|e| e.to_string())?.to_string_lossy().into_owned();
        self.send_deploy(Bundle { name: name.into(), entry, files }).await
    }

    async fn send_deploy(&self, bundle: Bundle) -> Result<String, String> {
        let (tx, rx) = oneshot::channel();
        {
            let mut s = self.state.borrow_mut();
            let Some(send) = s.worker.as_ref().map(|w| w.send.clone()) else { return Err("no worker".into()) };
            s.next_deploy += 1;
            let id = s.next_deploy;
            let message = ToWorker::Deploy {
                id,
                name: bundle.name.clone(),
                entry: bundle.entry.clone(),
                files: bundle.files.clone(),
                wall_ms: self.options.wall.as_millis() as u64,
            };
            s.deploys.insert(id, (bundle, tx));
            let _ = send.send(message);
        }
        match tokio::time::timeout(DEPLOY, rx).await {
            Ok(r) => r.map_err(|_| "the worker is gone".to_string())?,
            Err(_) => Err("the worker did not load it in time".into()),
        }
    }

    /// Define `(host-fetch deployment request)` in the front world, as
    /// [`crate::Host::install`] does.
    pub fn install(&self, front: &mut Vm) {
        let wake = self.wake.clone();
        front.set_wake_notifier(move || wake.notify_one());
        let me = self.clone();
        front.register_async("host-fetch", 2, move |vm, args| {
            let job = (|| {
                let deployment: String = args.get(vm, 0)?;
                let request = args.value(vm, 1)?;
                Ok::<_, Error>((deployment, vm.to_data(&request, REQUEST_BYTES)?))
            })();
            let asked = job.map(|(deployment, request)| me.fetch(&deployment, request));
            async move { asked?.await.map_err(|_| Error::new("host-fetch: the supervisor stopped")) }
        });
    }

    /// Drive `front` (installed) until its tasks have all finished.
    pub async fn run(&self, front: &mut Vm) {
        loop {
            match front.run_tasks_for(Duration::from_millis(10)) {
                Progress::Finished => return,
                Progress::OutOfTime => tokio::task::yield_now().await,
                Progress::Blocked => match front.next_timer() {
                    Some(t) => {
                        let _ = tokio::time::timeout_at(t.into(), self.wake.notified()).await;
                    }
                    None => self.wake.notified().await,
                },
            }
        }
    }
}

/// How long a worker may take to load a deployment: its loading's bound
/// and more.
const DEPLOY: Duration = Duration::from_secs(60);

/// The files under `dir`, by their paths in it: not in hidden
/// directories, `target` or `node_modules`, nor through symbolic links.
fn bundle(dir: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut files = Vec::new();
    let mut total = 0;
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(d) = dirs.pop() {
        for entry in std::fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))?.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            match entry.file_type() {
                Ok(t) if t.is_dir() => {
                    if !(name.starts_with('.') || name == "target" || name == "node_modules") {
                        dirs.push(path);
                    }
                }
                Ok(t) if t.is_file() => {
                    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                    total += bytes.len();
                    if total > BUNDLE_BYTES {
                        return Err(format!("{}: more than {BUNDLE_BYTES} bytes of files", dir.display()));
                    }
                    let relative = path.strip_prefix(dir).map_err(|e| e.to_string())?.to_string_lossy().into_owned();
                    files.push((relative, bytes));
                }
                _ => {}
            }
        }
    }
    files.sort();
    Ok(files)
}
