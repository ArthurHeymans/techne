//! The application runtime for one frontend: a VM, an open document, one view
//! of it and the Lisp session that interprets keys (`lisp/editor/main.scm`).
//!
//! The runtime owns the document, independently of the views and Lisp
//! values that refer to it. Frontends talk to it only through `present`:
//! inputs in, snapshots out. It is single-threaded; a host runs it on its
//! own thread (`serve`) and moves the data across.

use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::mpsc,
    time::{Duration, Instant},
};

use techne_text::{Assoc, Document, Range, Recovery};
use techne_vm::{
    api::{Foreign, FromValue, IntoValue, Root},
    tasks::Progress,
    value::Value,
    vm::{Error, Vm},
};

use crate::{
    View,
    present::{CursorShape, Input, Output, Snapshot},
};

/// Where the editor's Lisp is, in the source tree for now.
pub fn lisp_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lisp/editor")
}

/// Unsaved edits of a file are journaled under the state directory, named by
/// a hash of the file's absolute path.
pub fn journal_for(path: &Path) -> std::io::Result<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .ok_or_else(|| std::io::Error::other("no XDG_STATE_HOME or HOME"))?;
    let dir = state.join("techne/journals");
    std::fs::create_dir_all(&dir)?;
    let abs = std::path::absolute(path)?;
    let hash = techne_text::journal::hash(abs.as_os_str().as_encoded_bytes());
    let name: String = hash[..12].iter().map(|b| format!("{b:02x}")).collect();
    Ok(dir.join(format!("{name}.journal")))
}

pub struct Runtime {
    vm: Vm,
    doc: Rc<RefCell<Document>>,
    view: Rc<RefCell<View>>,
    session: Root,
    procs: Procs,
    next_id: u64,
    /// When the inputs not yet answered by a snapshot were made.
    pending: Vec<Instant>,
}

/// The Lisp procedures the runtime calls.
struct Procs {
    press: Root,
    click: Root,
    status: Root,
    cursor: Root,
    quit: Root,
    message: Root,
    bindings: Root,
    unsendable: Root,
}

impl Runtime {
    /// Open `path` with its journal and start a session with the named key
    /// profile ("emacs" or "modal").
    pub fn open(path: &Path, journal: &Path, profile: &str) -> Result<(Runtime, Recovery), String> {
        let (doc, recovery) = Document::open(path, journal).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut runtime = Runtime::with_document(doc, profile).map_err(|e| e.to_string())?;
        if let Recovery::Replayed { transactions, .. } = recovery {
            runtime.message(&format!("Recovered {transactions} unsaved edits from the journal")).map_err(|e| e.to_string())?;
        }
        Ok((runtime, recovery))
    }

    pub fn with_document(doc: Document, profile: &str) -> Result<Runtime, Error> {
        let mut vm = Vm::new();
        crate::install(&mut vm);
        let main = lisp_dir().join("main.scm");
        vm.eval_source(&format!("(require {:?})", main.display().to_string()))?;
        let doc = Rc::new(RefCell::new(doc));
        let view = Rc::new(RefCell::new(View::new(doc.clone(), "user")));
        let mut global = |name: &str| -> Result<Root, Error> {
            let v = vm.get_global(name).ok_or_else(|| Error::new(format!("main.scm does not define {name}")))?;
            Ok(vm.root(v))
        };
        let start = global("start-session")?;
        let procs = Procs {
            press: global("editor-press")?,
            click: global("editor-click")?,
            status: global("status-line")?,
            cursor: global("cursor-shape")?,
            quit: global("session-quit?")?,
            message: global("editor-message!")?,
            bindings: global("bound-keys")?,
            unsendable: global("editor-unsendable!")?,
        };
        let view_value = Foreign(view.clone()).into_value(&mut vm)?;
        let view_root = vm.root(view_value);
        let profile = profile.into_value(&mut vm)?;
        let session = vm.call(start.get(), &[view_root.get(), profile])?;
        let session = vm.root(session);
        Ok(Runtime { vm, doc, view, session, procs, next_id: 0, pending: Vec::new() })
    }

    pub fn document(&self) -> &Rc<RefCell<Document>> {
        &self.doc
    }

    /// Handle one input. Errors in Lisp outside a command become the
    /// session's message rather than ending the session.
    pub fn handle(&mut self, input: Input) -> Option<Output> {
        let result = match input {
            Input::Key { key, at } => {
                self.pending.push(at);
                self.call_lisp(|p| &p.press, &[Arg::Session, Arg::Str(key)]).map(drop)
            }
            Input::Click { revision, pos, extend, at } => {
                self.pending.push(at);
                // Re-resolve a click on an older snapshot; refuse one whose
                // text is gone rather than apply it to other text.
                let mapped = self.doc.borrow().map_pos(pos, Assoc::Before, revision);
                match mapped {
                    Some((p, false)) => self.call_lisp(|p| &p.click, &[Arg::Session, Arg::Int(p), Arg::Bool(extend)]).map(drop),
                    _ => self.message("The text clicked on has changed"),
                }
            }
            Input::Scroll { revision, anchor } => self.view.borrow_mut().scroll_to(anchor, revision).map_err(Error::new),
            Input::Unsendable { keys } => self.call_lisp(|p| &p.unsendable, &[Arg::Session, Arg::Strs(keys)]).map(drop),
            Input::Unrecognized { input } => self.message(&format!("Unrecognized input: {input}")),
            Input::Close => return Some(Output::Quit),
        };
        if let Err(e) = result {
            let _ = self.message(&e.to_string());
        }
        match self.call_lisp(|p| &p.quit, &[Arg::Session]) {
            Ok(v) if v.is_truthy() => Some(Output::Quit),
            _ => None,
        }
    }

    /// The key sequences the session binds, in Emacs notation, sorted.
    pub fn bindings(&mut self) -> Vec<String> {
        let mut keys: Vec<String> =
            self.call_lisp(|p| &p.bindings, &[Arg::Session]).and_then(|v| Vec::from_value(&mut self.vm, v)).unwrap_or_default();
        keys.sort();
        keys
    }

    /// Serve one frontend: send it a snapshot and the bindings, then handle
    /// inputs as they come, answering each batch with a snapshot; between
    /// inputs, run background Lisp tasks. `send` delivers an output and wakes
    /// the frontend. Returns when the session quits or the frontend is gone.
    pub fn serve(mut self, inputs: mpsc::Receiver<Input>, send: impl Fn(Output)) {
        send(Output::Snapshot(Box::new(self.snapshot())));
        send(Output::Bindings(self.bindings()));
        let mut busy = self.run_tasks(Duration::ZERO) == Progress::OutOfTime;
        loop {
            let first = if busy {
                match inputs.try_recv() {
                    Ok(i) => i,
                    Err(mpsc::TryRecvError::Empty) => {
                        busy = self.run_tasks(Duration::from_millis(2)) == Progress::OutOfTime;
                        continue;
                    }
                    Err(mpsc::TryRecvError::Disconnected) => return,
                }
            } else {
                match inputs.recv() {
                    Ok(i) => i,
                    Err(_) => return,
                }
            };
            let mut quit = false;
            for input in std::iter::once(first).chain(inputs.try_iter()) {
                quit |= matches!(self.handle(input), Some(Output::Quit));
            }
            send(Output::Snapshot(Box::new(self.snapshot())));
            if quit {
                send(Output::Quit);
                return;
            }
            busy |= self.run_tasks(Duration::ZERO) == Progress::OutOfTime;
        }
    }

    /// Run background Lisp tasks for about `budget`.
    pub fn run_tasks(&mut self, budget: Duration) -> Progress {
        self.vm.run_tasks_for(budget)
    }

    /// Start a Lisp task from source evaluating to a procedure of no
    /// arguments (used to put the editor under load).
    pub fn spawn(&mut self, source: &str) -> Result<(), Error> {
        let f = self.vm.eval_source(source)?;
        self.vm.spawn(f);
        Ok(())
    }

    pub fn snapshot(&mut self) -> Snapshot {
        self.next_id += 1;
        let (selections, primary, scroll) = {
            let mut view = self.view.borrow_mut();
            let s = view.selection();
            let selections = s.ranges().iter().map(|r: &Range| (r.anchor, r.head)).collect();
            let primary = s.primary_index();
            (selections, primary, view.scroll())
        };
        let status = self
            .call_lisp(|p| &p.status, &[Arg::Session])
            .and_then(|v| String::from_value(&mut self.vm, v))
            .unwrap_or_else(|e| format!("status-line: {e}"));
        let cursor = match self.call_lisp(|p| &p.cursor, &[Arg::Session]) {
            Ok(v) if v.is_symbol() && &*techne_vm::reader::symbol_name(v.as_symbol()) == "block" => CursorShape::Block,
            _ => CursorShape::Bar,
        };
        let doc = self.doc.borrow();
        Snapshot {
            id: self.next_id,
            revision: doc.revision(),
            text: doc.text().clone(),
            selections,
            primary,
            cursor,
            scroll,
            status,
            answers: std::mem::take(&mut self.pending),
        }
    }

    /// Show `text` as the session's message.
    pub fn message(&mut self, text: &str) -> Result<(), Error> {
        self.call_lisp(|p| &p.message, &[Arg::Session, Arg::Str(text.to_string())]).map(drop)
    }

    fn call_lisp(&mut self, which: fn(&Procs) -> &Root, args: &[Arg]) -> Result<Value, Error> {
        let f = which(&self.procs).clone();
        let mut values = Vec::with_capacity(args.len());
        let mut roots = Vec::new();
        for a in args {
            let v = match a {
                Arg::Session => self.session.get(),
                Arg::Str(s) => s.as_str().into_value(&mut self.vm)?,
                Arg::Int(n) => (*n).into_value(&mut self.vm)?,
                Arg::Bool(b) => (*b).into_value(&mut self.vm)?,
                Arg::Strs(v) => v.clone().into_value(&mut self.vm)?,
            };
            roots.push(self.vm.root(v));
        }
        values.extend(roots.iter().map(Root::get));
        self.vm.call(f.get(), &values)
    }
}

enum Arg {
    Session,
    Str(String),
    Int(usize),
    Bool(bool),
    Strs(Vec<String>),
}
