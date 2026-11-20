//! `techne web`: a browser attached to the editor running here.
//!
//! The gateway is Scheme (`lisp/web/gateway.scm`): HTTP, the WebSocket,
//! who may attach, which inputs a browser may send, the JSON it is shown
//! and the budgets that bound it. What is here is what Scheme cannot do:
//! attaching through the editor's private socket, whose frames are the
//! editor's own Rust protocol (`attach::Remote`), and handing each
//! presentation to Scheme. Its events come from the attachment's thread
//! through a bounded queue: one that overflows ends the session rather
//! than block that thread or grow without bound.

use std::{
    cell::RefCell,
    collections::VecDeque,
    path::PathBuf,
    process::ExitCode,
    sync::{Arc, Mutex},
};

use techne_editor::{
    attach::Remote,
    host::Event,
    present::{Input, Output, Snapshot},
    server::{self, OpenError},
};
use techne_vm::{
    api::{Args, Foreign, IntoValue, Value},
    vm::{Error, Vm},
};
use tokio::sync::Notify;

/// Events waiting for Scheme, at most: past it the browser has fallen
/// behind the editor.
const QUEUED: usize = 16;

const USAGE: &str = "usage: techne web [FILE]

Attach a browser to a running editor (start one with techne daemon).
Open the printed private URL. The gateway listens only on IPv4 loopback.";

pub fn main(args: Vec<String>) -> ExitCode {
    let file = match args.as_slice() {
        [] => None,
        [arg] if matches!(arg.as_str(), "-h" | "--help") => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        [arg] if !arg.starts_with('-') => Some(PathBuf::from(arg)),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(file) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("techne web: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(file: Option<PathBuf>) -> Result<(), String> {
    let socket = server::socket().ok_or("no private editor socket (set XDG_RUNTIME_DIR)")?;
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    rt.block_on(async {
        let mut vm = Vm::new();
        techne_net::install_on(&mut vm, tokio::runtime::Handle::current()).map_err(|e| e.msg.clone())?;
        install(&mut vm);
        let web = techne_vm::library::lisp_dir().join("web");
        let file = file.map(|f| std::path::absolute(&f).unwrap_or(f));
        vm.eval_source(&format!(
            "(require {:?}) (spawn (lambda () (web-serve {:?} {} {:?})))",
            web.join("gateway.scm").display().to_string(),
            socket.display().to_string(),
            file.map_or("#f".into(), |f| format!("{:?}", f.display().to_string())),
            web.display().to_string(),
        ))
        .map_err(|e| e.msg.clone())?;
        techne_net::run_tasks(&mut vm).await.map_err(|e| e.msg.clone())
    })
}

/// What the attachment's thread hands Scheme.
#[derive(Default)]
struct Queue {
    events: VecDeque<Event>,
    overflowed: bool,
    closed: bool,
}

/// An editor attachment, as Scheme holds it.
struct Attachment {
    remote: RefCell<Option<Remote>>,
    queue: Arc<Mutex<Queue>>,
    ready: Arc<Notify>,
}

impl Attachment {
    fn remote<T>(&self, who: &str, f: impl FnOnce(&mut Remote) -> T) -> Result<T, Error> {
        self.remote.borrow_mut().as_mut().map(f).ok_or_else(|| Error::new(format!("{who}: the attachment is closed")))
    }
}

/// An event for Scheme: `(snapshot . SNAPSHOT)`, `(clipboard . TEXT)`,
/// `(capture . ID)`, `(quit)`, `(failed . MESSAGE)`, `(refused . MESSAGE)`,
/// `(ended)` (the runtime restarts), `(overflow)`, or the eof object once
/// the attachment has closed.
enum Delivered {
    Event(Event),
    Overflow,
    Closed,
}

impl IntoValue for Delivered {
    fn into_value(self, vm: &mut Vm) -> Result<Value, Error> {
        let tagged = |vm: &mut Vm, tag: &str, rest: Option<Value>| match rest {
            Some(v) => vm.make_list(&[Value::symbol(tag), v]),
            None => vm.make_list(&[Value::symbol(tag)]),
        };
        match self {
            Delivered::Closed => Ok(Value::EOF),
            Delivered::Overflow => tagged(vm, "overflow", None),
            Delivered::Event(Event::Ended) => tagged(vm, "ended", None),
            Delivered::Event(Event::Failed(e)) => {
                let e = vm.make_string(&e)?;
                tagged(vm, "failed", Some(e))
            }
            Delivered::Event(Event::Output(output)) => match output {
                Output::Snapshot(s) => {
                    let s = Foreign::new(*s).into_value(vm)?;
                    tagged(vm, "snapshot", Some(s))
                }
                Output::Clipboard(text) => {
                    let text = vm.make_string(&text)?;
                    tagged(vm, "clipboard", Some(text))
                }
                Output::Refused(e) => {
                    let e = vm.make_string(&e)?;
                    tagged(vm, "refused", Some(e))
                }
                Output::Capture { id, .. } => {
                    let id = id.into_value(vm)?;
                    tagged(vm, "capture", Some(id))
                }
                Output::Quit => tagged(vm, "quit", None),
                // Not delivered: `next` skips them.
                Output::CancelCapture { .. } | Output::Bindings(_) | Output::Session(_) => tagged(vm, "other", None),
            },
        }
    }
}

/// Whether Scheme has a use for EVENT.
fn wanted(event: &Event) -> bool {
    !matches!(event, Event::Output(Output::CancelCapture { .. } | Output::Bindings(_) | Output::Session(_)))
}

async fn next(queue: Arc<Mutex<Queue>>, ready: Arc<Notify>) -> Result<Delivered, String> {
    loop {
        // Registered before looking, so a push in between still wakes it.
        let notified = ready.notified();
        {
            let mut q = queue.lock().expect("the queue");
            if q.overflowed {
                return Ok(Delivered::Overflow);
            }
            while let Some(event) = q.events.pop_front() {
                if wanted(&event) {
                    return Ok(Delivered::Event(event));
                }
            }
            if q.closed {
                return Ok(Delivered::Closed);
            }
        }
        notified.await;
    }
}

fn install(vm: &mut Vm) {
    vm.name_foreign_type::<Attachment>("editor-attachment");
    vm.name_foreign_type::<Snapshot>("editor-snapshot");
    techne_vm::procedures! { vm;
        /// Attach to the editor on SOCKET, its private socket, with
        /// PROFILE's keys, opening FILE (a string) unless it is #f. Return
        /// the attachment, or #f when no editor runs there.
        "(%editor-attach socket profile file)" => |socket: String, profile: String, file: Option<String>| -> Result<Option<Foreign<Attachment>>, Error> {
            let queue = Arc::new(Mutex::new(Queue::default()));
            let ready = Arc::new(Notify::new());
            let (q, r) = (queue.clone(), ready.clone());
            let deliver = move |event: Event| {
                let mut q = q.lock().expect("the queue");
                if q.events.len() >= QUEUED {
                    q.overflowed = true;
                } else {
                    q.events.push_back(event);
                }
                drop(q);
                r.notify_one();
            };
            let file = file.map(PathBuf::from);
            match Remote::attach(socket.as_ref(), &profile, file.as_deref(), deliver) {
                Ok(remote) => Ok(Some(Foreign::new(Attachment { remote: RefCell::new(Some(remote)), queue, ready }))),
                Err(OpenError::NotRunning) => Ok(None),
                Err(OpenError::Failed(e)) => Err(Error::new(format!("editor-attach: {e}"))),
            }
        };
    }
    vm.register_async("%editor-event", 1, |vm: &mut Vm, args: Args<'_>| {
        let a = args.get::<Foreign<Attachment>>(vm, 0).map(|a| (a.0.queue.clone(), a.0.ready.clone()));
        async move {
            let (queue, ready) = a.map_err(|e| e.msg.clone())?;
            next(queue, ready).await
        }
    });
    techne_vm::procedures! { vm;
        /// Send INPUT, the JSON of a frontend input as the editor's
        /// protocol has it (`{"Key": {"key": "a"}}`), for the runtime of
        /// GENERATION: the number of (ended) events seen before it.
        "(%editor-send! attachment input generation)" => |a: Foreign<Attachment>, input: String, generation: usize| -> Result<(), Error> {
            let input: Input = serde_json::from_str(&input).map_err(|e| Error::new(format!("editor-send!: {e}")))?;
            a.0.remote("editor-send!", |r| r.send_in_generation(input, generation as u64))
        };
    }
    techne_vm::procedures! { vm;
        /// Answer the capture ID with the failure MESSAGE.
        "(%editor-refuse-capture! attachment id message)" => |a: Foreign<Attachment>, id: usize, message: String| -> Result<(), Error> {
            a.0.remote("editor-refuse-capture!", |r| r.captured(id, Err(message)))
        };
    }
    techne_vm::procedures! { vm;
        /// Detach: the editor forgets this frontend's session.
        "(%editor-close! attachment)" => |a: Foreign<Attachment>| {
            if let Some(remote) = a.0.remote.borrow_mut().take() {
                remote.close();
            }
            let mut q = a.0.queue.lock().expect("the queue");
            q.closed = true;
            q.events.clear();
            drop(q);
            a.0.ready.notify_one();
        };
    }
    techne_vm::procedures! { vm;
        /// SNAPSHOT as JSON, as the editor's protocol has it, without its
        /// panes' text (`%snapshot-pane-text`).
        "(%snapshot-json snapshot)" => |s: Foreign<Snapshot>| -> Result<String, Error> {
            serde_json::to_string(&*s.0).map_err(|e| Error::new(format!("snapshot-json: {e}")))
        };
    }
    techne_vm::procedures! { vm;
        /// The size of the text of SNAPSHOT's pane I: (bytes lines).
        "(%snapshot-pane-size snapshot i)" => |s: Foreign<Snapshot>, i: usize| -> Result<Vec<usize>, Error> {
            let p = s.0.panes.get(i).ok_or_else(|| Error::new("snapshot-pane-size: no such pane"))?;
            Ok(vec![p.text.len_bytes(), p.text.len_lines()])
        };
    }
    techne_vm::procedures! { vm;
        /// The text of SNAPSHOT's pane I.
        "(%snapshot-pane-text snapshot i)" => |s: Foreign<Snapshot>, i: usize| -> Result<String, Error> {
            Ok(s.0.panes.get(i).ok_or_else(|| Error::new("snapshot-pane-text: no such pane"))?.text.to_string())
        };
    }
}
