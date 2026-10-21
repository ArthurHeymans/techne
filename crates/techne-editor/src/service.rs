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
//! world reads no more than that one may (`Vm::confine_reading`). Loading
//! its file is bounded by the deadline of a request, and stopped as one is
//! when the service is dropped. A
//! service queues `QUEUE` requests at most besides the one it runs; past
//! that, a call fails at once ("busy"). A request runs at most `DEADLINE`. A service whose world
//! panicked or whose thread ended is started again for the next request,
//! from its file, the requests it had failing.

use std::{
    cell::RefCell,
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

use techne_text::ropey::Rope;
use techne_vm::{
    api::{Foreign, IntoValue, Value},
    data::Data,
    vm::{Error, ExecId, Grants, InterruptHandle, Stop, Vm},
};
use tokio::sync::oneshot;

/// The requests a service queues, besides the one it runs.
pub const QUEUE: usize = 16;

/// How long a request, or loading the service's file, may run before it
/// is killed, unless the service says otherwise.
pub const DEADLINE: Duration = Duration::from_secs(10);

/// The most bytes an argument or an answer makes, copied as data.
pub const DATA_BYTES: usize = 64 << 20;

/// A text as it was at a revision, for a service to read.
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// The text's id in the asking world (`document-id`).
    pub id: u64,
    pub revision: u64,
    pub text: Rope,
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
    /// The request running, as the execution running it.
    now: Option<(u64, ExecId)>,
    /// Stops what the service runs.
    stop: Option<InterruptHandle>,
}

/// A service: its file, and its thread while it runs.
pub struct Service {
    name: String,
    file: PathBuf,
    /// The directory its world may read in, if confined.
    reading: Option<PathBuf>,
    /// How long loading its file, or a request, may run.
    deadline: Duration,
    queue: RefCell<Option<mpsc::SyncSender<Request>>>,
    running: Arc<Mutex<Running>>,
    next: std::cell::Cell<u64>,
    /// Times it was started.
    starts: std::cell::Cell<u64>,
}

impl Service {
    /// A service whose world loads `file`, started at its first request,
    /// reading only in `reading` if given, each request and loading the
    /// file killed past `deadline`.
    pub fn new(name: &str, file: PathBuf, reading: Option<PathBuf>, deadline: Duration) -> Service {
        Service {
            name: name.to_string(),
            file,
            reading,
            deadline,
            queue: RefCell::new(None),
            running: Arc::default(),
            next: std::cell::Cell::new(1),
            starts: std::cell::Cell::new(0),
        }
    }

    /// Ask the service to call `name` with `argument` and the snapshots
    /// `texts`. The answer comes on the receiver; dropping the `Cancel`
    /// returned with it before then cancels the request.
    pub fn call(
        &self,
        name: &str,
        argument: Data,
        texts: Vec<Snapshot>,
    ) -> Result<(oneshot::Receiver<Result<Data, String>>, Cancel), String> {
        let id = self.next.replace(self.next.get() + 1);
        let (reply, answer) = oneshot::channel();
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut request = Request { name: name.to_string(), argument, texts, reply, id, cancelled: cancelled.clone() };
        // Once more if its thread ended: it is started again.
        for _ in 0..2 {
            let queue = self.queue.borrow_mut().get_or_insert_with(|| self.start()).clone();
            match queue.try_send(request) {
                Ok(()) => return Ok((answer, Cancel { id, cancelled, running: self.running.clone(), done: false })),
                Err(mpsc::TrySendError::Full(_)) => return Err(format!("{}: busy", self.name)),
                Err(mpsc::TrySendError::Disconnected(r)) => {
                    request = r;
                    self.queue.borrow_mut().take();
                }
            }
        }
        Err(format!("{}: could not be started", self.name))
    }

    /// Times its world was started.
    pub fn starts(&self) -> u64 {
        self.starts.get()
    }

    fn start(&self) -> mpsc::SyncSender<Request> {
        self.starts.set(self.starts.get() + 1);
        let (tx, rx) = mpsc::sync_channel(QUEUE);
        let (file, reading, running, deadline) = (self.file.clone(), self.reading.clone(), self.running.clone(), self.deadline);
        let _ = std::thread::Builder::new()
            .name(format!("service {}", self.name))
            .spawn(move || serve(&file, reading.as_deref(), deadline, &rx, &running));
        tx
    }
}

impl Drop for Service {
    /// Its queue closes, and what its world runs, a request or loading its
    /// file, is killed: the thread then ends.
    fn drop(&mut self) {
        self.queue.get_mut().take();
        let r = self.running.lock().expect("the service");
        if let (Some((_, exec)), Some(stop)) = (r.now, &r.stop) {
            stop.stop(exec, Stop::Kill);
        }
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
    requests: &mpsc::Receiver<Request>,
    running: &Mutex<Running>,
) {
    let mut vm = Vm::with_grants(Grants::NONE.with(techne_vm::vm::Capability::Loading));
    install_snapshots(&mut vm);
    let watch = crate::deadline::Watch::new(vm.interrupt_handle());
    running.lock().expect("the service").stop = Some(vm.interrupt_handle());
    // Loading its file is an execution as a request is: killed past the
    // deadline, or when the service is dropped (request 0).
    let exec = vm.new_execution();
    running.lock().expect("the service").now = Some((0, exec));
    watch.arm(exec, Instant::now() + deadline);
    vm.enter_execution(exec);
    let loaded = reading
        .map_or(Ok(()), |dir| vm.confine_reading(dir))
        .and_then(|()| vm.eval_source(&format!("(require {:?})", file.display().to_string())).map(drop));
    vm.leave_execution(exec);
    running.lock().expect("the service").now = None;
    if let Err(e) = loaded {
        requests.iter().for_each(|r| drop(r.reply.send(Err(format!("the service did not start: {e}")))));
        return;
    }
    for r in requests {
        let exec = vm.new_execution();
        {
            let mut running = running.lock().expect("the service");
            if r.cancelled.load(std::sync::atomic::Ordering::Acquire) || r.reply.is_closed() {
                vm.discard_execution(exec);
                continue;
            }
            running.now = Some((r.id, exec));
        }
        watch.arm(exec, Instant::now() + deadline);
        vm.enter_execution(exec);
        let answer = answer(&mut vm, &r);
        vm.leave_execution(exec);
        running.lock().expect("the service").now = None;
        let _ = r.reply.send(answer.map_err(|e| e.to_string()));
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
    }
}

/// `make-service`, over its native.
const MAKE: &str = r#"
(define (make-service name file [deadline #f])
  "Return a service named NAME, its world made from the Lisp file FILE.
The world is of its own, on a thread of its own, started when first
asked. Making it needs this world's loading capability, and its world
reads no more than this one may. Loading FILE, and each request, is
killed past DEADLINE milliseconds, ten seconds if not given."
  (%make-service name file deadline))
"#;

/// The text natives of the editor's world that ask services.
pub(crate) fn install(vm: &mut Vm) {
    vm.name_foreign_type::<Service>("service");
    techne_vm::procedures! { vm;
        // A service named NAME made from FILE, its requests killed past
        // DEADLINE milliseconds, or #f (`make-service`).
        #[vm]
        "(%make-service name file deadline)" => |vm: &mut Vm, name: String, file: String, deadline: Option<usize>| -> Result<Foreign<Service>, Error> {
            vm.need(techne_vm::vm::Capability::Loading, "make-service")?;
            let path = PathBuf::from(&file);
            if !vm.may_read(&path) {
                return Err(Error::new(format!("make-service: {file} may not be read here")));
            }
            let deadline = deadline.map_or(DEADLINE, |ms| Duration::from_millis(ms as u64));
            Ok(Foreign::new(Service::new(&name, path, vm.reading_root().map(PathBuf::from), deadline)))
        };
        /// Return how many times SERVICE's world was started.
        "(service-starts service)" => |s: Foreign<Service>| s.0.starts() as i64;
    }
    vm.eval_in(techne_vm::vm::ROOT_MODULE, "<techne-editor service>", MAKE).expect("the service procedures");
    // Ask SERVICE to call its procedure NAME with ARGUMENT, data, and
    // snapshots of TEXTS, documents: (NAME argument snapshots) there.
    // Waits in the task asking for its answer, data.
    vm.register_async("service-call", 4, |vm, args| {
        let asked = (|| {
            let service = args.get::<Foreign<Service>>(vm, 0)?;
            let name = args.get::<String>(vm, 1)?;
            let argument = args.get::<Value>(vm, 2)?;
            let argument = vm.to_data(&argument, DATA_BYTES)?;
            let texts = args.get::<Vec<crate::TextArg>>(vm, 3)?;
            let texts = texts.iter().map(|t| Snapshot { id: t.0.id(), revision: t.0.revision(), text: t.0.rope().clone() }).collect();
            service.0.call(&name, argument, texts).map_err(Error::new)
        })();
        async move {
            let (answer, cancel) = asked?;
            let answer = answer.await;
            cancel.done();
            answer.map_err(|_| Error::new("the service ended"))?.map_err(Error::new)
        }
    });
}
