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
//! nothing of HTTP. A body may stream instead ([`body`]), a procedure as
//! `lisp/http` has it: a request's is read by the deployment as it calls
//! it, a response's (`(body write)`) is written by the request after its
//! head is given, within its lifetime, CPU bound and deadline, on its
//! instance, which serves no other request until the body's end. A
//! failure past the head cuts the body short.
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
//! deadline (including its queue wait) with 504, an unknown deployment with
//! 404, a full admission queue with 503; each is reported to the host's
//! failure hook. A request whose front task stops waiting
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
//!
//! A deployment in production may have a debug instance beside its
//! instances ([`Host::attach_debug`]): a world started from the same
//! snapshot, kept as a dev world is and with what is attached to it, that
//! serves only the requests routed to it ([`Host::fetch_routed`]; who
//! routes them is the caller's to decide), one at a time, each allowed
//! `Limits::debug_pause` more of wall time, to wait in a debugger. The
//! others are served by the deployment's instances as before, from its
//! code as deployed. It never joins them: detached
//! ([`Host::detach_debug`]), poisoned, or its deployment replaced, it is
//! dropped, and the requests routed to it then are served as any other.

pub mod body;
pub mod egress;
pub mod outbound;
pub mod supervisor;
pub mod wire;
pub mod worker;

use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

pub use body::Response;
use body::{Exchange, Reader};
use techne_vm::{
    api::Value,
    data::Data,
    tasks::{LifetimeId, LifetimeLimits, Progress, TaskId},
    vm::{Capability, Error, Grants, Output, ROOT_MODULE, Snapshot, Vm},
};
use tokio::sync::{Notify, oneshot};

/// How much longer than its deployment's wall time a request routed to a
/// debug instance may take, waiting in a debugger.
pub const DEBUG_PAUSE: Duration = Duration::from_secs(60);

/// The most bytes of a request copied out of the front world: the front
/// bounds what it reads to less (`lisp/http`: a megabyte of body).
pub const REQUEST_BYTES: usize = 16 << 20;

/// What a deployment's loading, requests and instances may use.
#[derive(Clone, Debug)]
pub struct Limits {
    /// CPU time of a request and the tasks it spawns: past it, they are
    /// killed and the request answered 500.
    pub cpu: Duration,
    /// Wall time from admission, including time queued: past it, the
    /// request is answered 504, without starting it if it is still queued.
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
    /// Requests admitted beyond those that can run at once. More are
    /// refused with 503; zero allows only the concurrency's worth.
    pub queue: usize,
    /// Instances kept between requests: as many as serve at once, so that
    /// a deployment busy at its concurrency makes none anew.
    pub idle: usize,
    /// Wall time, and CPU time, of loading the deployment.
    pub load: Duration,
    /// Bytes of standard output and error kept of a request, and of
    /// loading; more is dropped, and the output marked truncated.
    pub output: usize,
    /// Fetches of a request (`fetch-url`).
    pub fetches: usize,
    /// Wall time a request routed to a debug instance has beyond `wall`.
    pub debug_pause: Duration,
    /// Operations of a request on its bindings' storage (`kv-get` and
    /// the others).
    pub store_ops: usize,
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
            queue: 64,
            idle: 16,
            load: Duration::from_secs(10),
            output: 64 << 10,
            fetches: 50,
            debug_pause: DEBUG_PAUSE,
            store_ops: 1000,
        }
    }
}

/// A deployment: its code loaded into a world, snapshotted, and the
/// bindings its requests are given.
pub struct Deployment {
    snapshot: Snapshot,
    limits: Limits,
    output: Output,
    env: Rc<Data>,
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

    /// The deployment, its requests given `env` (a dict, by the names
    /// the deployment has for its bindings: a secret's value, a KV
    /// namespace's handle as `kv-get` takes it) in place of an empty one.
    /// Loading saw none of it: a snapshot holds no secret.
    pub fn with_env(mut self, env: Data) -> Deployment {
        self.env = Rc::new(env);
        self
    }
}

/// What a dev world or a debug instance runs beside the requests it
/// serves, an nREPL server say ([`Host::publish_dev`],
/// [`Host::attach_debug`]). The host calls `poll` before each slice of
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
    /// What it runs beside its requests, if it is a dev world or a debug
    /// instance.
    attached: Option<Box<dyn Attached>>,
    /// What a dev world's requests, and a debug instance's, are started
    /// with (`DEV_SERVE`).
    serve: Option<Value>,
    /// Which debug instance it is, if it is one.
    debug: Option<u64>,
}

/// A deployment's debug instance: its number, and the world while no
/// request runs on it.
struct Debug {
    id: u64,
    idle: Option<Instance>,
}

/// A deployment published under a name: its generation, which a new
/// publication bumps, its instances waiting for requests (its dev
/// world, while no request runs, if it is published for development),
/// and its debug instance, if one is attached.
struct Published {
    deployment: Deployment,
    generation: u64,
    idle: Vec<Instance>,
    dev: bool,
    debug: Option<Debug>,
}

/// A request the front world asked for.
struct Job {
    deployment: String,
    /// What it was asked under (`fetch_as`).
    tag: u64,
    request: Data,
    /// When admitted, its deadline. A request asked while the host serves
    /// (a broker fetching from another deployment of it) is admitted when
    /// dispatched.
    deadline: Option<Instant>,
    exchange: Rc<Exchange>,
    /// Routed to the deployment's debug instance, if it has one.
    debug: bool,
    /// How much of its deadline is the allowance of a request so routed:
    /// taken back if it is served as any other after all.
    pause: Duration,
}

/// A request being served.
struct Running {
    deployment: String,
    id: u64,
    limits: Limits,
    instance: Instance,
    /// Its debug instance was detached: it ends now.
    retired: bool,
    lifetime: LifetimeId,
    task: TaskId,
    deadline: Instant,
    exchange: Rc<Exchange>,
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
    /// Requests being served by instances, by deployment: not those on a
    /// debug instance, which take no instance's place.
    busy: HashMap<String, usize>,
    running: Vec<Running>,
    next_id: u64,
    next_debug: u64,
    broker: Option<outbound::Broker>,
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
                next_debug: 0,
                broker: None,
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
        // The debug instance of the one before goes with it.
        s.retire_debug(name);
        s.published.insert(name.to_string(), Published { deployment, generation, idle: Vec::new(), dev: false, debug: None });
        drop(s);
        self.wake.notify_one();
    }

    /// Give the deployment published under `name` a debug instance, in
    /// place of the one it has (a request running on that one is answered
    /// 504): a world started from its snapshot, kept
    /// across the requests routed to it ([`Host::fetch_routed`]), those
    /// that fail too, which it serves one at a time as a dev world serves
    /// its own (`%host-debugger`, `retry`, `use-value`); the deployment's
    /// other requests are served by its instances as before. `attach` is
    /// given the world, as [`Host::publish_dev`]'s is, and the host drives
    /// it and what is attached also while no request runs on it. It lasts
    /// until detached, poisoned, or another deployment is published under
    /// `name`, which answers a request running on it 504. As many
    /// requests may wait for it as the deployment's `Limits::queue` and
    /// one; more are refused with 503, whatever its instances serve. Its
    /// number, for [`Host::detach_debug`].
    pub fn attach_debug(&self, name: &str, attach: impl FnOnce(&mut Vm) -> Result<Box<dyn Attached>, Error>) -> Result<u64, Error> {
        let mut s = self.state.borrow_mut();
        s.next_debug += 1;
        let id = s.next_debug;
        let p = s.published.get(name).ok_or_else(|| Error::new("no such deployment"))?;
        if p.dev {
            return Err(Error::new("the deployment is served from its dev world"));
        }
        let mut world = new_instance(&p.deployment, p.generation, &self.wake)?;
        world.debug = Some(id);
        world.serve = Some(install_dev(&mut world.vm)?);
        world.attached = Some(attach(&mut world.vm)?);
        s.retire_debug(name);
        if let Some(p) = s.published.get_mut(name) {
            p.debug = Some(Debug { id, idle: Some(world) });
        }
        drop(s);
        self.wake.notify_one();
        Ok(id)
    }

    /// Drop debug instance `id` of the deployment published under `name`
    /// (any, with no `id`), if it has it still: a request running on it is
    /// answered 504, and those routed to it from now on are served as any
    /// other. Whether there was one.
    pub fn detach_debug(&self, name: &str, id: Option<u64>) -> bool {
        let mut s = self.state.borrow_mut();
        let attached = s.published.get(name).and_then(|p| p.debug.as_ref()).is_some_and(|d| id.is_none_or(|id| id == d.id));
        if attached {
            s.retire_debug(name);
            drop(s);
            self.wake.notify_one();
        }
        attached
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
        world.serve = Some(install_dev(&mut world.vm)?);
        world.attached = Some(attach(&mut world.vm)?);
        s.retire_debug(name);
        s.published.insert(name.to_string(), Published { deployment, generation, idle: vec![world], dev: true, debug: None });
        drop(s);
        self.wake.notify_one();
        Ok(())
    }

    /// Have `broker` fetch what requests ask for (`fetch-url`); without
    /// one, they fetch nothing.
    pub fn set_broker(&self, broker: outbound::Broker) {
        self.state.borrow_mut().broker = Some(broker);
    }

    /// What wakes the host from any thread, so that it drives its worlds:
    /// for what is attached to a dev world, called when something arrives
    /// for it from another thread.
    pub fn waker(&self) -> impl Fn() + Send + Sync + 'static {
        let wake = self.wake.clone();
        move || wake.notify_one()
    }

    /// Define `(host-fetch deployment request #:debug #f)` in the front
    /// world: the response of the deployment published as DEPLOYMENT (a
    /// string) to REQUEST, as `lisp/http` has them, or a response the host
    /// made for its failure; with DEBUG, from the deployment's debug
    /// instance, if it has one ([`Host::fetch_routed`]). A request's body
    /// may stream, and a response's (procedures, as `lisp/http` has
    /// them). Call it before the front world's tasks start waiting: it
    /// sets the world's wake notifier to the host's.
    pub fn install(&self, front: &mut Vm) {
        let wake = self.wake.clone();
        front.set_wake_notifier(move || wake.notify_one());
        let host = self.clone();
        body::install_front(front, move |deployment, request, body, debug| host.fetch_routed(0, deployment, request, body, debug));
    }

    /// Ask the deployment published as `deployment` to answer `request`,
    /// whose body streams from `body` if given (`request`'s own is then
    /// empty): its response, or one the host made for its failure, once
    /// served. Dropping the receiver, or the response's body, abandons the
    /// request (killed, unreported).
    pub fn fetch(&self, deployment: &str, request: Data, body: Option<Reader>) -> oneshot::Receiver<Response> {
        self.fetch_as(0, deployment, request, body)
    }

    /// [`Host::fetch`], the request's fetches asked of the broker under
    /// `tag` (a supervisor's number for it, say). Admission reserves a
    /// place within the deployment's concurrency and queue limits, and
    /// starts its wall deadline. Overload is answered 503, reported.
    pub fn fetch_as(&self, tag: u64, deployment: &str, request: Data, body: Option<Reader>) -> oneshot::Receiver<Response> {
        self.fetch_routed(tag, deployment, request, body, false)
    }

    /// [`Host::fetch_as`], the request served by the deployment's debug
    /// instance if `debug` and it has one ([`Host::attach_debug`]), after
    /// those routed to it before, within `Limits::debug_pause` more of
    /// wall time. A deployment without one serves it as any other, within
    /// its own wall time, also if the debug instance goes while the
    /// request waits for it: what routes requests need not know whether a
    /// debug instance lasts.
    pub fn fetch_routed(
        &self,
        tag: u64,
        deployment: &str,
        request: Data,
        body: Option<Reader>,
        debug: bool,
    ) -> oneshot::Receiver<Response> {
        let (tx, rx) = oneshot::channel();
        let exchange = Exchange::default();
        *exchange.reply.borrow_mut() = Some(tx);
        *exchange.reader.borrow_mut() = body;
        let now = Instant::now();
        let mut queue = self.queue.borrow_mut();
        let Ok(mut s) = self.state.try_borrow_mut() else {
            let job = Job {
                deployment: deployment.into(),
                tag,
                request,
                deadline: None,
                exchange: Rc::new(exchange),
                debug,
                pause: Duration::ZERO,
            };
            queue.push_back(job);
            self.wake.notify_one();
            return rx;
        };
        // Release expired and abandoned admissions before counting places.
        queue.retain(|job| {
            if job.exchange.abandoned() {
                return false;
            }
            if job.deadline.is_some_and(|d| d <= now) {
                s.fail(job.deployment.clone(), 0, &job.exchange, 504, "past its deadline".into());
                return false;
            }
            true
        });
        let Some(p) = s.published.get(deployment) else {
            s.fail(deployment.into(), 0, &exchange, 404, "no such deployment".into());
            return rx;
        };
        let debug = debug && p.debug.is_some();
        let (wall, pause) = (p.deployment.limits.wall, if debug { p.deployment.limits.debug_pause } else { Duration::ZERO });
        if s.full(deployment, debug, queue.iter()) {
            s.fail(deployment.into(), 0, &exchange, 503, "the request queue is full".into());
            return rx;
        }
        let deadline = Some(now + wall + pause);
        queue.push_back(Job { deployment: deployment.into(), tag, request, deadline, exchange: Rc::new(exchange), debug, pause });
        self.wake.notify_one();
        rx
    }

    /// Drive `front` (installed) and the instances until the front world's
    /// tasks have all finished and no request is being served; with no
    /// front, for ever (a worker's, its requests from [`Host::fetch`]).
    pub async fn run(&self, mut front: Option<&mut Vm>) -> Result<(), Error> {
        loop {
            let progress = match front.as_mut() {
                Some(f) => f.run_tasks_for(Duration::from_millis(2))?,
                None => Progress::Blocked,
            };
            // Idle dev worlds first: a redefinition waiting there is in
            // place before a request starts.
            let developing = self.drive_dev_worlds();
            self.dispatch();
            let serving = self.serve() | developing;
            let idle = self.state.borrow().running.is_empty() && self.queue.borrow().is_empty();
            if matches!(progress, Progress::Finished) && idle && front.is_some() {
                return Ok(());
            }
            if matches!(progress, Progress::OutOfTime) || serving {
                tokio::task::yield_now().await;
                continue;
            }
            let next = {
                let s = self.state.borrow();
                let dev = s.published.values().flat_map(|p| {
                    let worlds = p.idle.iter().filter(|_| p.dev).chain(p.debug.iter().flat_map(|d| &d.idle));
                    worlds.map(|i| i.vm.next_timer())
                });
                s.running
                    .iter()
                    .flat_map(|r| [r.instance.vm.next_timer(), Some(r.deadline)])
                    .chain(dev)
                    .chain(self.queue.borrow().iter().map(|j| j.deadline))
                    .chain([front.as_ref().and_then(|f| f.next_timer())])
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
        for mut job in jobs {
            // Its front task was cancelled while it waited.
            if job.exchange.abandoned() {
                continue;
            }
            // Asked while the host served: admitted now, as `fetch_as` does.
            if job.deadline.is_none() {
                let Some(p) = s.published.get(&job.deployment) else {
                    s.fail(job.deployment, 0, &job.exchange, 404, "no such deployment".into());
                    continue;
                };
                job.debug &= p.debug.is_some();
                let limits = &p.deployment.limits;
                job.pause = if job.debug { limits.debug_pause } else { Duration::ZERO };
                let wall = limits.wall;
                if s.full(&job.deployment, job.debug, waiting.iter().chain(&*self.queue.borrow())) {
                    s.fail(job.deployment, 0, &job.exchange, 503, "the request queue is full".into());
                    continue;
                }
                job.deadline = Some(Instant::now() + wall + job.pause);
            }
            // Its debug instance gone meanwhile: served as any other,
            // within the time any other has.
            if job.debug && !s.published.get(&job.deployment).is_some_and(|p| p.debug.is_some()) {
                job.debug = false;
                job.deadline = job.deadline.map(|d| d.checked_sub(job.pause).unwrap_or(d));
                job.pause = Duration::ZERO;
            }
            let deadline = job.deadline.expect("admitted");
            if deadline <= Instant::now() {
                s.fail(job.deployment, 0, &job.exchange, 504, "past its deadline".into());
                continue;
            }
            let busy = s.busy.get(&job.deployment).copied().unwrap_or(0);
            let Some(p) = s.published.get_mut(&job.deployment) else {
                s.fail(job.deployment, 0, &job.exchange, 404, "no such deployment".into());
                continue;
            };
            let instance = match &mut p.debug {
                // After the request it serves now.
                Some(d) if job.debug => match d.idle.take() {
                    Some(i) => Ok(i),
                    None => {
                        waiting.push_back(job);
                        continue;
                    }
                },
                _ if busy >= p.deployment.limits.concurrency => {
                    waiting.push_back(job);
                    continue;
                }
                _ => match p.idle.pop() {
                    Some(i) => Ok(i),
                    // A dev world gone (poisoned): nothing to serve with.
                    None if p.dev => Err(Error::new("the dev world is gone; deploy again")),
                    None => new_instance(&p.deployment, p.generation, &self.wake),
                },
            };
            let limits = p.deployment.limits.clone();
            let env = p.deployment.env.clone();
            s.next_id += 1;
            let id = s.next_id;
            let context = Rc::new(outbound::Context {
                deployment: job.deployment.clone(),
                tag: job.tag,
                broker: s.broker.clone(),
                fetches: Default::default(),
                limit: limits.fetches,
                store_ops: Default::default(),
                store_limit: limits.store_ops,
            });
            let started = match instance {
                Ok(i) => match start(i, &job.request, &job.exchange, context, id, &limits, &env) {
                    (i, Ok((lifetime, task))) => Ok((i, lifetime, task)),
                    (i, Err(e)) => Err((Some(i), e)),
                },
                Err(e) => Err((None, e)),
            };
            match started {
                Ok((instance, lifetime, task)) => {
                    if instance.debug.is_none() {
                        *s.busy.entry(job.deployment.clone()).or_default() += 1;
                    }
                    s.running.push(Running {
                        deployment: job.deployment,
                        id,
                        deadline,
                        limits,
                        instance,
                        retired: false,
                        lifetime,
                        task,
                        exchange: job.exchange,
                    });
                }
                Err((instance, e)) => {
                    // A dev world is kept, as after a request that failed,
                    // and a debug instance.
                    s.keep(&job.deployment, instance.filter(|i| i.attached.is_some() && i.vm.poisoned().is_none()));
                    s.fail(job.deployment, id, &job.exchange, 500, e.msg.clone())
                }
            }
        }
        // Before any asked for meanwhile (none: no world ran).
        let mut queue = self.queue.borrow_mut();
        waiting.append(&mut queue);
        *queue = waiting;
    }

    /// Run the dev worlds and debug instances no request runs on for a
    /// slice, with what is attached to them. Whether one has more to run
    /// at once. One poisoned meanwhile is dropped, what is attached having
    /// seen it: nothing runs in it any more, nor do its timers.
    fn drive_dev_worlds(&self) -> bool {
        let mut busy = false;
        let mut s = self.state.borrow_mut();
        for p in s.published.values_mut() {
            if p.dev {
                p.idle.retain_mut(|world| match slice(world) {
                    Ok(more) => {
                        busy |= more;
                        true
                    }
                    Err(_) => false,
                });
            }
            let gone = match p.debug.as_mut().and_then(|d| d.idle.as_mut()).map(slice) {
                Some(Ok(more)) => {
                    busy |= more;
                    false
                }
                Some(Err(_)) => true,
                None => false,
            };
            if gone {
                p.debug = None;
            }
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
            // One whose debug instance was detached runs no more.
            let slice_error = if Instant::now() < r.deadline && !r.exchange.abandoned() && !r.retired {
                match slice(&mut r.instance) {
                    Ok(more) => {
                        busy |= more;
                        None
                    }
                    Err(e) => Some(e),
                }
            } else {
                None
            };
            let vm = &mut r.instance.vm;
            let usage = vm.lifetime_usage(r.lifetime).unwrap_or_default();
            // What ends it before its result, in that order.
            let stopped = if r.exchange.abandoned() {
                Some((499, "abandoned by the front".to_string()))
            } else if usage.cpu_exceeded {
                Some((500, "past its CPU bound".to_string()))
            } else if let Some(why) = vm.poisoned() {
                Some((500, format!("the world is poisoned: {why}")))
            } else if let Some(error) = slice_error {
                Some((500, error.to_string()))
            } else if r.retired {
                Some((504, "its debug instance was detached".to_string()))
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
            // The request's body, what is left of it, is no longer read.
            r.exchange.reader.borrow_mut().take();
            if let Some(output) = r.instance.vm.take_output().filter(|o| !o.text.is_empty() || o.truncated) {
                (s.on_output)(&Printed { deployment: r.deployment.clone(), request: r.id, output });
            }
            let streamed = r.exchange.streams.get();
            let answer = match (stopped, result) {
                (Some(stop), _) => Err(stop),
                // Its body written (`%host-serve`), to be ended.
                (None, Some(Ok(_))) if streamed => Ok(None),
                (None, Some(Ok(v))) => {
                    r.instance.vm.to_data(&v, r.limits.response).map(Some).map_err(|e| (500, format!("the response is no data: {}", e.msg)))
                }
                (None, Some(Err(e))) => Err((500, e.msg.clone())),
                (None, None) => unreachable!("waited for above"),
            };
            let clean = answer.is_ok() && !leftover;
            match answer {
                Ok(Some(response)) => r.exchange.answer(response, ""),
                Ok(None) => {
                    if let Some(w) = r.exchange.writer.borrow_mut().take() {
                        w.end(Ok(()));
                    }
                }
                // No one to answer: nothing to report.
                Err((499, _)) => {}
                Err((status, message)) => s.fail(r.deployment.clone(), r.id, &r.exchange, status, message),
            }
            // A body still being written (by a task the request left) is
            // cut short.
            r.exchange.writer.borrow_mut().take();
            // A dev world is kept whatever, unless poisoned: it holds what
            // a REPL made of it.
            let keep = (clean || r.instance.attached.is_some()) && r.instance.vm.poisoned().is_none() && !r.retired;
            let debug = r.instance.debug;
            s.release(&r.deployment, keep.then_some(r.instance), debug);
            // One waiting for a place may start now.
            self.wake.notify_one();
        }
        busy
    }
}

impl State {
    /// A request of `deployment` is done, served by debug instance `debug`
    /// if by one: keep `instance` for the next if it may be. A debug
    /// instance not kept (poisoned) is the deployment's no longer.
    fn release(&mut self, deployment: &str, instance: Option<Instance>, debug: Option<u64>) {
        match debug {
            Some(id) if instance.is_none() => {
                if let Some(p) = self.published.get_mut(deployment)
                    && p.debug.as_ref().is_some_and(|d| d.id == id)
                {
                    p.debug = None;
                }
            }
            Some(_) => {}
            None => {
                if let Some(n) = self.busy.get_mut(deployment) {
                    *n = n.saturating_sub(1);
                }
            }
        }
        self.keep(deployment, instance);
    }

    /// Keep `instance` for `deployment`'s next request, if it is still of
    /// the deployment published: a debug instance where it was attached,
    /// if it still is; a dev world; another while few enough wait.
    fn keep(&mut self, deployment: &str, instance: Option<Instance>) {
        let (Some(p), Some(i)) = (self.published.get_mut(deployment), instance) else { return };
        if i.generation != p.generation {
            return;
        }
        match (i.debug, &mut p.debug) {
            (Some(id), Some(d)) if d.id == id && d.idle.is_none() => d.idle = Some(i),
            (Some(_), _) => {}
            (None, _) if p.dev || p.idle.len() < p.deployment.limits.idle => p.idle.push(i),
            (None, _) => {}
        }
    }

    /// Whether `deployment`, published, can admit no more requests beside
    /// those `queued`: of those for its debug instance if `debug`, as many
    /// as its queue holds and the one served; else of the others, as many
    /// as its queue holds and its instances serve. Neither takes the
    /// other's places.
    fn full<'a>(&self, deployment: &str, debug: bool, queued: impl Iterator<Item = &'a Job>) -> bool {
        let Some(limits) = self.published.get(deployment).map(|p| &p.deployment.limits) else { return true };
        let queued = queued.filter(|j| j.deployment == deployment && j.debug == debug).count();
        if debug {
            let running = self.running.iter().filter(|r| r.deployment == deployment && r.instance.debug.is_some()).count();
            queued + running > limits.queue
        } else {
            queued + self.busy.get(deployment).copied().unwrap_or(0) >= limits.concurrency.saturating_add(limits.queue)
        }
    }

    /// `deployment` has its debug instance no longer: the request running
    /// on it ends.
    fn retire_debug(&mut self, deployment: &str) {
        let Some(d) = self.published.get_mut(deployment).and_then(|p| p.debug.take()) else { return };
        for r in self.running.iter_mut().filter(|r| r.deployment == deployment && r.instance.debug == Some(d.id)) {
            r.retired = true;
        }
    }

    /// Answer a request with `status`, or cut its body short if its
    /// response was given, and report it.
    fn fail(&mut self, deployment: String, request: u64, exchange: &Exchange, status: u16, message: String) {
        exchange.answer(failed(status), &message);
        (self.on_failure)(&Failure { deployment, request, status, message });
    }
}

/// The response the host makes for a failure answered `status`.
pub(crate) fn failed(status: u16) -> Data {
    let reason = match status {
        404 => "Not Found",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Internal Server Error",
    };
    Data::Dict(vec![(Data::Symbol("status".into()), Data::Int(status.into())), (Data::Symbol("body".into()), Data::String(reason.into()))])
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
        if let Some(setup) = WORLD_SETUP.get() {
            setup(&mut vm);
        }
        vm.confine_reading(entry.parent().unwrap_or(Path::new("/")))?;
        // Before the deployment's code: nothing of it changes what serves
        // its requests.
        body::install(&mut vm);
        outbound::install(&mut vm);
        vm.eval_in(ROOT_MODULE, "<techne-host>", SERVE)?;
        vm.eval_in(ROOT_MODULE, "<techne-host>", outbound::LISP)?;
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
            Err(e) => {
                self.result = Some(Err(e));
                Step::Done
            }
            Ok(Progress::Blocked) => {
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
        Ok(Deployment { snapshot: vm.snapshot()?, limits, output, env: Rc::new(Data::Dict(Vec::new())) })
    }
}

/// What every world a deployment loads into is given first: for tests,
/// natives of their own (`set_world_setup`).
static WORLD_SETUP: std::sync::OnceLock<fn(&mut Vm)> = std::sync::OnceLock::new();

/// Give every world deployments load into `setup` first (its natives go
/// into their snapshots): for tests, a native that never returns, say.
/// Set once.
pub fn set_world_setup(setup: fn(&mut Vm)) {
    let _ = WORLD_SETUP.set(setup);
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
    Ok(Instance { vm, generation, attached: None, serve: None, debug: None })
}

/// How a world serves a request: `(fetch request env ctx)`, FETCH the
/// deployment's or a dev world's (`DEV_SERVE`), with the request's body
/// read from exchange X if READS; a response whose body is a procedure is
/// given with an empty one, and its body then written to X (`(body
/// write)`, within the request); the host ends it once the request has
/// succeeded within its bounds.
const SERVE: &str = r#"
(define (%host-serve fetch request x reads env ctx)
  (let* ((request (if reads
                      (dict-set request 'body (lambda () (or (%host-body-read x) (eof-object))))
                      request))
         (response (fetch request env ctx))
         (body (and (dict? response) (dict-ref response 'body #f))))
    (cond ((procedure? body)
           (%host-respond x (dict-set response 'body ""))
           (body (lambda (data) (%host-body-write x data)))
           #t)
          (else response))))
"#;

/// How a dev world serves a request: `fetch` as the world has it now, with
/// restarts for a debugger (`%host-debugger`, a procedure of the condition
/// that does not return if it invokes a restart, and returns #f
/// otherwise): `retry` calls `fetch` again, looked up anew, as the same
/// request, its budgets spent so far still spent, what the first attempt
/// did done and the tasks it started still running; `use-value` answers
/// with a response given. Conditions the world raises to stop the request
/// (interrupted, out of memory) go on, debugger or not.
const DEV_SERVE: &str = r#"
(define %host-debugger #f)
(lambda (request env ctx)
  (let again ()
    (restart-case
     (with-exception-handler
      (lambda (c)
        (if (and %host-debugger
                 (not (and (error-object? c)
                           (let ((m (error-object-message c)))
                             (or (equal? m "interrupted")
                                 (and (string? m) (>= (string-length m) 13)
                                      (string=? (substring m 0 13) "out of memory")))))))
            (or (%host-debugger c) (raise c))
            (raise c)))
      (lambda () ((%host-fetch) request env ctx)))
     (retry () (again))
     (use-value (response) response))))
"#;

/// Install in a dev world what serves its requests (`DEV_SERVE`), with
/// `(%host-fetch)`, its `fetch` now, and `%host-debugger`, unset, in its
/// root module, where what is attached sets it; the procedure serving a
/// request, held here so that no definition of the deployment's replaces
/// it.
fn install_dev(vm: &mut Vm) -> Result<Value, Error> {
    techne_vm::procedures! { vm;
        #[vm]
        "(%host-fetch)" => |vm: &mut Vm| -> Result<Value, Error> {
            vm.get_global("fetch").filter(|f| vm.is_procedure(f)).ok_or_else(|| Error::new("no fetch procedure"))
        };
    }
    vm.eval_in(ROOT_MODULE, "<techne-host>", DEV_SERVE)
}

/// Run `instance` for a slice, with what is attached to it: whether it
/// has more to run at once.
fn slice(instance: &mut Instance) -> Result<bool, Error> {
    let more = instance.attached.as_mut().is_some_and(|a| a.poll(&mut instance.vm));
    let progress = instance.vm.run_tasks_for(Duration::from_millis(2))?;
    if let Some(a) = &mut instance.attached {
        a.reap(&mut instance.vm);
    }
    Ok(more || matches!(progress, Progress::OutOfTime))
}

/// Start `request`, numbered `id`, on `instance`: `(fetch request env
/// ctx)` in a lifetime of its own (through `SERVE`, with its `exchange`),
/// `fetch` as the world has it now (a dev world's may have been
/// redefined). The instance is handed back, started or not.
fn start(
    mut instance: Instance,
    request: &Data,
    exchange: &Rc<Exchange>,
    context: Rc<outbound::Context>,
    id: u64,
    limits: &Limits,
    env: &Data,
) -> (Instance, Result<(LifetimeId, TaskId), Error>) {
    let vm = &mut instance.vm;
    // What a dev world wrote between requests is no request's.
    let _ = vm.take_output();
    let serve = instance.serve.clone();
    let begun = (|| {
        // A dev world's `fetch` is looked up as the request runs, where a
        // debugger can see to it missing.
        let fetch = match serve {
            Some(serve) => serve,
            None => vm.get_global("fetch").filter(|f| vm.is_procedure(f)).ok_or_else(|| Error::new("no fetch procedure"))?,
        };
        let serve = vm.get_global_in(ROOT_MODULE, "%host-serve").ok_or_else(|| Error::new("no %host-serve"))?;
        let request = vm.from_data(request)?;
        let x = vm.make_foreign(exchange.clone(), "exchange")?;
        let reads = vm.from_data(&Data::Bool(exchange.reader.borrow().is_some()))?;
        let env = vm.from_data(env)?;
        let ctx = vm.from_data(&Data::Dict(vec![(Data::Symbol("id".into()), Data::Int(id as i64))]))?;
        let lifetime = vm.new_lifetime(LifetimeLimits { cpu: Some(limits.cpu), tasks: Some(limits.tasks) });
        vm.set_lifetime_data(lifetime, context);
        match vm.spawn_in_with(lifetime, &serve, &[fetch, request, x, reads, env, ctx]) {
            Ok(task) => Ok((lifetime, task)),
            Err(e) => {
                vm.end_lifetime(lifetime);
                Err(e)
            }
        }
    })();
    (instance, begun)
}
