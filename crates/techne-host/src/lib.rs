//! Serving deployments of techne Lisp from worlds.
//!
//! A [`Deployment`] is a project's code loaded once into a world and
//! snapshotted: its entry module provides `(fetch request env ctx)`, and
//! every instance of it is a world started from the snapshot
//! ([`Deployment::load`]). Loading reads only the deployment's directory
//! and Techne's own Lisp, the libraries it ships, and runs within bounds
//! of time and memory, without the clock or
//! randomness; an instance has no files, sockets, processes, console or
//! loading of modules, and has the clock, to the millisecond, and entropy.
//!
//! A [`Host`] serves deployments on its thread. It drives a front world, a
//! world of the host's own that receives requests (from HTTP, in Lisp:
//! `lisp/http`) and hands each to a deployment with `(host-fetch name
//! request)` ([`Host::install`]); and the instances answering them. The
//! request and the response cross from world to world as data
//! (`techne_vm::data`): dicts, strings, bytevectors. The host itself knows
//! nothing of HTTP.
//!
//! Each request runs as a task of an instance, `fetch` applied to it by
//! the host (no code of the host's is compiled where the deployment's
//! definitions could change it), in a lifetime of its own that bounds its
//! CPU time and its tasks and is ended with it, and within a deadline of
//! wall time. An instance answers one request at a time. It is kept for
//! the next only after a request that succeeded and left no task behind:
//! after any other, its state may be half changed (a kill runs no
//! cleanup), and it is dropped, as one whose world is poisoned or whose
//! deployment was replaced. Failures are answered as HTTP would: an
//! uncaught condition or a request past its CPU bound with 500, past its
//! deadline with 504, an unknown deployment with 404; each is reported to
//! the host's failure hook. A request whose front task stops waiting
//! (cancelled) is killed, unreported.
//!
//! What a deployment writes to its standard output and error does not reach
//! the host process's (only the runtime's own diagnostics, which `TECHNE_`
//! variables turn on, go there): an instance's are captured, up to `Limits::output`
//! bytes a request, and what a request wrote is handed to the host's
//! output hook when it ends, however it ends ([`Host::on_output`]). What
//! loading wrote is kept with the deployment ([`Deployment::output`]).
//!
//! A request's CPU bound holds within a slice too: Lisp looping where its
//! task cannot be preempted (a `define-library` body, `eval`, a native
//! calling back) is stopped by the VM's watchdog past it, as is loading a
//! deployment. A Rust native that never looks for stops is still beyond
//! it (runtime work 3 in doc/serverless.md).
//!
//! [`Host::run`] drives the front world and the instances on the current
//! thread, in turn, each for a slice, and waits when none has work: for a
//! wake from a future one of them waits on, or for the next timer or
//! deadline. Run it on a current-thread tokio runtime. A `Host` is a
//! handle: clones share it, so a deployment can be published while it
//! runs (from a task on the same thread).
//!
//! A deployment published for development ([`Host::publish_dev`]) is
//! served instead by one world, its dev world, kept across requests, those
//! that fail too, and driven between them as well, so that what is
//! attached to it ([`Attached`]: an nREPL server, through which a REPL
//! redefines its code) runs. A redefinition changes the next request:
//! every request looks `fetch` up anew, in instances and dev worlds alike.

use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use techne_vm::{
    data::Data,
    tasks::{LifetimeId, LifetimeLimits, Progress, TaskId},
    vm::{Capability, Error, Grants, Output, Snapshot, Vm},
};
use tokio::sync::{Notify, oneshot};

/// The most bytes of a request copied out of the front world: the front
/// bounds what it reads to less (`lisp/http`: a megabyte of body).
pub const REQUEST_BYTES: usize = 16 << 20;

/// What a deployment's loading, requests and instances may use.
#[derive(Clone, Debug)]
pub struct Limits {
    /// CPU time of a request and the tasks it spawns: past it, they are
    /// killed and the request answered 500.
    pub cpu: Duration,
    /// Wall time of a request: past it, it is killed and answered 504.
    pub wall: Duration,
    /// Tasks of a request unfinished at once.
    pub tasks: usize,
    /// Bytes an instance's world may hold, and the world loading it.
    pub memory: usize,
    /// Bytes of a response as it crosses into the front world.
    pub response: usize,
    /// Requests served at once, each by an instance of its own (at least
    /// one); more wait.
    pub concurrency: usize,
    /// Instances kept between requests.
    pub idle: usize,
    /// Wall time, and CPU time, of loading the deployment.
    pub load: Duration,
    /// Bytes of standard output and error kept of a request, and of
    /// loading; more is dropped, and the output marked truncated.
    pub output: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            cpu: Duration::from_millis(50),
            wall: Duration::from_secs(30),
            tasks: 64,
            memory: 128 << 20,
            response: 16 << 20,
            concurrency: 16,
            idle: 4,
            load: Duration::from_secs(10),
            output: 64 << 10,
        }
    }
}

/// A deployment: its code loaded into a world, snapshotted.
pub struct Deployment {
    snapshot: Snapshot,
    limits: Limits,
    output: Output,
}

impl Deployment {
    /// Load the module at `entry`, which must provide `fetch`, and what it
    /// requires, into a world granted only to load modules, from the
    /// entry's directory and Techne's own Lisp (`(techne http)` and the
    /// other libraries it ships) alone, within `limits.load` of time and
    /// `limits.memory`; take the grant back and snapshot the world.
    /// Loading reaches nothing else: no other files, sockets, processes,
    /// console, clock or entropy, so that it is deterministic. What a
    /// snapshot cannot hold (a task left running, a channel) is an error.
    pub fn load(entry: &Path, limits: Limits) -> Result<Deployment, Error> {
        let mut loading = Loading::start(entry, limits)?;
        loop {
            match loading.step() {
                Step::Done => return loading.finish(),
                Step::Wait(d) => std::thread::sleep(d),
                Step::Again => {}
            }
        }
    }

    /// [`Deployment::load`], yielding to the runtime it runs on between
    /// slices of the loading, so that other work on its thread (a host
    /// serving the deployment before) goes on meanwhile. Loading stuck
    /// where its task cannot be preempted (a `define-library` body) still
    /// holds the thread, up to `limits.load`.
    pub async fn load_async(entry: &Path, limits: Limits) -> Result<Deployment, Error> {
        let mut loading = Loading::start(entry, limits)?;
        loop {
            match loading.step() {
                Step::Done => return loading.finish(),
                Step::Wait(d) => tokio::time::sleep(d).await,
                Step::Again => tokio::task::yield_now().await,
            }
        }
    }

    /// What loading wrote to standard output and error.
    pub fn output(&self) -> &Output {
        &self.output
    }
}

/// What a dev world runs beside the requests it serves, an nREPL server
/// say ([`Host::publish_dev`]). The host calls `poll` before each slice of
/// the world's tasks, and `reap` after.
pub trait Attached {
    /// Handle what waits (which may give the world tasks to run); whether
    /// more waits.
    fn poll(&mut self, vm: &mut Vm) -> bool;
    /// See to what the slice just run ended.
    fn reap(&mut self, vm: &mut Vm);
}

/// A world started from a deployment's snapshot.
struct Instance {
    vm: Vm,
    /// The deployment's generation it was started from.
    generation: u64,
    /// What it runs beside its requests, if it is a dev world.
    attached: Option<Box<dyn Attached>>,
}

/// A deployment published under a name: its generation, which a new
/// publication bumps, and its instances waiting for requests (its dev
/// world, while no request runs, if it is published for development).
struct Published {
    deployment: Deployment,
    generation: u64,
    idle: Vec<Instance>,
    dev: bool,
}

/// A request the front world asked for.
struct Job {
    deployment: String,
    request: Data,
    reply: oneshot::Sender<Data>,
}

/// A request being served.
struct Running {
    deployment: String,
    id: u64,
    limits: Limits,
    instance: Instance,
    lifetime: LifetimeId,
    task: TaskId,
    deadline: Instant,
    reply: oneshot::Sender<Data>,
}

/// A failure the host answered for: the deployment, the request's id (as
/// its `ctx` has it; 0 for one never started), the status answered and
/// what happened.
#[derive(Clone, Debug)]
pub struct Failure {
    pub deployment: String,
    pub request: u64,
    pub status: u16,
    pub message: String,
}

/// What a request wrote to its standard output and error: the deployment,
/// the request's id (as its `ctx` has it) and the output.
#[derive(Clone, Debug)]
pub struct Printed {
    pub deployment: String,
    pub request: u64,
    pub output: Output,
}

struct State {
    published: HashMap<String, Published>,
    /// Requests being served, by deployment.
    busy: HashMap<String, usize>,
    running: Vec<Running>,
    next_id: u64,
    on_failure: Box<dyn FnMut(&Failure)>,
    on_output: Box<dyn FnMut(&Printed)>,
}

/// Serves deployments on its thread: see the crate's documentation.
#[derive(Clone)]
pub struct Host {
    state: Rc<RefCell<State>>,
    queue: Rc<RefCell<VecDeque<Job>>>,
    wake: Arc<Notify>,
}

impl Default for Host {
    fn default() -> Host {
        Host::new()
    }
}

impl Host {
    pub fn new() -> Host {
        Host {
            state: Rc::new(RefCell::new(State {
                published: HashMap::new(),
                busy: HashMap::new(),
                running: Vec::new(),
                next_id: 0,
                on_failure: Box::new(|f| eprintln!("techne-host: {} #{}: {} {}", f.deployment, f.request, f.status, f.message)),
                on_output: Box::new(|p| {
                    for line in p.output.text.lines() {
                        eprintln!("techne-host: {} #{}: {line}", p.deployment, p.request);
                    }
                    if p.output.truncated {
                        eprintln!("techne-host: {} #{}: (output cut short)", p.deployment, p.request);
                    }
                }),
            })),
            queue: Rc::new(RefCell::new(VecDeque::new())),
            wake: Arc::new(Notify::new()),
        }
    }

    /// Call `f` with each failure answered for, in place of writing it to
    /// standard error. It is called while the host serves, and must not
    /// call the host.
    pub fn on_failure(&self, f: impl FnMut(&Failure) + 'static) {
        self.state.borrow_mut().on_failure = Box::new(f);
    }

    /// Call `f` with what each request wrote to standard output and
    /// error, when it ends, if it wrote any, in place of writing it to
    /// standard error. It is called while the host serves, and must not
    /// call the host.
    pub fn on_output(&self, f: impl FnMut(&Printed) + 'static) {
        self.state.borrow_mut().on_output = Box::new(f);
    }

    /// Serve `deployment` under `name`, in place of the one published
    /// under it: requests from now on go to it; those being served finish
    /// on the one they started on, whose instances are then dropped.
    pub fn publish(&self, name: &str, deployment: Deployment) {
        let mut s = self.state.borrow_mut();
        let generation = s.published.get(name).map_or(0, |p| p.generation + 1);
        s.published.insert(name.to_string(), Published { deployment, generation, idle: Vec::new(), dev: false });
        drop(s);
        self.wake.notify_one();
    }

    /// Serve `deployment` under `name` for development, in place of the one
    /// published under it: from one world started from its snapshot, its
    /// dev world, which serves its requests one at a time and is kept
    /// after each, whatever its state (a request that failed may have left
    /// it half changed), until the world is poisoned or another deployment
    /// is published under `name`; the request being served finishes on the
    /// world before, which is then dropped. `attach` is given the dev world,
    /// its wake notifier the host's, to install what runs beside its
    /// requests; the host drives the world and what is attached also while
    /// no request runs.
    pub fn publish_dev(
        &self,
        name: &str,
        mut deployment: Deployment,
        attach: impl FnOnce(&mut Vm) -> Result<Box<dyn Attached>, Error>,
    ) -> Result<(), Error> {
        deployment.limits.concurrency = 1;
        let mut s = self.state.borrow_mut();
        let generation = s.published.get(name).map_or(0, |p| p.generation + 1);
        let mut world = new_instance(&deployment, generation, &self.wake)?;
        world.attached = Some(attach(&mut world.vm)?);
        s.published.insert(name.to_string(), Published { deployment, generation, idle: vec![world], dev: true });
        drop(s);
        self.wake.notify_one();
        Ok(())
    }

    /// What wakes the host from any thread, so that it drives its worlds:
    /// for what is attached to a dev world, called when something arrives
    /// for it from another thread.
    pub fn waker(&self) -> impl Fn() + Send + Sync + 'static {
        let wake = self.wake.clone();
        move || wake.notify_one()
    }

    /// Define `(host-fetch deployment request)` in the front world: the
    /// response of the deployment published as DEPLOYMENT (a string) to
    /// REQUEST, as `lisp/http` has them, or a response the host made for
    /// its failure. Call it before the front world's tasks start waiting:
    /// it sets the world's wake notifier to the host's.
    pub fn install(&self, front: &mut Vm) {
        let wake = self.wake.clone();
        front.set_wake_notifier(move || wake.notify_one());
        let queue = self.queue.clone();
        let wake = self.wake.clone();
        front.register_async("host-fetch", 2, move |vm, args| {
            let job = (|| {
                let deployment: String = args.get(vm, 0)?;
                let request = args.value(vm, 1)?;
                Ok::<_, Error>((deployment, vm.to_data(&request, REQUEST_BYTES)?))
            })();
            let (tx, rx) = oneshot::channel();
            let asked = job.map(|(deployment, request)| {
                queue.borrow_mut().push_back(Job { deployment, request, reply: tx });
                wake.notify_one();
            });
            async move {
                asked?;
                rx.await.map_err(|_| Error::new("host-fetch: the host stopped"))
            }
        });
    }

    /// Drive `front` (installed) and the instances until the front world's
    /// tasks have all finished and no request is being served.
    pub async fn run(&self, front: &mut Vm) {
        loop {
            let progress = front.run_tasks_for(Duration::from_millis(2));
            // Idle dev worlds first: a redefinition waiting there is in
            // place before a request starts.
            let developing = self.drive_dev_worlds();
            self.dispatch();
            let serving = self.serve() | developing;
            let idle = self.state.borrow().running.is_empty() && self.queue.borrow().is_empty();
            if matches!(progress, Progress::Finished) && idle {
                return;
            }
            if matches!(progress, Progress::OutOfTime) || serving {
                tokio::task::yield_now().await;
                continue;
            }
            let next = {
                let s = self.state.borrow();
                let dev = s.published.values().filter(|p| p.dev).flat_map(|p| p.idle.iter().map(|i| i.vm.next_timer()));
                s.running
                    .iter()
                    .flat_map(|r| [r.instance.vm.next_timer(), Some(r.deadline)])
                    .chain(dev)
                    .chain([front.next_timer()])
                    .flatten()
                    .min()
            };
            match next {
                Some(t) => {
                    let _ = tokio::time::timeout_at(t.into(), self.wake.notified()).await;
                }
                None => self.wake.notified().await,
            }
        }
    }

    /// Start the requests waiting that may start.
    fn dispatch(&self) {
        let jobs = std::mem::take(&mut *self.queue.borrow_mut());
        let mut waiting = VecDeque::new();
        let mut s = self.state.borrow_mut();
        for job in jobs {
            // Its front task was cancelled while it waited.
            if job.reply.is_closed() {
                continue;
            }
            let busy = s.busy.get(&job.deployment).copied().unwrap_or(0);
            let Some(p) = s.published.get_mut(&job.deployment) else {
                s.fail(job.deployment, 0, job.reply, 404, "no such deployment".into());
                continue;
            };
            if busy >= p.deployment.limits.concurrency {
                waiting.push_back(job);
                continue;
            }
            let limits = p.deployment.limits.clone();
            let instance = match p.idle.pop() {
                Some(i) => Ok(i),
                // A dev world gone (poisoned): nothing to serve with.
                None if p.dev => Err(Error::new("the dev world is gone; deploy again")),
                None => new_instance(&p.deployment, p.generation, &self.wake),
            };
            let dev = p.dev;
            s.next_id += 1;
            let id = s.next_id;
            let started = match instance {
                Ok(i) => match start(i, &job.request, id, &limits) {
                    (i, Ok((lifetime, task))) => Ok((i, lifetime, task)),
                    (i, Err(e)) => Err((Some(i), e)),
                },
                Err(e) => Err((None, e)),
            };
            match started {
                Ok((instance, lifetime, task)) => {
                    *s.busy.entry(job.deployment.clone()).or_default() += 1;
                    s.running.push(Running {
                        deployment: job.deployment,
                        id,
                        deadline: Instant::now() + limits.wall,
                        limits,
                        instance,
                        lifetime,
                        task,
                        reply: job.reply,
                    });
                }
                Err((instance, e)) => {
                    // A dev world is kept, as after a request that failed.
                    if let Some(i) = instance.filter(|i| dev && i.vm.poisoned().is_none())
                        && let Some(p) = s.published.get_mut(&job.deployment)
                        && p.generation == i.generation
                    {
                        p.idle.push(i);
                    }
                    s.fail(job.deployment, id, job.reply, 500, e.msg.clone())
                }
            }
        }
        // Before any asked for meanwhile (none: no world ran).
        let mut queue = self.queue.borrow_mut();
        waiting.append(&mut queue);
        *queue = waiting;
    }

    /// Run the dev worlds no request runs on for a slice, with what is
    /// attached to them. Whether one has more to run at once. One poisoned
    /// meanwhile is dropped, what is attached having seen it: nothing runs
    /// in it any more, nor do its timers.
    fn drive_dev_worlds(&self) -> bool {
        let mut busy = false;
        let mut s = self.state.borrow_mut();
        for p in s.published.values_mut().filter(|p| p.dev) {
            for world in &mut p.idle {
                busy |= slice(world);
            }
            p.idle.retain(|w| w.vm.poisoned().is_none());
        }
        busy
    }

    /// Run each request being served for a slice, and answer those done.
    /// Whether one has more to run at once.
    fn serve(&self) -> bool {
        let mut busy = false;
        let mut s = self.state.borrow_mut();
        let mut i = 0;
        while i < s.running.len() {
            let r = &mut s.running[i];
            busy |= slice(&mut r.instance);
            let vm = &mut r.instance.vm;
            let usage = vm.lifetime_usage(r.lifetime).unwrap_or_default();
            // What ends it before its result, in that order.
            let stopped = if r.reply.is_closed() {
                Some((499, "abandoned by the front".to_string()))
            } else if usage.cpu_exceeded {
                Some((500, "past its CPU bound".to_string()))
            } else if let Some(why) = vm.poisoned() {
                Some((500, format!("the world is poisoned: {why}")))
            } else if Instant::now() >= r.deadline {
                Some((504, "past its deadline".to_string()))
            } else {
                None
            };
            let result = if stopped.is_none() { vm.task_result(&r.task) } else { None };
            if stopped.is_none() && result.is_none() {
                i += 1;
                continue;
            }
            let mut r = s.running.swap_remove(i);
            // Tasks of the request left unfinished are killed with its
            // lifetime: the instance cannot be trusted after.
            let leftover = usage.live > 0;
            r.instance.vm.end_lifetime(r.lifetime);
            if let Some(output) = r.instance.vm.take_output().filter(|o| !o.text.is_empty() || o.truncated) {
                (s.on_output)(&Printed { deployment: r.deployment.clone(), request: r.id, output });
            }
            let answer = match (stopped, result) {
                (Some(stop), _) => Err(stop),
                (None, Some(Ok(v))) => {
                    r.instance.vm.to_data(&v, r.limits.response).map_err(|e| (500, format!("the response is no data: {}", e.msg)))
                }
                (None, Some(Err(e))) => Err((500, e.msg.clone())),
                (None, None) => unreachable!("waited for above"),
            };
            let clean = answer.is_ok() && !leftover;
            match answer {
                Ok(response) => {
                    let _ = r.reply.send(response);
                }
                // No one to answer: nothing to report.
                Err((499, _)) => {}
                Err((status, message)) => s.fail(r.deployment.clone(), r.id, r.reply, status, message),
            }
            // A dev world is kept whatever, unless poisoned: it holds what
            // a REPL made of it.
            let keep = clean || (r.instance.attached.is_some() && r.instance.vm.poisoned().is_none());
            s.release(&r.deployment, keep.then_some(r.instance));
            // One waiting for a place may start now.
            self.wake.notify_one();
        }
        busy
    }
}

impl State {
    /// A request of `deployment` is done: keep `instance` for the next if
    /// it may be.
    fn release(&mut self, deployment: &str, instance: Option<Instance>) {
        if let Some(n) = self.busy.get_mut(deployment) {
            *n = n.saturating_sub(1);
        }
        if let Some(p) = self.published.get_mut(deployment)
            && let Some(i) = instance
            && i.generation == p.generation
            && (p.dev || p.idle.len() < p.deployment.limits.idle)
        {
            p.idle.push(i);
        }
    }

    /// Answer a request with `status`, and report it.
    fn fail(&mut self, deployment: String, request: u64, reply: oneshot::Sender<Data>, status: u16, message: String) {
        (self.on_failure)(&Failure { deployment, request, status, message });
        let reason = match status {
            404 => "Not Found",
            504 => "Gateway Timeout",
            _ => "Internal Server Error",
        };
        let _ = reply.send(Data::Dict(vec![
            (Data::Symbol("status".into()), Data::Int(status.into())),
            (Data::Symbol("body".into()), Data::String(reason.into())),
        ]));
    }
}

/// A deployment being loaded: the module at its entry, loaded into its
/// world from a task in a lifetime of its own, bounded by `limits.load`
/// of CPU and of wall time, a slice at a time ([`Loading::step`]).
struct Loading {
    vm: Vm,
    entry: PathBuf,
    limits: Limits,
    lifetime: LifetimeId,
    task: TaskId,
    deadline: Instant,
    /// How it ended, once it has.
    result: Option<Result<(), Error>>,
}

/// What a loading asks for after a slice.
enum Step {
    Done,
    /// It waits on a timer: nothing to run for that long.
    Wait(Duration),
    /// It has more to run.
    Again,
}

impl Loading {
    fn start(entry: &Path, limits: Limits) -> Result<Loading, Error> {
        if limits.concurrency == 0 {
            return Err(Error::new("a deployment serves one request at once at least"));
        }
        let entry = entry.canonicalize().map_err(|e| Error::new(format!("{}: {e}", entry.display())))?;
        let mut vm = Vm::with_grants(Grants::NONE.with(Capability::Loading));
        vm.confine_reading(entry.parent().unwrap_or(Path::new("/")))?;
        vm.set_memory_limit(limits.memory);
        vm.capture_output(limits.output);
        // Made before the deployment's code exists: nothing of it can
        // change what this calls. A top-level `require` evaluated from
        // Scheme loads the module from Scheme too, where its task can be
        // preempted and stopped (`eval` would load it as it compiles,
        // where it cannot).
        let source = format!("(require {:?})", entry.display().to_string());
        let prefixed = |e: Error| Error::new(format!("{}: {}", entry.display(), e.msg));
        let load = vm.eval_source(&format!("(lambda () (eval-source {source:?} \"user\" \"<host>\"))")).map_err(prefixed)?;
        let lifetime = vm.new_lifetime(LifetimeLimits { cpu: Some(limits.load), tasks: None });
        let task = vm.spawn_in(lifetime, &load).map_err(prefixed)?;
        let deadline = Instant::now() + limits.load;
        Ok(Loading { vm, entry, limits, lifetime, task, deadline, result: None })
    }

    /// Run the loading for a slice.
    fn step(&mut self) -> Step {
        // The deadline first: a load done past it took too long too.
        if Instant::now() >= self.deadline {
            self.result = Some(Err(Error::new("it takes too long to load")));
            return Step::Done;
        }
        if let Some(r) = self.vm.task_result(&self.task) {
            self.result = Some(r.map(drop));
            return Step::Done;
        }
        match self.vm.run_tasks_for(Duration::from_millis(10)) {
            Progress::Blocked => {
                let until = self.vm.next_timer().map_or(self.deadline, |t| t.min(self.deadline));
                Step::Wait(until.saturating_duration_since(Instant::now()).min(Duration::from_millis(10)))
            }
            _ => Step::Again,
        }
    }

    /// The deployment, once loaded: its world without loading, its
    /// output taken, snapshotted. Nothing the loading made stays rooted
    /// from Rust.
    fn finish(mut self) -> Result<Deployment, Error> {
        let entry = self.entry.display().to_string();
        let usage = self.vm.end_lifetime(self.lifetime);
        let loaded = match usage {
            Some(u) if u.cpu_exceeded => Err(Error::new("it takes too long to load")),
            _ => self.result.take().unwrap_or_else(|| Err(Error::new("not loaded"))),
        };
        let Loading { mut vm, limits, task, .. } = self;
        drop(task);
        loaded.map_err(|e| Error::new(format!("{entry}: {}", e.msg)))?;
        if !vm.get_global("fetch").is_some_and(|f| vm.is_procedure(&f)) {
            return Err(Error::new(format!("{entry}: provides no fetch procedure")));
        }
        vm.set_grant(Capability::Loading, false);
        let output = vm.take_output().unwrap_or_default();
        Ok(Deployment { snapshot: vm.snapshot()?, limits, output })
    }
}

/// A world started from `deployment`'s snapshot, granted the clock, to
/// the millisecond, and entropy, waking `wake` when a future one of its
/// tasks waits on is ready.
fn new_instance(deployment: &Deployment, generation: u64, wake: &Arc<Notify>) -> Result<Instance, Error> {
    let mut vm = deployment.snapshot.world();
    vm.set_grant(Capability::Clock, true);
    vm.set_grant(Capability::Entropy, true);
    vm.set_clock_resolution(Duration::from_millis(1));
    vm.set_memory_limit(deployment.limits.memory);
    vm.capture_output(deployment.limits.output);
    let notify = wake.clone();
    vm.set_wake_notifier(move || notify.notify_one());
    Ok(Instance { vm, generation, attached: None })
}

/// Run `instance` for a slice, with what is attached to it: whether it
/// has more to run at once.
fn slice(instance: &mut Instance) -> bool {
    let more = instance.attached.as_mut().is_some_and(|a| a.poll(&mut instance.vm));
    let progress = instance.vm.run_tasks_for(Duration::from_millis(2));
    if let Some(a) = &mut instance.attached {
        a.reap(&mut instance.vm);
    }
    more || matches!(progress, Progress::OutOfTime)
}

/// Start `request`, numbered `id`, on `instance`: `(fetch request env
/// ctx)` in a lifetime of its own, `fetch` as the world has it now (a dev
/// world's may have been redefined). The instance is handed back, started
/// or not.
fn start(mut instance: Instance, request: &Data, id: u64, limits: &Limits) -> (Instance, Result<(LifetimeId, TaskId), Error>) {
    let vm = &mut instance.vm;
    // What a dev world wrote between requests is no request's.
    let _ = vm.take_output();
    let begun = (|| {
        let fetch = vm.get_global("fetch").filter(|f| vm.is_procedure(f)).ok_or_else(|| Error::new("no fetch procedure"))?;
        let request = vm.from_data(request)?;
        let env = vm.from_data(&Data::Dict(Vec::new()))?;
        let ctx = vm.from_data(&Data::Dict(vec![(Data::Symbol("id".into()), Data::Int(id as i64))]))?;
        let lifetime = vm.new_lifetime(LifetimeLimits { cpu: Some(limits.cpu), tasks: Some(limits.tasks) });
        match vm.spawn_in_with(lifetime, &fetch, &[request, env, ctx]) {
            Ok(task) => Ok((lifetime, task)),
            Err(e) => {
                vm.end_lifetime(lifetime);
                Err(e)
            }
        }
    })();
    (instance, begun)
}
