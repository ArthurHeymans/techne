//! The application runtime for one frontend: a VM, the documents open in it,
//! and the Lisp session that interprets keys and arranges views of them in
//! panes (`lisp/editor/main.scm`).
//!
//! Frontends talk to it only through `present`: inputs in, snapshots out.
//! It is single-threaded; a host runs it on its own thread (`serve`) and
//! moves the data across.

use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
    rc::Rc,
    sync::mpsc,
    time::{Duration, Instant},
};

use techne_text::{Assoc, Document, Range, Recovery};
use techne_vm::{
    api::{Foreign, FromValue, IntoValue, Root},
    heap::{Kind, is_kind},
    tasks::Progress,
    value::Value,
    vm::{Error, Vm},
};

use crate::{
    View,
    present::{CursorShape, Highlight, Input, KeyHint, Minibuffer, Output, Pane, Place, Row, Run, Snapshot},
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

/// How far past a pane's scroll anchor layers are asked for highlights:
/// more than a screen, so that drawing never runs Lisp.
const LAYER_WINDOW: usize = 64 * 1024;

pub struct Runtime {
    vm: Vm,
    /// The document the runtime was opened with.
    doc: Rc<RefCell<Document>>,
    /// The panes' views, by id, as of the last snapshot.
    views: HashMap<u64, Rc<RefCell<View>>>,
    session: Root,
    procs: Procs,
    next_id: u64,
    /// When the inputs not yet answered by a snapshot were made.
    pending: Vec<Instant>,
    /// The session's state as last sent.
    state: Option<String>,
}

/// The Lisp procedures the runtime calls.
struct Procs {
    press: Root,
    click: Root,
    panes: Root,
    focus: Root,
    status: Root,
    echo: Root,
    layers: Root,
    cursor: Root,
    quit: Root,
    message: Root,
    bindings: Root,
    unsendable: Root,
    minibuffer: Root,
    places: Root,
    hints: Root,
    state: Root,
    restore: Root,
    clipboard_in: Root,
    clipboard_out: Root,
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
        techne_process::install(&mut vm)?;
        // The application, and its interface for extensions, the library
        // (techne editor).
        for file in ["main.scm", "api.scm"] {
            vm.eval_source(&format!("(require {:?})", lisp_dir().join(file).display().to_string()))?;
        }
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
            panes: global("editor-panes")?,
            focus: global("editor-focus")?,
            status: global("pane-status")?,
            echo: global("echo-line")?,
            layers: global("pane-layers")?,
            cursor: global("cursor-shape")?,
            quit: global("session-quit?")?,
            message: global("editor-message!")?,
            bindings: global("bound-keys")?,
            unsendable: global("editor-unsendable!")?,
            minibuffer: global("editor-minibuffer")?,
            places: global("editor-pane-places")?,
            hints: global("editor-key-hints")?,
            state: global("editor-session-state")?,
            restore: global("editor-restore!")?,
            clipboard_in: global("editor-clipboard!")?,
            clipboard_out: global("editor-clipboard-out")?,
        };
        let view_value = Foreign(view.clone()).into_value(&mut vm)?;
        let view_root = vm.root(view_value);
        let profile = profile.into_value(&mut vm)?;
        let session = vm.call(start.get(), &[view_root.get(), profile])?;
        let session = vm.root(session);
        let id = view.borrow().id();
        let views = HashMap::from([(id, view)]);
        Ok(Runtime { vm, doc, views, session, procs, next_id: 0, pending: Vec::new(), state: None })
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
            Input::Click { view, revision, pos, extend, at } => {
                self.pending.push(at);
                // Re-resolve a click on an older snapshot; refuse one whose
                // text is gone rather than apply it to other text.
                match self.views.get(&view).cloned() {
                    None => self.message("The pane clicked on is gone"),
                    Some(v) => {
                        let mapped = v.borrow().document().borrow().map_pos(pos, Assoc::Before, revision);
                        match mapped {
                            Some((p, false)) => {
                                let args = [Arg::Session, Arg::View(v), Arg::Int(p), Arg::Bool(extend)];
                                self.call_lisp(|p| &p.click, &args).map(drop)
                            }
                            _ => self.message("The text clicked on has changed"),
                        }
                    }
                }
            }
            Input::Scroll { view, revision, anchor } => match self.views.get(&view) {
                Some(v) => v.borrow_mut().scroll_to(anchor, revision).map_err(Error::new),
                None => Ok(()),
            },
            Input::Unsendable { keys } => self.call_lisp(|p| &p.unsendable, &[Arg::Session, Arg::Strs(keys)]).map(drop),
            Input::Unrecognized { input } => self.message(&format!("Unrecognized input: {input}")),
            Input::Clipboard { text } => self.call_lisp(|p| &p.clipboard_in, &[Arg::Session, Arg::Str(text)]).map(drop),
            Input::Wake => Ok(()),
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
    /// inputs as they come, answering each batch with a snapshot. Between
    /// inputs, background Lisp tasks run; when one wakes (a process wrote,
    /// a timer fired) the frontend gets a snapshot too, so their effects
    /// show as they happen. `wake` is a sender of `inputs`, for the VM to
    /// say a task woke. `send` delivers an output and wakes the frontend.
    /// Returns when the session quits or the frontend is gone.
    pub fn serve(mut self, inputs: mpsc::Receiver<Input>, wake: mpsc::Sender<Input>, send: impl Fn(Output)) {
        self.vm.set_wake_notifier(move || {
            let _ = wake.send(Input::Wake);
        });
        send(Output::Snapshot(Box::new(self.snapshot())));
        send(Output::Bindings(self.bindings()));
        if let Some(state) = self.changed_state() {
            send(Output::Session(state));
        }
        let mut progress = self.run_tasks(Duration::ZERO);
        loop {
            // Busy tasks run between inputs; waiting ones until a timer.
            let first = match progress {
                Progress::OutOfTime => match inputs.try_recv() {
                    Ok(i) => Some(i),
                    Err(mpsc::TryRecvError::Empty) => {
                        progress = self.run_tasks(Duration::from_millis(2));
                        if progress == Progress::OutOfTime {
                            continue;
                        }
                        None
                    }
                    Err(mpsc::TryRecvError::Disconnected) => return,
                },
                Progress::Blocked if self.vm.next_timer().is_some() => {
                    let wait = self.vm.next_timer().map_or(Duration::ZERO, |t| t.saturating_duration_since(Instant::now()));
                    match inputs.recv_timeout(wait) {
                        Ok(i) => Some(i),
                        Err(mpsc::RecvTimeoutError::Timeout) => None,
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }
                _ => match inputs.recv() {
                    Ok(i) => Some(i),
                    Err(_) => return,
                },
            };
            let mut quit = false;
            for input in first.into_iter().chain(inputs.try_iter()) {
                quit |= matches!(self.handle(input), Some(Output::Quit));
            }
            progress = self.run_tasks(Duration::ZERO);
            send(Output::Snapshot(Box::new(self.snapshot())));
            if let Some(text) = self.clipboard_out() {
                send(Output::Clipboard(text));
            }
            if let Some(state) = self.changed_state() {
                send(Output::Session(state));
            }
            if quit {
                send(Output::Quit);
                return;
            }
        }
    }

    /// Text killed since last asked, for the system clipboard.
    pub fn clipboard_out(&mut self) -> Option<String> {
        let v = self.call_lisp(|p| &p.clipboard_out, &[Arg::Session]).ok()?;
        Option::<String>::from_value(&mut self.vm, v).ok().flatten()
    }

    /// The session's state for coming back after a crash, if it changed
    /// since it was last asked for.
    pub fn changed_state(&mut self) -> Option<String> {
        let state = self.call_lisp(|p| &p.state, &[Arg::Session]).and_then(|v| String::from_value(&mut self.vm, v)).ok()?;
        (self.state.as_ref() != Some(&state)).then(|| {
            self.state = Some(state.clone());
            state
        })
    }

    /// Bring back a session from its state (`Output::Session`): its files
    /// are opened again, with their unsaved edits, in its panes.
    pub fn restore(&mut self, state: &str) -> Result<(), Error> {
        self.call_lisp(|p| &p.restore, &[Arg::Session, Arg::Str(state.to_string())]).map(drop)
    }

    /// Run background Lisp tasks for about `budget`.
    pub fn run_tasks(&mut self, budget: Duration) -> Progress {
        self.vm.run_tasks_for(budget)
    }

    /// Evaluate source in the user module, as the session's code would (for
    /// hosts and tests); the result as `write` shows it.
    pub fn eval(&mut self, source: &str) -> Result<String, Error> {
        self.vm.eval_source(source).map(techne_vm::builtins::repr)
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
        let views = self.pane_views().unwrap_or_else(|_| self.views.values().take(1).cloned().collect());
        self.views = views.iter().map(|v| (v.borrow().id(), v.clone())).collect();
        let places = self.places().unwrap_or_default();
        let panes =
            views.iter().enumerate().map(|(i, v)| Pane { place: places.get(i).copied().unwrap_or(Place::WHOLE), ..self.pane(v) }).collect();
        let focus = self
            .call_lisp(|p| &p.focus, &[Arg::Session])
            .and_then(|v| usize::from_value(&mut self.vm, v))
            .unwrap_or(0)
            .min(views.len().saturating_sub(1));
        let echo = self
            .call_lisp(|p| &p.echo, &[Arg::Session])
            .and_then(|v| String::from_value(&mut self.vm, v))
            .unwrap_or_else(|e| format!("echo-line: {e}"));
        let minibuffer = self.minibuffer().unwrap_or_else(|e| {
            Some(Minibuffer {
                prompt: format!("editor-minibuffer: {e} "),
                input: String::new(),
                caret: 0,
                rows: Vec::new(),
                selected: None,
            })
        });
        let key_hints = self.key_hints().unwrap_or_default();
        Snapshot { id: self.next_id, panes, focus, echo, minibuffer, key_hints, answers: std::mem::take(&mut self.pending) }
    }

    /// The open minibuffer: Lisp gives `(prompt input-view rows selected)`
    /// or #f.
    fn minibuffer(&mut self) -> Result<Option<Minibuffer>, Error> {
        let v = self.call_lisp(|p| &p.minibuffer, &[Arg::Session])?;
        if v.is_false() {
            return Ok(None);
        }
        let vm = &mut self.vm;
        let [prompt, view, rows, selected] = Vec::<Value>::from_value(vm, v)?[..] else {
            return Err(Error::new("the minibuffer is (prompt view rows selected)"));
        };
        let (prompt, rows, selected) =
            (String::from_value(vm, prompt)?, Vec::<Value>::from_value(vm, rows)?, Option::<usize>::from_value(vm, selected)?);
        let view = Foreign::<RefCell<View>>::from_value(vm, view)?;
        let (input, caret) = {
            let mut v = view.borrow_mut();
            let s = v.selection();
            let caret = s.ranges()[s.primary_index()].head;
            (v.document().borrow().text().to_string(), caret)
        };
        let rows = rows.into_iter().map(|r| row(vm, r)).collect::<Result<Vec<_>, _>>()?;
        Ok(Some(Minibuffer { prompt, input, caret, selected: selected.filter(|&i| i < rows.len()), rows }))
    }

    /// The keys which-key shows: Lisp gives `(key description prefix?)`.
    fn key_hints(&mut self) -> Result<Vec<KeyHint>, Error> {
        let list = self.call_lisp(|p| &p.hints, &[Arg::Session])?;
        let vm = &mut self.vm;
        Vec::<Value>::from_value(vm, list)?
            .into_iter()
            .map(|h| match Vec::<Value>::from_value(vm, h)?[..] {
                [key, description, prefix] => Ok(KeyHint {
                    key: String::from_value(vm, key)?,
                    description: String::from_value(vm, description)?,
                    prefix: prefix.is_truthy(),
                }),
                _ => Err(Error::new("a key hint is (key description prefix?)")),
            })
            .collect()
    }

    /// Where the panes are: Lisp gives `(x y w h)` for each.
    fn places(&mut self) -> Result<Vec<Place>, Error> {
        let list = self.call_lisp(|p| &p.places, &[Arg::Session])?;
        Vec::<Vec<f64>>::from_value(&mut self.vm, list)?
            .into_iter()
            .map(|p| match p[..] {
                [x, y, w, h] => Ok(Place { x: x as f32, y: y as f32, w: w as f32, h: h as f32 }),
                _ => Err(Error::new("a place is (x y w h)")),
            })
            .collect()
    }

    /// The views the session shows, in order.
    fn pane_views(&mut self) -> Result<Vec<Rc<RefCell<View>>>, Error> {
        let list = self.call_lisp(|p| &p.panes, &[Arg::Session])?;
        let views = Vec::<Foreign<RefCell<View>>>::from_value(&mut self.vm, list)?;
        if views.is_empty() {
            return Err(Error::new("editor-panes: no panes"));
        }
        Ok(views.into_iter().map(|v| v.0).collect())
    }

    fn pane(&mut self, view: &Rc<RefCell<View>>) -> Pane {
        let (id, selections, primary, scroll) = {
            let mut v = view.borrow_mut();
            let s = v.selection();
            let selections = s.ranges().iter().map(|r: &Range| (r.anchor, r.head)).collect();
            let primary = s.primary_index();
            (v.id(), selections, primary, v.scroll())
        };
        let status = self
            .call_lisp(|p| &p.status, &[Arg::Session, Arg::View(view.clone())])
            .and_then(|v| String::from_value(&mut self.vm, v))
            .unwrap_or_else(|e| format!("pane-status: {e}"));
        let cursor = match self.call_lisp(|p| &p.cursor, &[Arg::Session, Arg::View(view.clone())]) {
            Ok(v) if v.is_symbol() && &*techne_vm::reader::symbol_name(v.as_symbol()) == "block" => CursorShape::Block,
            _ => CursorShape::Bar,
        };
        let doc = view.borrow().document().clone();
        let len = doc.borrow().len();
        let window = [Arg::Session, Arg::View(view.clone()), Arg::Int(scroll), Arg::Int((scroll + LAYER_WINDOW).min(len))];
        let layers = self.call_lisp(|p| &p.layers, &window).and_then(|v| highlights(&mut self.vm, v)).unwrap_or_default();
        let layers = layers.into_iter().filter(|h| h.from < h.to && h.to <= len).collect();
        let doc = doc.borrow();
        Pane {
            view: id,
            revision: doc.revision(),
            text: doc.text().clone(),
            selections,
            primary,
            cursor,
            scroll,
            status,
            layers,
            place: Place::WHOLE,
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
                Arg::View(v) => Foreign(v.clone()).into_value(&mut self.vm)?,
            };
            roots.push(self.vm.root(v));
        }
        values.extend(roots.iter().map(Root::get));
        self.vm.call(f.get(), &values)
    }
}

/// Highlights from Lisp: a list of `(from to face)`, the face a symbol.
fn highlights(vm: &mut Vm, v: Value) -> Result<Vec<Highlight>, Error> {
    Vec::<Value>::from_value(vm, v)?
        .into_iter()
        .map(|h| match Vec::<Value>::from_value(vm, h)?[..] {
            [from, to, face] if face.is_symbol() => Ok(Highlight {
                from: usize::from_value(vm, from)?,
                to: usize::from_value(vm, to)?,
                face: techne_vm::reader::symbol_name(face.as_symbol()).to_string(),
            }),
            _ => Err(Error::new("a highlight is (from to face)")),
        })
        .collect()
}

/// A row from Lisp: a list of columns, each a string or a list of runs,
/// a run a string or `(text face)`, the face a symbol or #f.
fn row(vm: &mut Vm, v: Value) -> Result<Row, Error> {
    let run = |vm: &mut Vm, r: Value| -> Result<Run, Error> {
        if is_kind(r, Kind::String) {
            return Ok(Run { text: String::from_value(vm, r)?, face: None });
        }
        match Vec::<Value>::from_value(vm, r)?[..] {
            [text, face] => Ok(Run {
                text: String::from_value(vm, text)?,
                face: face.is_symbol().then(|| techne_vm::reader::symbol_name(face.as_symbol()).to_string()),
            }),
            _ => Err(Error::new("a run is a string or (text face)")),
        }
    };
    let columns = Vec::<Value>::from_value(vm, v)?
        .into_iter()
        .map(|c| {
            if is_kind(c, Kind::String) {
                run(vm, c).map(|r| vec![r])
            } else {
                Vec::<Value>::from_value(vm, c)?.into_iter().map(|r| run(vm, r)).collect()
            }
        })
        .collect::<Result<_, Error>>()?;
    Ok(Row { columns })
}

enum Arg {
    Session,
    View(Rc<RefCell<View>>),
    Str(String),
    Int(usize),
    Bool(bool),
    Strs(Vec<String>),
}
