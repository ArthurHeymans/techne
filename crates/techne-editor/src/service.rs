//! Services: worlds of their own on threads of their own, which the
//! editor's world asks for work (PLAN.md, Stage 1, step 8.6; an
//! experiment, `examples/sibling.rs` measuring it). A service shares no
//! values with the editor: a request is a procedure's name, its argument
//! as data (`techne_vm::data::Data`) and snapshots of texts; the answer is
//! data. A snapshot is a document's text at a revision, shared, not
//! copied (a rope shares its chunks), so asking about a large document
//! costs the same as asking about a small one. What the answer says of a
//! document is as of the revision its snapshot had: the asker compares
//! it with the document's now, and drops a stale answer.
//!
//! Lisp in the editor's world asks with `(service-call service name
//! argument texts)`, which waits in the asking task without holding up
//! the editor. Killing that task (or a newer request replacing it) cancels
//! the request: one still queued is dropped, one running is killed. Making
//! a service needs the loading capability of the world making it, and its
//! world reads no more than that one may (`Vm::confine_reading`). A
//! service given files (the files capability, which the world making it
//! must have) can list and search them (`search.rs`), and write them as
//! that world may. Loading
//! its file is bounded by the deadline of a request, and stopped as one is
//! when the service is dropped. A
//! service queues `QUEUE` requests at most besides the one it runs; past
//! that, a call fails at once ("busy"). A request runs at most `DEADLINE`,
//! and its world holds `MEMORY` at most, unless the service says
//! otherwise. A service whose world panicked or whose thread ended, or
//! that was stopped (`service-stop!`), is started again for the next
//! request, from its file, the requests it had failing. The services made
//! are listed (`services`), each telling what it is (`service-info`), and
//! are stopped, restarted and closed (`service-close!`), as shutting the
//! scope that made one does. A document may be
//! granted to it (`access.rs`): its snapshot then names the grant, through
//! which it proposes changes, until its world ends.

use std::{
    cell::RefCell,
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use techne_text::ropey::Rope;
use techne_vm::{
    api::{Foreign, IntoValue, Value},
    data::Data,
    vm::{Capability, Error, ExecId, Grants, InterruptHandle, Stop, Vm},
};
use tokio::sync::oneshot;

/// The requests a service queues, besides the one it runs.
pub const QUEUE: usize = 16;

/// How long a request, or loading the service's file, may run before it
/// is killed, unless the service says otherwise.
pub const DEADLINE: Duration = Duration::from_secs(10);

/// The most bytes an argument or an answer makes, copied as data.
pub const DATA_BYTES: usize = 64 << 20;

/// The most memory a service's world holds, unless the service says
/// otherwise (`Vm::set_memory_limit`).
pub const MEMORY: usize = 512 << 20;

/// A text as it was at a revision, for a service to read.
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// The text's id in the asking world (`document-id`).
    pub id: u64,
    pub revision: u64,
    pub text: Rope,
    /// The grant it was given through, if any (`access.rs`): what the
    /// service names to propose changes of the text.
    pub grant: Option<u64>,
}

struct Request {
    name: String,
    argument: Data,
    texts: Vec<Snapshot>,
    reply: oneshot::Sender<Result<Data, String>>,
    id: u64,
    /// Set when the asker no longer waits, under `Running`'s lock: the
    /// service skips the request if it has not started it.
    cancelled: Arc<std::sync::atomic::AtomicBool>,
}

/// What the service's thread shares with the asking world.
#[derive(Default)]
struct Running {
    /// The request running, as the execution running it, and since when.
    now: Option<(u64, ExecId)>,
    since: Option<Instant>,
    /// Stops what the service runs.
    stop: Option<InterruptHandle>,
    /// What its world held after its last request (`Vm::held`).
    held: usize,
    /// How its last request failed, if it did.
    failed: Option<String>,
    /// Why its world ended, if it did by itself.
    ended: Option<String>,
    /// Stopped: what it has queued fails, and it ends.
    stopped: bool,
    /// Its file did not load: it answers each request so.
    unloaded: bool,
}

/// A service: its file, and its thread while it runs.
pub struct Service {
    /// Never another service's, also once this one is gone.
    id: u64,
    name: String,
    file: PathBuf,
    /// The directory its world may read in, if confined.
    reading: Option<PathBuf>,
    /// How long loading its file, or a request, may run.
    deadline: Duration,
    /// Whether its world has the files capability.
    files: bool,
    /// The most memory its world holds.
    memory: usize,
    queue: RefCell<Option<mpsc::SyncSender<Request>>>,
    /// What its world's thread shares, the world started last's.
    running: RefCell<Arc<Mutex<Running>>>,
    next: std::cell::Cell<u64>,
    /// Times it was started.
    starts: std::cell::Cell<u64>,
    /// Times its world ended (it panicked, or was stopped): what was
    /// granted to an earlier world is not the next one's. Its world's
    /// thread moves it on as it ends.
    generation: Arc<AtomicU64>,
    /// The generation of the world started last.
    world: std::cell::Cell<u64>,
    /// Why its last world ended, if one did.
    ended: RefCell<Option<String>>,
    /// Closed (`close`): it starts no world again.
    closed: std::cell::Cell<bool>,
}

impl Service {
    /// A service whose world loads `file`, started at its first request,
    /// reading only in `reading` if given, each request and loading the
    /// file killed past `deadline`, with the files capability if `files`,
    /// holding `memory` bytes at most.
    pub fn new(name: &str, file: PathBuf, reading: Option<PathBuf>, deadline: Duration, files: bool, memory: usize) -> Service {
        Service {
            id: techne_text::document::fresh_id(),
            name: name.to_string(),
            file,
            reading,
            deadline,
            files,
            memory,
            queue: RefCell::new(None),
            running: RefCell::default(),
            next: std::cell::Cell::new(1),
            starts: std::cell::Cell::new(0),
            generation: Arc::default(),
            world: std::cell::Cell::new(0),
            ended: RefCell::default(),
            closed: std::cell::Cell::new(false),
        }
    }

    /// Identifies it: what grants are made to (`access.rs`).
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    /// Times its world ended, as it goes on: a grant holds while this is
    /// what it was when the grant was made.
    pub(crate) fn generation(&self) -> Arc<AtomicU64> {
        self.generation.clone()
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Ask the service to call `name` with `argument` and the snapshots
    /// `texts`. The answer comes on the receiver; dropping the `Cancel`
    /// returned with it before then cancels the request.
    /// `granted`: the generation the grants of the snapshots were checked
    /// against, if any: the request goes to that generation's world only.
    pub fn call(
        &self,
        name: &str,
        argument: Data,
        texts: Vec<Snapshot>,
        granted: Option<u64>,
    ) -> Result<(oneshot::Receiver<Result<Data, String>>, Cancel), String> {
        if self.closed.get() {
            return Err(format!("{}: closed", self.name));
        }
        let id = self.next.replace(self.next.get() + 1);
        let (reply, answer) = oneshot::channel();
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut request = Request { name: name.to_string(), argument, texts, reply, id, cancelled: cancelled.clone() };
        // Once more if its thread ended: it is started again.
        for _ in 0..2 {
            let queue = self.queue.borrow_mut().get_or_insert_with(|| self.start()).clone();
            if granted.is_some_and(|g| g != self.world.get()) {
                return Err(format!("{}: the grant is revoked", self.name));
            }
            match queue.try_send(request) {
                Ok(()) => return Ok((answer, Cancel { id, cancelled, running: self.running.borrow().clone(), done: false })),
                Err(mpsc::TrySendError::Full(_)) => return Err(format!("{}: busy", self.name)),
                Err(mpsc::TrySendError::Disconnected(r)) => {
                    // Its thread ended, and moved the generation on.
                    request = r;
                    self.queue.borrow_mut().take();
                    let why = self.running.borrow().lock().expect("the service").ended.clone();
                    *self.ended.borrow_mut() = Some(why.unwrap_or_else(|| "its thread ended".into()));
                }
            }
        }
        Err(format!("{}: could not be started", self.name))
    }

    /// Times its world was started.
    pub fn starts(&self) -> u64 {
        self.starts.get()
    }

    /// Stop its world: its queue closes, and what it runs is killed. It is
    /// started again, a new world, at the next request; what was granted
    /// to this one no longer holds.
    pub fn stop(&self) {
        if self.queue.borrow_mut().take().is_some() {
            self.kill_running();
            end(&self.generation, self.world.get());
            *self.ended.borrow_mut() = Some("stopped".into());
        }
    }

    /// Stop its world, and start another now.
    pub fn restart(&self) -> Result<(), String> {
        if self.closed.get() {
            return Err(format!("{}: closed", self.name));
        }
        self.stop();
        *self.queue.borrow_mut() = Some(self.start());
        Ok(())
    }

    /// Stop its world for good: a request fails from now on. What the
    /// scope that made it does when it is shut.
    pub fn close(&self) {
        self.closed.set(true);
        self.stop();
    }

    /// Whether it was closed.
    pub fn closed(&self) -> bool {
        self.closed.get()
    }

    /// What it is, as data: its name; its state (starting its world, idle,
    /// running a request, failed to load its file, ended by itself or
    /// stopped, started again at the next request, or closed); its starts
    /// and how many worlds ended; how long the request running has run;
    /// what its world held after its last request and may hold; its
    /// deadline; how its last request failed and why its last world
    /// ended, if they did. `grants`: how many grants it holds.
    pub(crate) fn info(&self, grants: usize) -> Data {
        let running = self.running.borrow();
        let r = running.lock().expect("the service");
        let state = if self.closed.get() {
            "closed"
        } else if self.queue.borrow().is_none() && self.starts.get() > 0 {
            "stopped"
        } else if r.ended.is_some() {
            "ended"
        } else if r.unloaded {
            "failed"
        } else if r.now.is_some_and(|(id, _)| id == 0) {
            "starting"
        } else if r.now.is_some() {
            "running"
        } else {
            "idle"
        };
        let int = |n: u64| Data::Int(i64::try_from(n).unwrap_or(i64::MAX));
        let text = |t: &Option<String>| t.clone().map_or(Data::Bool(false), Data::String);
        let field = |k: &str, v: Data| (Data::Symbol(k.into()), v);
        Data::Dict(vec![
            field("name", Data::String(self.name.clone())),
            field("state", Data::Symbol(state.into())),
            field("starts", int(self.starts.get())),
            field("ended", int(self.generation.load(Ordering::SeqCst))),
            field("running-ms", r.since.filter(|_| r.now.is_some()).map_or(Data::Bool(false), |s| int(s.elapsed().as_millis() as u64))),
            field("held", int(r.held as u64)),
            field("memory", int(self.memory as u64)),
            field("deadline-ms", int(self.deadline.as_millis() as u64)),
            field("grants", int(grants as u64)),
            field("last-failure", text(&r.failed)),
            field("last-end", text(&self.ended.borrow())),
        ])
    }

    fn kill_running(&self) {
        let running = self.running.borrow();
        let mut r = running.lock().expect("the service");
        r.stopped = true;
        if let (Some((_, exec)), Some(stop)) = (r.now, &r.stop) {
            stop.stop(exec, Stop::Kill);
        }
    }

    fn start(&self) -> mpsc::SyncSender<Request> {
        self.starts.set(self.starts.get() + 1);
        let world = self.generation.load(Ordering::SeqCst);
        self.world.set(world);
        let generation = self.generation.clone();
        let (tx, rx) = mpsc::sync_channel(QUEUE);
        // A thread of its own, which an earlier world's, ending, does not
        // share.
        let running = Arc::<Mutex<Running>>::default();
        *self.running.borrow_mut() = running.clone();
        let (file, reading, deadline, files, memory) = (self.file.clone(), self.reading.clone(), self.deadline, self.files, self.memory);
        let _ = std::thread::Builder::new().name(format!("service {}", self.name)).spawn(move || {
            // However it ends, the next world is another generation.
            let _ended = Ended(generation, world);
            serve(&file, reading.as_deref(), deadline, files, memory, &rx, &running);
        });
        tx
    }
}

/// The world of generation `world` ended: the next is another, unless that
/// was told already.
fn end(generation: &AtomicU64, world: u64) {
    let _ = generation.compare_exchange(world, world + 1, Ordering::SeqCst, Ordering::SeqCst);
}

/// Ends a world's generation when dropped, as its thread ends.
struct Ended(Arc<AtomicU64>, u64);

impl Drop for Ended {
    fn drop(&mut self) {
        end(&self.0, self.1);
    }
}

impl Drop for Service {
    /// Its queue closes, and what its world runs, a request or loading its
    /// file, is killed: the thread then ends.
    fn drop(&mut self) {
        self.queue.get_mut().take();
        self.kill_running();
    }
}

/// Cancels its request when dropped before `done`.
pub struct Cancel {
    id: u64,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
    running: Arc<Mutex<Running>>,
    done: bool,
}

impl Cancel {
    fn done(mut self) {
        self.done = true;
    }
}

impl Drop for Cancel {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        // Queued, it is skipped; running, it is killed. Under the lock the
        // service takes to start a request: it has or will see this.
        let r = self.running.lock().expect("the service");
        self.cancelled.store(true, std::sync::atomic::Ordering::Release);
        if let (Some((id, exec)), Some(stop)) = (r.now, &r.stop)
            && id == self.id
        {
            stop.stop(exec, Stop::Kill);
        }
    }
}

/// The service's world: made from `file`, answering requests until the
/// asking side is gone, or its world panicked.
fn serve(
    file: &std::path::Path,
    reading: Option<&std::path::Path>,
    deadline: Duration,
    files: bool,
    memory: usize,
    requests: &mpsc::Receiver<Request>,
    running: &Mutex<Running>,
) {
    let grants = Grants::NONE.with(Capability::Loading);
    let mut vm = Vm::with_grants(if files { grants.with(Capability::Files) } else { grants });
    vm.set_memory_limit(memory);
    install_snapshots(&mut vm);
    crate::search::install(&mut vm);
    // For the tests: a world that panics, and ends.
    #[cfg(test)]
    vm.register_fn("%service-panic", || -> i64 { panic!("a test's panic") });
    let watch = crate::deadline::Watch::new(vm.interrupt_handle());
    running.lock().expect("the service").stop = Some(vm.interrupt_handle());
    // Loading its file is an execution as a request is: killed past the
    // deadline, or when the service is dropped (request 0).
    let exec = vm.new_execution();
    {
        // Stopped before it began, it loads nothing.
        let mut running = running.lock().expect("the service");
        if running.stopped {
            return;
        }
        (running.now, running.since) = (Some((0, exec)), Some(Instant::now()));
    }
    watch.arm(exec, Instant::now() + deadline);
    vm.enter_execution(exec);
    let loaded = reading
        .map_or(Ok(()), |dir| vm.confine_reading(dir))
        .and_then(|()| vm.eval_source(&format!("(require {:?})", file.display().to_string())).map(drop));
    vm.leave_execution(exec);
    {
        let mut running = running.lock().expect("the service");
        (running.now, running.held) = (None, vm.held().total());
        running.failed = loaded.as_ref().err().map(|e| format!("did not start: {e}"));
        running.unloaded = loaded.is_err();
    }
    if let Err(e) = loaded {
        requests.iter().for_each(|r| drop(r.reply.send(Err(format!("the service did not start: {e}")))));
        return;
    }
    for r in requests {
        let exec = vm.new_execution();
        {
            let mut running = running.lock().expect("the service");
            if running.stopped {
                let _ = r.reply.send(Err("the service was stopped".into()));
                vm.discard_execution(exec);
                continue;
            }
            if r.cancelled.load(std::sync::atomic::Ordering::Acquire) || r.reply.is_closed() {
                vm.discard_execution(exec);
                continue;
            }
            (running.now, running.since) = (Some((r.id, exec)), Some(Instant::now()));
        }
        watch.arm(exec, Instant::now() + deadline);
        vm.enter_execution(exec);
        let answer = answer(&mut vm, &r);
        vm.leave_execution(exec);
        let answer = answer.map_err(|e| e.to_string());
        {
            let mut running = running.lock().expect("the service");
            (running.now, running.held) = (None, vm.held().total());
            running.failed = answer.as_ref().err().map(|e| format!("{}: {e}", r.name));
            if let Some(why) = vm.poisoned() {
                running.ended = Some(format!("its world was retired: {why}"));
            }
        }
        let _ = r.reply.send(answer);
        if vm.poisoned().is_some() {
            return;
        }
    }
}

fn answer(vm: &mut Vm, r: &Request) -> Result<Data, Error> {
    let f = vm.get_global(&r.name).ok_or_else(|| Error::new(format!("{} is not defined", r.name)))?;
    let argument = vm.from_data(&r.argument)?;
    let texts = r.texts.iter().cloned().map(Foreign::new).collect::<Vec<_>>().into_value(vm)?;
    let v = vm.call(&f, &[argument, texts])?;
    vm.to_data(&v, DATA_BYTES)
}

/// What a service reads of the snapshots it is given.
fn install_snapshots(vm: &mut Vm) {
    vm.name_foreign_type::<Snapshot>("snapshot");
    techne_vm::procedures! { vm;
        /// Return the id SNAPSHOT's text has in the asking world.
        "(snapshot-id snapshot)" => |s: Foreign<Snapshot>| s.0.id as i64;
        /// Return the revision SNAPSHOT is the text at.
        "(snapshot-revision snapshot)" => |s: Foreign<Snapshot>| s.0.revision as i64;
        /// Return the length of SNAPSHOT's text in bytes.
        "(snapshot-length snapshot)" => |s: Foreign<Snapshot>| s.0.text.len_bytes();
        /// Return SNAPSHOT's text.
        "(snapshot-string snapshot)" => |s: Foreign<Snapshot>| s.0.text.to_string();
        /// Return the grant SNAPSHOT was given through, a number to name in
        /// a proposal of changes to its text, or #f if none.
        "(snapshot-grant snapshot)" => |s: Foreign<Snapshot>| s.0.grant.map(|g| g as i64);
    }
}

/// `make-service`, over its native.
const MAKE: &str = r#"
(define (make-service name file [deadline #f] #:files [files #f] #:memory [memory #f])
  "Return a service named NAME, its world made from the Lisp file FILE.
The world is of its own, on a thread of its own, started when first
asked. Making it needs this world's loading capability, and its world
reads no more than this one may. Loading FILE, and each request, is
killed past DEADLINE milliseconds, ten seconds if not given. With
FILES, its world has the files capability too, which this one must
have, reading and writing as this one may: it can list and search
files (`project-files`, `search-file`). Its world holds MEMORY
megabytes at most, 512 if not given: past that, what it runs is
refused, as in any world. The current scope owns it: shutting the scope
closes it, as `service-close!` does."
  (scope-own! (%make-service name file deadline files memory) service-close! service-closed?))
"#;

/// The text natives of the editor's world that ask services.
pub(crate) fn install(vm: &mut Vm, access: crate::access::Shared) {
    vm.name_foreign_type::<Service>("service");
    // The services made, while they live: for `services`.
    let made: Rc<RefCell<Vec<std::rc::Weak<Service>>>> = Rc::default();
    let registry = made.clone();
    techne_vm::procedures! { vm;
        // A service named NAME made from FILE, its requests killed past
        // DEADLINE milliseconds, or #f, its world holding MEMORY
        // megabytes, or #f (`make-service`).
        #[vm]
        "(%make-service name file deadline files memory)" => move |vm: &mut Vm, name: String, file: String, deadline: Option<usize>, files: bool, memory: Option<usize>| -> Result<Foreign<Service>, Error> {
            vm.need(Capability::Loading, "make-service")?;
            if files {
                vm.need(Capability::Files, "make-service")?;
            }
            let path = PathBuf::from(&file);
            if !vm.may_read(&path) {
                return Err(Error::new(format!("make-service: {file} may not be read here")));
            }
            let deadline = deadline.map_or(DEADLINE, |ms| Duration::from_millis(ms as u64));
            let memory = match memory {
                None => MEMORY,
                Some(mb) => mb.checked_mul(1 << 20).filter(|&b| b > 0).ok_or_else(|| Error::new(format!("make-service: {mb} megabytes is no memory limit")))?,
            };
            let service = Foreign::new(Service::new(&name, path, vm.reading_root().map(PathBuf::from), deadline, files, memory));
            let mut made = registry.borrow_mut();
            made.retain(|s| s.strong_count() > 0);
            made.push(Rc::downgrade(&service.0));
            Ok(service)
        };
        /// Return how many times SERVICE's world was started.
        "(service-starts service)" => |s: Foreign<Service>| s.0.starts() as i64;
        /// Return SERVICE's name.
        "(service-name service)" => |s: Foreign<Service>| s.0.name().to_string();
        /// Stop SERVICE's world: what it runs is killed, and its queued
        /// requests fail. It starts again, a new world, at the next
        /// request; documents granted to the stopped one are not the new
        /// one's.
        "(service-stop! service)" => |s: Foreign<Service>| s.0.stop();
        /// Stop SERVICE's world, and start another now.
        "(service-restart! service)" => |s: Foreign<Service>| s.0.restart();
        /// Close SERVICE: its world is stopped, and requests fail from now
        /// on. What shutting the scope that made it does.
        "(service-close! service)" => |s: Foreign<Service>| s.0.close();
        /// Return #t if SERVICE was closed.
        "(service-closed? service)" => |s: Foreign<Service>| s.0.closed();
    }
    let a = access.clone();
    techne_vm::procedures! { vm;
        /// Return what SERVICE is, a dict: its name; its state (starting,
        /// idle, running a request, failed to load its file, ended by
        /// itself, stopped or closed); how many
        /// times its world started and ended; how long the request running
        /// has run (running-ms); the bytes its world held after its last
        /// request (held), and may hold (memory); its deadline; the grants
        /// it holds; and how its last request failed and why its last
        /// world ended, if they did.
        "(service-info service)" => move |s: Foreign<Service>| s.0.info(a.borrow().grants_of(&s.0));
    }
    techne_vm::procedures! { vm;
        /// Return the services made, but closed ones, oldest first.
        "(services)" => move || -> Vec<Foreign<Service>> {
            made.borrow().iter().filter_map(std::rc::Weak::upgrade).filter(|s| !s.closed.get()).map(Foreign).collect()
        };
    }
    vm.eval_in(techne_vm::vm::ROOT_MODULE, "<techne-editor service>", MAKE).expect("the service procedures");
    // Ask SERVICE to call its procedure NAME with ARGUMENT, data, and
    // snapshots of TEXTS, documents: (NAME argument snapshots) there.
    // Waits in the task asking for its answer, data.
    // TEXTS may hold grants of documents (`document-grant`) made to SERVICE:
    // the snapshot of such a document names its grant.
    vm.register_async("service-call", 4, move |vm, args| {
        let asked = (|| {
            let service = args.get::<Foreign<Service>>(vm, 0)?;
            let name = args.get::<String>(vm, 1)?;
            let argument = args.get::<Value>(vm, 2)?;
            let argument = vm.to_data(&argument, DATA_BYTES)?;
            let texts = args.get::<Vec<crate::access::Given>>(vm, 3)?;
            let texts: Vec<Snapshot> = texts
                .iter()
                .map(|t| match t {
                    crate::access::Given::Text(t) => {
                        Ok(Snapshot { id: t.0.id(), revision: t.0.revision(), text: t.0.rope().clone(), grant: None })
                    }
                    crate::access::Given::Grant(g) => {
                        access.borrow().snapshot(*g, &service.0).map_err(|e| Error::new(format!("service-call: {e}")))
                    }
                })
                .collect::<Result<_, Error>>()?;
            // Checked against this generation: for its world alone.
            let granted = texts.iter().any(|t| t.grant.is_some()).then(|| service.0.generation().load(Ordering::SeqCst));
            service.0.call(&name, argument, texts, granted).map_err(Error::new)
        })();
        async move {
            let (answer, cancel) = asked?;
            let answer = answer.await;
            cancel.done();
            answer.map_err(|_| Error::new("the service ended"))?.map_err(Error::new)
        }
    });
}

#[cfg(test)]
mod tests {
    use crate::runtime::Runtime;

    /// A world that ends by itself (here, panicking) holds no grant from
    /// then on: what it proposed is revoked at once, and a request through
    /// its grant is refused rather than handed to the world started next.
    #[test]
    fn a_world_ending_by_itself_holds_no_grant() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("svc.scm");
        std::fs::write(&file, "(define (propose edits ss) (let ((s (car ss))) (list (snapshot-grant s) (snapshot-revision s) edits)))\n(define (die a s) (%service-panic))\n").unwrap();
        let mut rt = Runtime::with_document(techne_text::Document::new(""), "emacs").unwrap();
        let until = |rt: &mut Runtime, done: &str| {
            let start = std::time::Instant::now();
            while rt.eval(done).unwrap() != "#t" {
                assert!(start.elapsed() < std::time::Duration::from_secs(20), "{done}");
                rt.run_tasks(std::time::Duration::from_millis(5));
            }
        };
        rt.eval(&format!(
            "(define doc (make-document \"alpha beta\"))
             (define svc (make-service \"svc\" {:?}))
             (define grant (document-grant doc 'stage #:to svc))
             (define (ask name argument)
               (guard (e (#t (condition/report-string e))) (service-call svc name argument (list grant))))
             (define p #f)
             (spawn (lambda () (let ((a (ask \"propose\" '((0 5 \"ALPHA\"))))) (set! p (proposal-file! svc (car a) (cadr a) (caddr a) \"\")))))",
            file.display().to_string()
        ))
        .unwrap();
        until(&mut rt, "(and p #t)");
        assert_eq!(rt.eval("(proposal-state p)").unwrap(), "\"pending\"");
        rt.eval("(define died #f) (spawn (lambda () (set! died (ask \"die\" 0))))").unwrap();
        until(&mut rt, "(and died #t)");
        // Its thread ends as it returns the panic's error: told at once.
        until(&mut rt, "(equal? (proposal-state p) \"revoked\")");
        rt.eval("(define again #f) (spawn (lambda () (set! again (ask \"propose\" '((6 10 \"BETA\"))))))").unwrap();
        until(&mut rt, "(and again #t)");
        assert!(rt.eval("again").unwrap().contains("revoked"), "{}", rt.eval("again").unwrap());
        assert_eq!(rt.eval("(service-starts svc)").unwrap(), "1", "no world was started for it");
    }
}
