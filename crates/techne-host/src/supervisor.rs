//! A supervisor: deployments served by a worker process
//! ([`crate::worker`]), with sandbox entry and cgroup resource confinement
//! required independently (`Options::sandboxing`, `Options::resources`),
//! for a front world of the supervisor's (`host-fetch`, as
//! [`crate::Host::install`] has it).
//!
//! The supervisor trusts nothing the worker says (`wire`): a reply must be
//! to a request it sent, and anything else (a malformed frame, a reply to
//! a request never sent, a body past its credits) kills the worker; a
//! reply to one it no longer waits for (abandoned, cut short) is dropped.
//! It bounds what it waits for too: a request the worker has not answered
//! a little past its wall time kills the worker; a response's body still
//! streaming then, behind a client slow to read it, is cut short. A
//! request cancelled so is waited for until the worker says it is done
//! with it, which it does at once, or a worker that cannot is killed.
//! What is sent to a worker waits bounded (`QUEUED`): one that does not
//! read it is killed too. A worker that ends, killed or for
//! a native that held its thread past a kill (`worker::STUCK`) or past its
//! cgroup's memory, has its requests answered for, with 500, reported;
//! another is started, at most a few a minute, and given the last
//! deployment it acknowledged. Requests are not run again.
//!
//! What a worker's requests fetch is fetched here ([`crate::egress`]),
//! within the policy given (`Options::egress`): for a request the
//! supervisor sent it and still serves, a bounded number of times each
//! (`FETCHES`), a bounded number at once (`FETCHING`); the worker's own
//! counts are not trusted.
//!
//! For development (`Options::dev`), the worker serves each deployment
//! from its dev world, with a REPL server in it ([`crate::worker::Dev`]),
//! whose clients connect to the supervisor ([`Supervisor::serve_repl`]),
//! which has the network: each connection is relayed as bytes both ways.
//! A worker replaced takes its REPL connections with it: they are closed,
//! and clients connect again to the next.

use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    ffi::OsString,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::process::ExitStatusExt,
    },
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use techne_sandbox::cgroup::{self, Cgroups};
use techne_vm::{data::Data, tasks::Progress, vm::Vm};
use tokio::{
    io::AsyncWriteExt,
    net::UnixStream,
    sync::{Notify, Semaphore, mpsc, oneshot},
};

use crate::{
    Failure, Printed, Response,
    body::{self, Piece, Reader},
    wire::{BUNDLE_BYTES, FromWorker, MAX_FRAME, REPL_CHUNK, ToWorker, WINDOW, encode, read_frame, write_encoded},
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

/// Whether workers must have their configured cgroup resource bounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceConfinement {
    /// Refuse to start without cgroups; never run a worker unbounded.
    Required,
    /// For development: run workers unbounded, with a diagnostic, when
    /// cgroups cannot be adopted or a worker's cgroup cannot have every
    /// configured bound.
    IfAvailable,
}

/// The confinement of the worker serving now, not just what was requested.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Confinement {
    /// The worker entered its OS sandbox before becoming ready.
    pub sandboxed: bool,
    /// The worker joined a cgroup with every configured bound applied.
    pub cgroup_bounded: bool,
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
    /// Required independently of sandbox entry.
    pub resources: ResourceConfinement,
    /// What a worker's cgroup bounds, when resource confinement is available.
    pub cgroup: cgroup::Limits,
    /// A request's wall time, the deployments' own.
    pub wall: Duration,
    /// Whether deployments are served for development (their dev worlds,
    /// with a REPL).
    pub dev: bool,
    /// What requests may fetch.
    pub egress: crate::egress::Policy,
}

/// Bytes waiting to be written to a worker, at most: a deployment and
/// more. A worker that does not read them is ended.
const QUEUED: usize = 48 << 20;

/// What goes to a worker: each message encoded as it is sent, the bytes
/// waiting bounded (`QUEUED`).
#[derive(Clone)]
struct Link {
    tx: mpsc::UnboundedSender<Vec<u8>>,
    queued: Rc<std::cell::Cell<usize>>,
    /// Whether a message was refused for the bytes waiting.
    over: Rc<std::cell::Cell<bool>>,
}

impl Link {
    fn send(&self, m: ToWorker) -> Result<(), ()> {
        let bytes = encode(&m.to_data());
        if bytes.len() > MAX_FRAME || self.queued.get() + bytes.len() > QUEUED {
            self.over.set(true);
            return Err(());
        }
        self.queued.set(self.queued.get() + bytes.len());
        self.tx.send(bytes).map_err(|_| ())
    }
}

/// The most bytes of a fetch a worker asks for: a request body's and its
/// URL and headers.
const FETCH_BYTES: usize = (1 << 20) + (64 << 10);

/// Fetches of a request, and of a worker's requests at once.
const FETCHES: usize = 50;
const FETCHING: usize = 64;

/// Bytes a relayed REPL connection may have waiting to be written to its
/// client.
const REPL_QUEUE: usize = 64;

/// A REPL connection relayed: what goes to its client, and the task
/// reading from it, which ends when the connection is dropped.
struct Repl {
    queue: mpsc::Sender<Vec<u8>>,
    reader: tokio::task::AbortHandle,
}

impl Drop for Repl {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

/// What a deployment's worker is given: its name, entry, and files.
#[derive(Clone)]
struct Bundle {
    name: String,
    entry: String,
    files: Vec<(String, Vec<u8>)>,
}

/// A request the worker serves: who waits for its response, until it is
/// given; the queue its response's body arrives on, while that streams;
/// the credits for its own body's chunks, while that streams.
/// A request cancelled (abandoned, its body cut short) waits for the
/// worker to say it is done with it, as it does at once, within its
/// deadline.
struct Pending {
    name: String,
    reply: Option<oneshot::Sender<Response>>,
    deadline: Instant,
    body: Option<mpsc::Sender<Piece>>,
    credits: Option<Rc<Semaphore>>,
    /// What sends its body.
    pump: Option<tokio::task::AbortHandle>,
    cancelled: bool,
    /// Its fetches so far.
    fetches: usize,
}

impl Pending {
    /// Cancel it: nothing more of it is sent or received.
    fn cancel(&mut self) {
        self.cancelled = true;
        self.reply = None;
        self.body = None;
        if let Some(p) = self.pump.take() {
            p.abort();
        }
    }
}

impl Drop for Pending {
    /// No more of the request's body is sent.
    fn drop(&mut self) {
        if let Some(p) = &self.pump {
            p.abort();
        }
    }
}

/// The worker running now.
struct Running {
    generation: u64,
    send: Link,
    /// The worker's pidfd: a kill through it cannot reach another process
    /// given the worker's pid once its waiter reaped it.
    pidfd: OwnedFd,
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
    /// REPL connections relayed to the worker now, by number: what goes
    /// to each client.
    repls: HashMap<u64, Repl>,
    next_cid: u64,
    /// Fetches being made.
    fetching: usize,
    on_failure: Box<dyn FnMut(&Failure)>,
    on_output: Box<dyn FnMut(&Printed)>,
}

/// Serves deployments with a worker process: see the module.
#[derive(Clone)]
pub struct Supervisor {
    options: Rc<Options>,
    egress: Rc<crate::egress::Egress>,
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
            Err(e) if options.resources == ResourceConfinement::Required => {
                return Err(std::io::Error::new(e.kind(), format!("workers require cgroup resource confinement: {e}")));
            }
            Err(e) => {
                eprintln!("techne cloud: workers' resources are not bounded: {e}");
                None
            }
        };
        let sandbox = options.sandboxing != Sandboxing::Off;
        let egress = Rc::new(crate::egress::Egress::new(options.egress.clone()).map_err(std::io::Error::other)?);
        let s = Supervisor {
            egress,
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
                repls: HashMap::new(),
                next_cid: 0,
                fetching: 0,
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

    /// The effective confinement of the current worker; none while no
    /// worker is ready (including during replacement).
    pub fn confinement(&self) -> Option<Confinement> {
        let s = self.state.borrow();
        s.worker.as_ref().map(|w| Confinement { sandboxed: s.sandbox, cgroup_bounded: w.cgroup.is_some() })
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
                // Once: the sandbox is all that falls back.
                Err(e) if e.kind() == std::io::ErrorKind::Unsupported && sandbox && self.options.sandboxing == Sandboxing::IfAvailable => {
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
        // SAFETY: pidfd_open takes a pid and flags; the child is not
        // reaped before its waiter, below, is.
        let pidfd = match unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } {
            fd if fd >= 0 => unsafe { OwnedFd::from_raw_fd(fd as i32) },
            // Not `Unsupported` where pidfds are not (before Linux 5.3):
            // that is the sandbox's refusal, which falls back.
            _ => return Err(std::io::Error::other(format!("pidfd_open: {}", std::io::Error::last_os_error()))),
        };
        let cgroup = match &*self.cgroups {
            Some(c) => match c.worker(&format!("w{generation}"), &self.options.cgroup).and_then(|w| w.add(pid).map(|()| w)) {
                Ok(w) => Some(w),
                // As when there are no cgroups: development goes on.
                Err(e) if self.options.resources == ResourceConfinement::IfAvailable => {
                    eprintln!("techne cloud: worker {generation}'s resources are not bounded: {e}");
                    None
                }
                Err(e) => return Err(e),
            },
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
        let (tx, mut outgoing) = mpsc::unbounded_channel::<Vec<u8>>();
        let send = Link { tx, queued: Default::default(), over: Default::default() };
        let (me, queued) = (self.clone(), send.queued.clone());
        tokio::task::spawn_local(async move {
            while let Some(bytes) = outgoing.recv().await {
                if let Err(e) = write_encoded(&mut write, &bytes).await {
                    me.end_worker(generation, &format!("the worker is not reachable: {e}"));
                    break;
                }
                queued.set(queued.get() - bytes.len());
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
        self.state.borrow_mut().worker = Some(Running { generation, send, pidfd, cgroup });
        Ok(())
    }

    /// What worker `generation` said: why it is at fault, if it is.
    fn heard(&self, generation: u64, message: FromWorker) -> Option<String> {
        let mut s = self.state.borrow_mut();
        // Of a worker gone: nothing it says counts.
        if s.worker.as_ref().is_none_or(|w| w.generation != generation) {
            return None;
        }
        // Of a request sent, served no longer.
        let past = |rid: u64, s: &State| (rid <= s.next_rid).then_some(None);
        match message {
            FromWorker::Response { rid, response, streams } => {
                let Some(p) = s.pending.get_mut(&rid) else {
                    return past(rid, &s).unwrap_or_else(|| Some(format!("a response to no request ({rid})")));
                };
                if p.cancelled {
                    return None;
                }
                let Some(reply) = p.reply.take() else { return Some(format!("a second response ({rid})")) };
                let body = streams.then(|| {
                    let (writer, reader) = body::channel();
                    let (queue, pieces) = mpsc::channel(WINDOW + 1);
                    p.body = Some(queue);
                    let me = self.clone();
                    tokio::task::spawn_local(async move {
                        let send = me.state.borrow().worker.as_ref().map(|w| w.send.clone());
                        let credit = |send: &Option<Link>| {
                            send.as_ref().map(|s| s.send(ToWorker::Credit { rid }));
                        };
                        // Its reader gone: the request is abandoned.
                        if !body::relay(pieces, writer, || credit(&send)).await {
                            me.cancel(rid);
                        }
                    });
                    reader
                });
                if !streams {
                    s.pending.remove(&rid);
                }
                let _ = reply.send(Response { head: response, body });
                None
            }
            FromWorker::Body { rid, bytes } => match s.pending.get(&rid) {
                Some(p) if p.cancelled => None,
                Some(p) => match p.body.as_ref().map(|q| q.try_send(Piece::Bytes(bytes))) {
                    Some(Err(mpsc::error::TrySendError::Full(_))) => Some(format!("a body past its credits ({rid})")),
                    Some(_) => None,
                    None => Some(format!("a body to no response streaming ({rid})")),
                },
                None => past(rid, &s).unwrap_or_else(|| Some(format!("a body to no request ({rid})"))),
            },
            // The end of its response's body, or of the request cancelled.
            FromWorker::BodyEnd { rid, error } => match s.pending.remove(&rid) {
                Some(p) if p.cancelled => None,
                Some(p) => match p.body.as_ref().map(|q| q.try_send(Piece::End(error))) {
                    Some(Err(mpsc::error::TrySendError::Full(_))) => Some(format!("a body past its credits ({rid})")),
                    Some(_) => None,
                    None => Some(format!("a body to no response streaming ({rid})")),
                },
                None => past(rid, &s).unwrap_or_else(|| Some(format!("a body to no request ({rid})"))),
            },
            // No more than it was sent chunks for.
            FromWorker::Credit { rid } => match s.pending.get(&rid).and_then(|p| p.credits.as_ref()) {
                Some(c) if c.available_permits() >= WINDOW => Some(format!("credits past its body's chunks ({rid})")),
                Some(c) => {
                    c.add_permits(1);
                    None
                }
                None => None,
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
            FromWorker::Fetch { rid, fid, request } => {
                let (fetching, sent) = (s.fetching, s.next_rid);
                let refused = match s.pending.get_mut(&rid) {
                    Some(p) if !p.cancelled && p.fetches < FETCHES && fetching < FETCHING => {
                        p.fetches += 1;
                        None
                    }
                    Some(p) if p.fetches >= FETCHES => Some(format!("a request fetches {FETCHES} times at most")),
                    Some(_) if fetching >= FETCHING => Some("too many fetches at once".to_string()),
                    Some(_) => Some("the request was cancelled".to_string()),
                    None if rid <= sent => Some("the request is over".to_string()),
                    None => return Some(format!("a fetch for no request ({rid})")),
                };
                let send = s.worker.as_ref().map(|w| w.send.clone());
                let refused = refused.or_else(|| (encode(&request).len() > FETCH_BYTES).then(|| "a fetch too large".to_string()));
                if let Some(why) = refused {
                    send.map(|w| w.send(ToWorker::Fetched { fid, response: Err(why) }));
                    return None;
                }
                s.fetching += 1;
                let me = self.clone();
                tokio::task::spawn_local(async move {
                    // Its place given back however the fetch ends.
                    struct Place(Supervisor);
                    impl Drop for Place {
                        fn drop(&mut self) {
                            self.0.state.borrow_mut().fetching -= 1;
                        }
                    }
                    let place = Place(me.clone());
                    let response = me.egress.fetch(&request).await;
                    drop(place);
                    // To the worker that asked, if it is still the one.
                    let current = me.state.borrow().worker.as_ref().filter(|w| w.generation == generation).map(|w| w.send.clone());
                    current.map(|w| w.send(ToWorker::Fetched { fid, response }));
                });
                None
            }
            FromWorker::ReplData { cid, bytes } => {
                // One gone meanwhile, or that cannot keep up, is closed.
                if s.repls.get(&cid).is_some_and(|r| r.queue.try_send(bytes).is_err()) {
                    s.repls.remove(&cid);
                    if let Some(w) = &s.worker {
                        let _ = w.send.send(ToWorker::ReplClose { cid });
                    }
                }
                None
            }
            FromWorker::ReplClose { cid } => {
                s.repls.remove(&cid);
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
                // the worker, through its pidfd: none once reaped.
                None => unsafe {
                    libc::syscall(libc::SYS_pidfd_send_signal, w.pidfd.as_raw_fd(), libc::SIGKILL, std::ptr::null::<libc::siginfo_t>(), 0);
                },
            }
            // Its REPL connections end with it.
            s.repls.clear();
            (std::mem::take(&mut s.pending), std::mem::take(&mut s.deploys))
        };
        // Their responses' bodies streaming are cut short.
        for (_, mut p) in requests {
            match p.reply.take() {
                Some(reply) => self.answer(reply, &p.name, 500, why),
                None => self.report(&p.name, 500, why),
            }
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
    fn answer(&self, reply: oneshot::Sender<Response>, name: &str, status: u16, why: &str) {
        self.report(name, status, why);
        let _ = reply.send(Response { head: crate::failed(status), body: None });
    }

    fn report(&self, name: &str, status: u16, why: &str) {
        let failure = Failure { deployment: name.into(), request: 0, status, message: why.into() };
        (self.state.borrow_mut().on_failure)(&failure);
    }

    /// Kill the worker over a request it has not answered past its wall
    /// time and grace, or for not reading what it is sent; cut short a
    /// response's body streaming then, and cancel requests the front
    /// abandoned; for ever.
    async fn watch(self) {
        loop {
            let now = Instant::now();
            let (abandoned, cut): (Vec<u64>, Vec<u64>) = {
                let s = self.state.borrow();
                let abandoned = s.pending.iter().filter(|(_, p)| p.reply.as_ref().is_some_and(|r| r.is_closed()));
                let cut = s.pending.iter().filter(|(_, p)| !p.cancelled && p.reply.is_none() && p.deadline <= now);
                (abandoned.map(|(r, _)| *r).collect(), cut.map(|(r, _)| *r).collect())
            };
            for rid in abandoned {
                self.cancel(rid);
            }
            // Its body cut short; the worker given a grace to be done.
            for rid in cut {
                let name = self.state.borrow_mut().pending.get_mut(&rid).map(|p| {
                    p.deadline = now + GRACE;
                    p.name.clone()
                });
                self.report(&name.unwrap_or_default(), 504, "the response's body was cut short past its wall time");
                self.cancel(rid);
            }
            let (next, late, worker) = {
                let s = self.state.borrow();
                let late = s.pending.values().any(|p| p.deadline <= now);
                let next = s.pending.values().map(|p| p.deadline).min();
                (next, late, s.worker.as_ref().map(|w| (w.generation, w.send.over.get())))
            };
            match worker {
                Some((g, _)) if late => self.end_worker(g, "the worker did not answer a request past its wall time"),
                Some((g, true)) => self.end_worker(g, "the worker does not read what it is sent"),
                _ => {}
            }
            // Abandoned requests are looked for now and then.
            let until =
                next.map_or(Duration::from_millis(250), |t| t.saturating_duration_since(Instant::now())).min(Duration::from_millis(250));
            let _ = tokio::time::timeout(until, self.changed.notified()).await;
        }
    }

    /// Cancel request `rid`, if it is not yet: the worker is told.
    fn cancel(&self, rid: u64) {
        let mut s = self.state.borrow_mut();
        let Some(p) = s.pending.get_mut(&rid).filter(|p| !p.cancelled) else { return };
        p.cancel();
        if let Some(w) = &s.worker {
            let _ = w.send.send(ToWorker::Cancel { rid });
        }
    }

    /// Ask the deployment published as `name` to answer `request`, as
    /// [`crate::Host::fetch`] does.
    pub fn fetch(&self, name: &str, request: Data, body: Option<Reader>) -> oneshot::Receiver<Response> {
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
        let credits = body.as_ref().map(|_| Rc::new(Semaphore::new(WINDOW)));
        let _ = send.send(ToWorker::Request { rid, name: name.into(), request, streams: body.is_some() });
        let pump = body.zip(credits.clone()).map(|(reader, credits)| {
            let send = send.clone();
            let pumping = tokio::task::spawn_local(body::pump(reader, credits, move |piece| {
                send.send(match piece {
                    Piece::Bytes(bytes) => ToWorker::Body { rid, bytes },
                    Piece::End(error) => ToWorker::BodyEnd { rid, error },
                })
                .is_ok()
            }));
            pumping.abort_handle()
        });
        let pending = Pending { name: name.into(), reply: Some(tx), deadline, body: None, credits, pump, cancelled: false, fetches: 0 };
        s.pending.insert(rid, pending);
        drop(s);
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
                dev: self.options.dev,
            };
            s.deploys.insert(id, (bundle, tx));
            let _ = send.send(message);
        }
        match tokio::time::timeout(DEPLOY, rx).await {
            Ok(r) => r.map_err(|_| "the worker is gone".to_string())?,
            Err(_) => Err("the worker did not load it in time".into()),
        }
    }

    /// Accept REPL clients on `addr` (loopback, for development), each
    /// relayed to the worker's dev world: the address bound.
    pub async fn serve_repl(&self, addr: &str) -> std::io::Result<std::net::SocketAddr> {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let bound = listener.local_addr()?;
        let me = self.clone();
        tokio::task::spawn_local(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let _ = stream.set_nodelay(true);
                me.relay(stream);
            }
        });
        Ok(bound)
    }

    /// Relay `stream`, a REPL client, to the worker as a connection of its
    /// own, until either end closes it.
    fn relay(&self, stream: tokio::net::TcpStream) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (cid, send) = {
            let mut s = self.state.borrow_mut();
            let Some(send) = s.worker.as_ref().map(|w| w.send.clone()) else { return };
            s.next_cid += 1;
            (s.next_cid, send)
        };
        let (queue, mut queued) = mpsc::channel::<Vec<u8>>(REPL_QUEUE);
        let _ = send.send(ToWorker::ReplOpen { cid });
        let (mut from, mut to) = stream.into_split();
        tokio::task::spawn_local(async move {
            while let Some(bytes) = queued.recv().await {
                if to.write_all(&bytes).await.is_err() {
                    break;
                }
            }
            let _ = to.shutdown().await;
        });
        let me = self.clone();
        let reader = tokio::task::spawn_local(async move {
            let mut chunk = vec![0; REPL_CHUNK];
            while let Ok(n @ 1..) = from.read(&mut chunk).await {
                if send.send(ToWorker::ReplData { cid, bytes: chunk[..n].to_vec() }).is_err() {
                    break;
                }
            }
            let ended = me.state.borrow_mut().repls.remove(&cid);
            if ended.is_some() {
                let _ = send.send(ToWorker::ReplClose { cid });
            }
        });
        let reader = reader.abort_handle();
        self.state.borrow_mut().repls.insert(cid, Repl { queue, reader });
    }

    /// Define `(host-fetch deployment request)` in the front world, as
    /// [`crate::Host::install`] does.
    pub fn install(&self, front: &mut Vm) {
        let wake = self.wake.clone();
        front.set_wake_notifier(move || wake.notify_one());
        let me = self.clone();
        body::install_front(front, move |deployment, request, body| me.fetch(deployment, request, body));
    }

    /// Drive `front` (installed) until its tasks have all finished.
    pub async fn run(&self, front: &mut Vm) -> Result<(), techne_vm::vm::Error> {
        loop {
            match front.run_tasks_for(Duration::from_millis(10))? {
                Progress::Finished => return Ok(()),
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
