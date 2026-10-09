//! The application runtime: a VM, the documents open in it, and for each
//! frontend attached to it a Lisp session that interprets its keys and
//! arranges views of the documents in its panes (`lisp/editor/main.scm`,
//! what it asks of a session in `host.scm`). The sessions share the
//! documents and buffers (EDITOR.md, section 6).
//!
//! Frontends talk to it only through `present`: inputs in, snapshots out.
//! It is single-threaded; a host runs it on its own thread (`serve`) and
//! moves the data across.

use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

use techne_text::{Assoc, Document, Range, Recovery};
pub use techne_vm::tasks::Progress;
use techne_vm::{
    api::{Foreign, FromValue, IntoValue, Root},
    heap::{Kind, is_kind},
    value::Value,
    vm::{Error, ExecId, InterruptHandle, Stop, Vm},
};

use crate::{
    Text, View,
    present::{
        Completion, CursorShape, Display, Highlight, Input, KeyHint, LineNumbers, Minibuffer, Output, Pane, Place, Recenter, Row, Run,
        Snapshot, ViewRequest,
    },
    server::Reply,
};

/// Where the editor's Lisp is, in the source tree for now.
pub fn lisp_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lisp/editor")
}

/// The editor's own features that are packages (`lisp/editor/packages`), in
/// name order.
pub fn builtin_packages() -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(lisp_dir().join("packages"))
        .map(|entries| entries.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|e| e == "scm")).collect())
        .unwrap_or_default();
    paths.sort();
    paths
}

/// The journal directory under `state`, made if missing. It is private, as
/// the journals in it are, also when it was made before it had to be.
fn journal_dir(state: &Path) -> std::io::Result<PathBuf> {
    let dir = state.join("techne/journals");
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.recursive(true).create(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}

/// Unsaved edits of a file are journaled under the state directory, named by
/// a hash of the file's absolute path.
pub fn journal_for(path: &Path) -> std::io::Result<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .ok_or_else(|| std::io::Error::other("no XDG_STATE_HOME or HOME"))?;
    let dir = journal_dir(&state)?;
    let name = |path: &Path| {
        let hash = techne_text::journal::hash(path.as_os_str().as_encoded_bytes());
        let name: String = hash[..12].iter().map(|b| format!("{b:02x}")).collect();
        dir.join(format!("{name}.journal"))
    };
    let canonical = name(&techne_text::document::file_path(path)?);
    let legacy = name(&std::path::absolute(path)?);
    if legacy != canonical && legacy.try_exists()? {
        techne_text::journal::migrate(&legacy, &canonical)?;
    }
    Ok(canonical)
}

/// How far past a pane's scroll anchor layers are asked for highlights:
/// more than a screen, so that drawing never runs Lisp.
const LAYER_WINDOW: usize = 64 * 1024;

/// A frontend attached to a runtime, by the number its host gave it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Client(pub u64);

impl Client {
    /// The frontend a runtime is made for by `open` and `with_document`,
    /// which the methods not naming one act for.
    pub const FIRST: Client = Client(0);
}

/// A file to open, and where its unsaved edits are journaled.
#[derive(Clone, Debug)]
pub struct File {
    pub path: PathBuf,
    pub journal: PathBuf,
}

/// What a runtime gets while it serves (`Runtime::serve`).
pub enum Msg {
    /// A frontend attaches (`Runtime::attach`), its inputs interrupted
    /// with `interrupts`. `state` is its session as it last sent it
    /// (`Output::Session`), brought back after a crash.
    Attach { client: Client, file: Option<File>, profile: String, state: Option<String>, interrupts: Arc<Interrupts> },
    /// An input from an attached frontend.
    Input(Client, Input),
    /// Show the file at `path` in the session used last, for another
    /// program (`server`): `reply` gets `Opened`, or `Failed`, and with
    /// `wait`, `Done` once the file is done with. It is dropped after
    /// the last. `id` is the request's, unique to the process.
    Open { id: usize, path: PathBuf, wait: bool, reply: mpsc::Sender<Reply> },
    /// The program of request `id` stopped waiting (`Open`).
    Gone(usize),
    /// A background task woke.
    Wake,
    /// End: no frontend is attached, and none will be.
    Stop,
}

pub struct Runtime {
    vm: Vm,
    /// The documents of files, by path, shared by all the sessions.
    documents: crate::Documents,
    /// The document the runtime was opened with.
    doc: Option<Rc<RefCell<Document>>>,
    clients: BTreeMap<Client, Attached>,
    /// The frontend whose session Lisp is called for.
    serving: Client,
    next_id: u64,
    /// The programs waiting until a file they opened is done with, by the
    /// id of their request, which Lisp knows them by (`Msg::Open`).
    waiting: HashMap<usize, mpsc::Sender<Reply>>,
}

/// A frontend attached, and its session.
struct Attached {
    session: Root,
    /// The panes' views, by id, as of the last snapshot.
    views: HashMap<u64, Rc<RefCell<View>>>,
    /// When the inputs not yet answered by a snapshot were made.
    pending: Vec<Instant>,
    /// The session's state as last sent.
    state: Option<String>,
    /// Which of its inputs are interrupted (`Interrupts`).
    interrupts: Arc<Interrupts>,
    /// The number of its next input (`Interrupts`).
    next: u64,
}

/// Stops a frontend's inputs from another thread, as C-g does in Emacs: a
/// key whose command evaluates forever is broken, and the inputs queued
/// behind it are discarded. A frontend's inputs are numbered from 0 in the
/// order it sends them, and each is handled as an execution of its own
/// (`techne_vm::stop`): C-g reaches that execution only, not another
/// frontend's command, a background task or what runs between inputs (a
/// snapshot). A command may catch the break and go on; stopping it again,
/// as a second C-g does, kills it.
#[derive(Default)]
pub struct Interrupts(Mutex<Gate>);

#[derive(Default)]
struct Gate {
    /// Inputs numbered below are stopped or discarded.
    before: u64,
    /// The input being handled, its execution, and whether it was broken.
    handling: Option<(u64, ExecId, bool)>,
    stop: Option<InterruptHandle>,
}

impl Interrupts {
    /// Stop the inputs numbered below `n`: the one being handled is broken
    /// (it raises the condition "interrupted", which its command shows as
    /// its error), or killed if it was broken already; those not handled
    /// yet are discarded.
    pub fn interrupt_before(&self, n: u64) {
        self.stop_before(n, false);
    }

    /// Kill the input being handled if it is numbered below `n`, and
    /// discard those after it that are.
    pub fn kill_before(&self, n: u64) {
        self.stop_before(n, true);
    }

    fn stop_before(&self, n: u64, kill: bool) {
        let mut gate = self.0.lock().expect("the gate");
        gate.before = gate.before.max(n);
        let before = gate.before;
        let stop = gate.stop.clone();
        if let (Some((k, exec, broken)), Some(stop)) = (&mut gate.handling, stop)
            && *k < before
        {
            stop.stop(*exec, if kill || *broken { Stop::Kill } else { Stop::Break });
            *broken = true;
        }
    }

    /// Start handling input `k` as the execution `exec`; false when it is
    /// discarded.
    fn start(&self, k: u64, exec: ExecId) -> bool {
        let mut gate = self.0.lock().expect("the gate");
        let go = k >= gate.before;
        gate.handling = go.then_some((k, exec, false));
        go
    }

    /// Input `k` was handled.
    fn end(&self) {
        self.0.lock().expect("the gate").handling = None;
    }
}

/// The Lisp procedures the runtime calls, by name: each is looked up when
/// it is called, so redefining one while running takes effect at once.
const PROCS: [&str; 28] = [
    "editor-open!",
    "editor-take-done!",
    "editor-forget!",
    "editor-detach!",
    "editor-select!",
    "editor-completion",
    "pane-display",
    "editor-press",
    "editor-click",
    "editor-panes",
    "editor-focus",
    "pane-status",
    "echo-line",
    "pane-layers",
    "cursor-shape",
    "session-quit?",
    "editor-message!",
    "bound-keys",
    "editor-unsendable!",
    "editor-minibuffer",
    "editor-pane-places",
    "editor-key-hints",
    "editor-take-request!",
    "editor-paged!",
    "editor-session-state",
    "editor-restore!",
    "editor-clipboard!",
    "editor-clipboard-out",
];

impl Runtime {
    /// Open `path` with its journal and start a session with the named key
    /// profile ("emacs" or "modal").
    pub fn open(path: &Path, journal: &Path, profile: &str) -> Result<(Runtime, Recovery), String> {
        let mut runtime = Runtime::new().map_err(|e| e.to_string())?;
        let file = File { path: path.to_owned(), journal: journal.to_owned() };
        let recovery = runtime.attach(Client::FIRST, Some(&file), profile)?.unwrap_or(Recovery::Clean);
        Ok((runtime, recovery))
    }

    /// A runtime with a session over `doc`, keys read by `profile`.
    pub fn with_document(doc: Document, profile: &str) -> Result<Runtime, Error> {
        let mut runtime = Runtime::new()?;
        let doc = Rc::new(RefCell::new(doc));
        if let Some(path) = doc.borrow().path() {
            runtime.documents.borrow_mut().insert(path.to_owned(), Rc::downgrade(&doc));
        }
        runtime.doc = Some(doc.clone());
        runtime.start_session(Client::FIRST, Some(View::of_document(doc, "user")), profile)?;
        Ok(runtime)
    }

    /// A runtime with the editor loaded and no frontend attached.
    pub fn new() -> Result<Runtime, Error> {
        let documents = crate::Documents::default();
        let mut vm = Vm::new();
        crate::install_with_documents(&mut vm, documents.clone());
        techne_process::install(&mut vm)?;
        // The application, and its interface for extensions, the library
        // (techne editor).
        for file in ["main.scm", "api.scm"] {
            vm.eval_source(&format!("(require {:?})", lisp_dir().join(file).display().to_string()))?;
        }
        // Features written against that library alone, loaded as any
        // extension is: each a package of its own, named by its file.
        for path in builtin_packages() {
            let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            vm.eval_source(&format!("(load-package '{name} {:?})", path.display().to_string()))?;
        }
        for name in PROCS {
            if vm.get_global(name).is_none() {
                return Err(Error::new(format!("main.scm does not define {name}")));
            }
        }
        Ok(Runtime { vm, documents, doc: None, clients: BTreeMap::new(), serving: Client::FIRST, next_id: 0, waiting: HashMap::new() })
    }

    /// Attach a frontend as `client`, its keys read by `profile`: its
    /// session shows `file`, opened with its journal unless it is open
    /// already, and without one what the current session shows. Returns
    /// what was recovered from the journal when the file was opened.
    pub fn attach(&mut self, client: Client, file: Option<&File>, profile: &str) -> Result<Option<Recovery>, String> {
        let opened = file.map(|f| crate::open_shared(&self.documents, &f.path, || Ok(f.journal.clone()))).transpose()?;
        if let Some((doc, _)) = &opened
            && self.doc.is_none()
        {
            self.doc = Some(doc.clone());
        }
        let (view, recovery) = opened.map_or((None, None), |(doc, r)| (Some(View::of_document(doc, "user")), r));
        self.start_session(client, view, profile).map_err(|e| e.to_string())?;
        if let Some(Recovery::Replayed { transactions, .. }) = &recovery {
            self.message(&format!("Recovered {transactions} unsaved edits from the journal")).map_err(|e| e.to_string())?;
        }
        Ok(recovery)
    }

    fn start_session(&mut self, client: Client, view: Option<View>, profile: &str) -> Result<(), Error> {
        let start = self.vm.get_global("start-session").ok_or_else(|| Error::new("main.scm does not define start-session"))?;
        let start = self.vm.root(start);
        let view = view.map(|v| Rc::new(RefCell::new(v)));
        let view_value = match &view {
            Some(v) => Foreign(v.clone()).into_value(&mut self.vm)?,
            None => false.into_value(&mut self.vm)?,
        };
        let view_root = self.vm.root(view_value);
        let profile = profile.into_value(&mut self.vm)?;
        let session = self.vm.call(start.get(), &[view_root.get(), profile])?;
        let session = self.vm.root(session);
        let views = view.map(|v| {
            let id = v.borrow().id();
            (id, v)
        });
        let views = views.into_iter().collect();
        let attached = Attached { session, views, pending: Vec::new(), state: None, interrupts: Arc::default(), next: 0 };
        self.clients.insert(client, attached);
        self.serving = client;
        Ok(())
    }

    /// Detach `client`: its session ends; the buffers it showed stay.
    pub fn detach(&mut self, client: Client) {
        if self.clients.contains_key(&client) {
            self.serving = client;
            if let Err(e) = self.call_lisp("editor-detach!", &[Arg::Session]) {
                eprintln!("techne: editor-detach!: {e}");
            }
            self.clients.remove(&client);
        }
    }

    /// The frontends attached.
    pub fn clients(&self) -> impl Iterator<Item = Client> + '_ {
        self.clients.keys().copied()
    }

    /// The document the runtime was opened with.
    pub fn document(&self) -> Option<&Rc<RefCell<Document>>> {
        self.doc.as_ref()
    }

    /// Interrupt the inputs of `client` with `interrupts` when it serves.
    pub fn set_interrupts(&mut self, client: Client, interrupts: Arc<Interrupts>) {
        if let Some(a) = self.clients.get_mut(&client) {
            a.interrupts = interrupts;
        }
    }

    /// Act for `client`: the methods that do not name one act for it.
    pub fn select(&mut self, client: Client) -> Result<(), Error> {
        if !self.clients.contains_key(&client) {
            return Err(Error::new(format!("no frontend {} is attached", client.0)));
        }
        self.serving = client;
        self.call_lisp("editor-select!", &[Arg::Session]).map(drop)
    }

    /// Handle one input of the frontend acted for. Errors in Lisp outside
    /// a command become the session's message rather than ending the
    /// session. Returns `Output::Quit` when the session quits.
    pub fn handle(&mut self, input: Input) -> Option<Output> {
        let exec = self.vm.new_execution();
        self.handle_as(exec, input)
    }

    /// Handle `input` as the execution `exec` (`Vm::new_execution`), which
    /// stops reach (`Interrupts`); its error, if any, is shown after it.
    fn handle_as(&mut self, exec: ExecId, input: Input) -> Option<Output> {
        if matches!(input, Input::Close) {
            self.vm.discard_execution(exec);
            return Some(Output::Quit);
        }
        self.vm.enter_execution(exec);
        let result = self.handle_input(input);
        self.vm.leave_execution(exec);
        if let Err(e) = result {
            let _ = self.message(&if e.is_kill() { "Quit (killed)".to_string() } else { e.to_string() });
        }
        self.quitting().then_some(Output::Quit)
    }

    fn handle_input(&mut self, input: Input) -> Result<(), Error> {
        let views = self.clients.get(&self.serving).map(|a| a.views.clone()).unwrap_or_default();
        match input {
            Input::Key { key, at } => {
                self.pending(at);
                self.call_lisp("editor-press", &[Arg::Session, Arg::Str(key)]).map(drop)
            }
            Input::Click { view, revision, pos, extend, at } => {
                self.pending(at);
                // Re-resolve a click on an older snapshot; refuse one whose
                // text is gone rather than apply it to other text.
                match views.get(&view).cloned() {
                    None => self.message("The pane clicked on is gone"),
                    Some(v) => {
                        let mapped = v.borrow().text().map_pos(pos, Assoc::Before, revision);
                        match mapped {
                            Some((p, false)) => {
                                let args = [Arg::Session, Arg::View(v), Arg::Int(p), Arg::Bool(extend)];
                                self.call_lisp("editor-click", &args).map(drop)
                            }
                            _ => self.message("The text clicked on has changed"),
                        }
                    }
                }
            }
            Input::Scroll { view, revision, anchor, caret } => match views.get(&view).cloned() {
                Some(v) => {
                    let scrolled = v.borrow_mut().scroll_to(anchor, revision).map_err(Error::new);
                    let mapped = caret.and_then(|p| v.borrow().text().map_pos(p, Assoc::Before, revision));
                    match (scrolled, mapped) {
                        (Ok(()), Some((p, _))) => self.call_lisp("editor-paged!", &[Arg::Session, Arg::View(v), Arg::Int(p)]).map(drop),
                        (result, _) => result,
                    }
                }
                None => Ok(()),
            },
            Input::Edge { view, end } => match views.get(&view) {
                Some(_) => self.message(if end { "End of buffer" } else { "Beginning of buffer" }),
                None => Ok(()),
            },
            Input::Unsendable { keys } => self.call_lisp("editor-unsendable!", &[Arg::Session, Arg::Strs(keys)]).map(drop),
            Input::Unrecognized { input } => self.message(&format!("Unrecognized input: {input}")),
            Input::Clipboard { text } => self.call_lisp("editor-clipboard!", &[Arg::Session, Arg::Str(text)]).map(drop),
            Input::Close => Ok(()),
        }
    }

    /// Whether the session acted for has been asked to end.
    fn quitting(&mut self) -> bool {
        self.call_lisp("session-quit?", &[Arg::Session]).is_ok_and(|v| v.is_truthy())
    }

    /// An input made at `at` waits for the snapshot that answers it.
    fn pending(&mut self, at: Instant) {
        if let Some(a) = self.clients.get_mut(&self.serving) {
            a.pending.push(at);
        }
    }

    /// The key sequences the session binds, in Emacs notation, sorted.
    pub fn bindings(&mut self) -> Vec<String> {
        let mut keys: Vec<String> =
            self.call_lisp("bound-keys", &[Arg::Session]).and_then(|v| Vec::from_value(&mut self.vm, v)).unwrap_or_default();
        keys.sort();
        keys
    }

    /// Serve the frontends attached and those attaching: each gets a
    /// snapshot and the bindings when it attaches, then its inputs are
    /// handled as they come. Every batch of messages is answered with a
    /// snapshot for each frontend, as an input of one may change what
    /// another shows. Between inputs, background Lisp tasks run; when one
    /// wakes (a process wrote, a timer fired) the frontends get a snapshot
    /// too, so their effects show as they happen. `wake` is a sender of
    /// `inputs`, for the VM to say a task woke. `send` delivers an output
    /// to a frontend and wakes it. A frontend whose session quits gets
    /// `Output::Quit` and is detached. Returns on `Msg::Stop`, or when no
    /// more can come.
    pub fn serve(mut self, inputs: mpsc::Receiver<Msg>, wake: mpsc::Sender<Msg>, send: impl Fn(Client, Output)) {
        self.vm.set_wake_notifier(move || {
            let _ = wake.send(Msg::Wake);
        });
        let stop = self.vm.interrupt_handle();
        for a in self.clients.values() {
            a.interrupts.0.lock().expect("the gate").stop = Some(stop.clone());
        }
        let mut greet: Vec<Client> = self.clients().collect();
        let mut progress = self.run_tasks(Duration::ZERO);
        self.answer(&mut greet, &send);
        loop {
            // Busy tasks run between inputs; waiting ones until a timer.
            let first = match progress {
                Progress::OutOfTime => match inputs.try_recv() {
                    Ok(m) => Some(m),
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
                        Ok(m) => Some(m),
                        Err(mpsc::RecvTimeoutError::Timeout) => None,
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }
                _ => match inputs.recv() {
                    Ok(m) => Some(m),
                    Err(_) => return,
                },
            };
            for msg in first.into_iter().chain(inputs.try_iter()) {
                match msg {
                    Msg::Wake => {}
                    Msg::Stop => return,
                    Msg::Attach { client, file, profile, state, interrupts } => match self.attach(client, file.as_ref(), &profile) {
                        Ok(_) => {
                            interrupts.0.lock().expect("the gate").stop = Some(stop.clone());
                            self.set_interrupts(client, interrupts);
                            if let Some(e) = state.and_then(|s| self.restore(&s).err()) {
                                let _ = self.message(&format!("The session could not be restored: {e}"));
                            }
                            greet.push(client);
                        }
                        Err(e) => send(client, Output::Refused(e)),
                    },
                    Msg::Open { id, path, wait, reply } => self.open_for(id, &path, wait, reply),
                    Msg::Gone(id) => {
                        if self.waiting.remove(&id).is_some() {
                            let _ = self.call_lisp("editor-forget!", &[Arg::Int(id)]);
                        }
                    }
                    Msg::Input(client, input) => {
                        let Some(a) = self.clients.get_mut(&client) else { continue };
                        if matches!(input, Input::Close) {
                            self.detach(client);
                            send(client, Output::Quit);
                            continue;
                        }
                        let (k, interrupts) = (a.next, a.interrupts.clone());
                        a.next += 1;
                        let exec = self.vm.new_execution();
                        if interrupts.start(k, exec) {
                            if self.select(client).is_ok() {
                                self.handle_as(exec, input);
                            } else {
                                self.vm.discard_execution(exec);
                            }
                            interrupts.end();
                        } else {
                            self.vm.discard_execution(exec);
                        }
                    }
                }
            }
            progress = self.run_tasks(Duration::ZERO);
            self.answer(&mut greet, &send);
        }
    }

    /// Show the file at `path` for another program (`Msg::Open`).
    fn open_for(&mut self, id: usize, path: &Path, wait: bool, reply: mpsc::Sender<Reply>) {
        let id = wait.then_some(id);
        let args = [Arg::Str(path.display().to_string()), id.map_or(Arg::Bool(false), Arg::Int)];
        match self.call_lisp("editor-open!", &args) {
            Ok(_) => {
                let _ = reply.send(Reply::Opened);
                if let Some(id) = id {
                    self.waiting.insert(id, reply);
                }
            }
            Err(e) => {
                let _ = reply.send(Reply::Failed(e.to_string()));
            }
        }
    }

    /// Answer every frontend with a snapshot, those in `greet`, attached
    /// since, with the bindings too; detach those whose session quit. Tell
    /// the programs waiting for files done with since.
    fn answer(&mut self, greet: &mut Vec<Client>, send: &impl Fn(Client, Output)) {
        let done = self.call_lisp("editor-take-done!", &[]).and_then(|v| Vec::<usize>::from_value(&mut self.vm, v)).unwrap_or_default();
        for id in done {
            if let Some(reply) = self.waiting.remove(&id) {
                let _ = reply.send(Reply::Done);
            }
        }
        for client in self.clients().collect::<Vec<_>>() {
            self.serving = client;
            if self.quitting() {
                send(client, Output::Quit);
                self.detach(client);
                continue;
            }
            send(client, Output::Snapshot(Box::new(self.snapshot())));
            if greet.contains(&client) {
                send(client, Output::Bindings(self.bindings()));
            }
            if let Some(text) = self.clipboard_out() {
                send(client, Output::Clipboard(text));
            }
            if let Some(state) = self.changed_state() {
                send(client, Output::Session(state));
            }
        }
        greet.clear();
    }

    /// Text killed since last asked, for the system clipboard.
    pub fn clipboard_out(&mut self) -> Option<String> {
        let v = self.call_lisp("editor-clipboard-out", &[Arg::Session]).ok()?;
        Option::<String>::from_value(&mut self.vm, v).ok().flatten()
    }

    /// The session's state for coming back after a crash, if it changed
    /// since it was last asked for.
    pub fn changed_state(&mut self) -> Option<String> {
        let state = self.call_lisp("editor-session-state", &[Arg::Session]).and_then(|v| String::from_value(&mut self.vm, v)).ok()?;
        let a = self.clients.get_mut(&self.serving)?;
        (a.state.as_ref() != Some(&state)).then(|| {
            a.state = Some(state.clone());
            state
        })
    }

    /// Bring back a session from its state (`Output::Session`): its files
    /// are opened again, with their unsaved edits, in its panes.
    pub fn restore(&mut self, state: &str) -> Result<(), Error> {
        self.call_lisp("editor-restore!", &[Arg::Session, Arg::Str(state.to_string())]).map(drop)
    }

    /// Run background Lisp tasks for about `budget`: with none, one task's
    /// time slice.
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

    /// What the frontend acted for shows now.
    pub fn snapshot(&mut self) -> Snapshot {
        self.next_id += 1;
        let client = self.serving;
        let last = |rt: &Runtime| rt.clients.get(&client).map(|a| a.views.values().take(1).cloned().collect()).unwrap_or_default();
        let views = self.pane_views().unwrap_or_else(|_| last(self));
        if let Some(a) = self.clients.get_mut(&client) {
            a.views = views.iter().map(|v| (v.borrow().id(), v.clone())).collect();
        }
        let places = self.places().unwrap_or_default();
        let panes =
            views.iter().enumerate().map(|(i, v)| Pane { place: places.get(i).copied().unwrap_or(Place::WHOLE), ..self.pane(v) }).collect();
        let focus = self
            .call_lisp("editor-focus", &[Arg::Session])
            .and_then(|v| usize::from_value(&mut self.vm, v))
            .unwrap_or(0)
            .min(views.len().saturating_sub(1));
        let echo = self
            .call_lisp("echo-line", &[Arg::Session])
            .and_then(|v| String::from_value(&mut self.vm, v))
            .unwrap_or_else(|e| format!("echo-line: {e}"));
        let minibuffer = self.minibuffer().unwrap_or_else(|e| {
            Some(Minibuffer {
                prompt: format!("editor-minibuffer: {e} "),
                input: String::new(),
                caret: 0,
                rows: Vec::new(),
                selected: None,
                input_selected: false,
            })
        });
        let key_hints = self.key_hints().unwrap_or_default();
        let completion = self.completion().unwrap_or_default();
        let answers = self.clients.get_mut(&client).map(|a| std::mem::take(&mut a.pending)).unwrap_or_default();
        Snapshot { id: self.next_id, panes, focus, echo, minibuffer, completion, key_hints, answers }
    }

    /// The open minibuffer: Lisp gives `(prompt input-view rows selected)`
    /// or #f.
    fn minibuffer(&mut self) -> Result<Option<Minibuffer>, Error> {
        let v = self.call_lisp("editor-minibuffer", &[Arg::Session])?;
        if v.is_false() {
            return Ok(None);
        }
        let vm = &mut self.vm;
        let [prompt, view, rows, selected, input_selected] = Vec::<Value>::from_value(vm, v)?[..] else {
            return Err(Error::new("the minibuffer is (prompt view rows selected input-selected?)"));
        };
        let (prompt, rows, selected) =
            (String::from_value(vm, prompt)?, Vec::<Value>::from_value(vm, rows)?, Option::<usize>::from_value(vm, selected)?);
        let view = Foreign::<RefCell<View>>::from_value(vm, view)?;
        let (input, caret) = {
            let mut v = view.borrow_mut();
            let s = v.selection();
            let caret = s.ranges()[s.primary_index()].head;
            (v.text().rope().to_string(), caret)
        };
        let rows = rows.into_iter().map(|r| row(vm, r)).collect::<Result<Vec<_>, _>>()?;
        Ok(Some(Minibuffer {
            prompt,
            input,
            caret,
            selected: selected.filter(|&i| i < rows.len()),
            input_selected: input_selected.is_truthy(),
            rows,
        }))
    }

    /// In-buffer completion's popup: Lisp gives `(view at rows selected)`
    /// or #f.
    fn completion(&mut self) -> Result<Option<Completion>, Error> {
        let v = self.call_lisp("editor-completion", &[Arg::Session])?;
        if v.is_false() {
            return Ok(None);
        }
        let vm = &mut self.vm;
        let [view, at, rows, selected] = Vec::<Value>::from_value(vm, v)?[..] else {
            return Err(Error::new("a completion is (view at rows selected)"));
        };
        let view = Foreign::<RefCell<View>>::from_value(vm, view)?.borrow().id();
        let rows = Vec::<Value>::from_value(vm, rows)?.into_iter().map(|r| row(vm, r)).collect::<Result<Vec<_>, _>>()?;
        let selected = Option::<usize>::from_value(vm, selected)?.filter(|&i| i < rows.len());
        Ok(Some(Completion { view, at: usize::from_value(vm, at)?, rows, selected }))
    }

    /// The keys which-key shows: Lisp gives `(key description prefix?)`.
    fn key_hints(&mut self) -> Result<Vec<KeyHint>, Error> {
        let list = self.call_lisp("editor-key-hints", &[Arg::Session])?;
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
        let list = self.call_lisp("editor-pane-places", &[Arg::Session])?;
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
        let list = self.call_lisp("editor-panes", &[Arg::Session])?;
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
            .call_lisp("pane-status", &[Arg::Session, Arg::View(view.clone())])
            .and_then(|v| String::from_value(&mut self.vm, v))
            .unwrap_or_else(|e| format!("pane-status: {e}"));
        let cursor = match self.call_lisp("cursor-shape", &[Arg::Session, Arg::View(view.clone())]) {
            Ok(v) if v.is_symbol() && &*techne_vm::reader::symbol_name(v.as_symbol()) == "block" => CursorShape::Block,
            _ => CursorShape::Bar,
        };
        let text = view.borrow().text().clone();
        let len = text.len();
        let end = (scroll + LAYER_WINDOW).min(len);
        let window = [Arg::Session, Arg::View(view.clone()), Arg::Int(scroll), Arg::Int(end)];
        let layers = self.call_lisp("pane-layers", &window).and_then(|v| highlights(&mut self.vm, v)).unwrap_or_default();
        // A presentation's own faces first, the layers' over them.
        let own = match &text {
            Text::Presentation(p) => p.borrow().highlights(scroll, end),
            Text::Document(_) => Vec::new(),
        };
        let mut layers: Vec<Highlight> = own.into_iter().chain(layers).filter(|h| h.from < h.to && h.to <= len).collect();
        layers.sort_by_key(|h| h.from);
        let request =
            self.call_lisp("editor-take-request!", &[Arg::Session, Arg::View(view.clone())]).ok().and_then(|v| request(&mut self.vm, v));
        let display = self
            .call_lisp("pane-display", &[Arg::Session, Arg::View(view.clone())])
            .and_then(|v| display(&mut self.vm, v))
            .unwrap_or_default();
        Pane {
            request,
            view: id,
            revision: text.revision(),
            text: text.rope().clone(),
            selections,
            primary,
            cursor,
            scroll,
            status,
            layers,
            place: Place::WHOLE,
            display,
        }
    }

    /// Show `text` as the session's message.
    pub fn message(&mut self, text: &str) -> Result<(), Error> {
        self.call_lisp("editor-message!", &[Arg::Session, Arg::Str(text.to_string())]).map(drop)
    }

    fn call_lisp(&mut self, name: &str, args: &[Arg]) -> Result<Value, Error> {
        let f = self.vm.get_global(name).ok_or_else(|| Error::new(format!("{name} is not defined")))?;
        let f = self.vm.root(f);
        let mut values = Vec::with_capacity(args.len());
        let mut roots = Vec::new();
        for a in args {
            let v = match a {
                Arg::Session => match self.clients.get(&self.serving) {
                    Some(a) => a.session.get(),
                    None => return Err(Error::new(format!("no frontend {} is attached", self.serving.0))),
                },
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

/// A pane's display options from Lisp: `(line-numbers eob-marker)`, the
/// first #f, `absolute` or `relative`, the second a string or #f.
fn display(vm: &mut Vm, v: Value) -> Result<Display, Error> {
    let [numbers, marker] = Vec::<Value>::from_value(vm, v)?[..] else {
        return Err(Error::new("a display is (line-numbers eob-marker)"));
    };
    let line_numbers = match numbers.is_symbol().then(|| techne_vm::reader::symbol_name(numbers.as_symbol()).to_string()).as_deref() {
        Some("absolute") => LineNumbers::Absolute,
        Some("relative") => LineNumbers::Relative,
        _ => LineNumbers::Off,
    };
    Ok(Display { line_numbers, eob_marker: Option::<String>::from_value(vm, marker)? })
}

/// A view request from Lisp: `(page screens context)`, `(recenter where)`,
/// or #f.
fn request(vm: &mut Vm, v: Value) -> Option<ViewRequest> {
    let items = Vec::<Value>::from_value(vm, v).ok()?;
    let name = |v: Value| v.is_symbol().then(|| techne_vm::reader::symbol_name(v.as_symbol()).to_string());
    match (items.first().copied().and_then(name)?.as_str(), &items[1..]) {
        ("page", &[screens, context]) => {
            Some(ViewRequest::Page { screens: f64::from_value(vm, screens).ok()? as f32, context: usize::from_value(vm, context).ok()? })
        }
        ("recenter", &[at]) => Some(ViewRequest::Recenter(match name(at)?.as_str() {
            "top" => Recenter::Top,
            "bottom" => Recenter::Bottom,
            _ => Recenter::Middle,
        })),
        _ => None,
    }
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

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn the_journal_directory_is_private_also_when_it_was_not() {
        let state = tempfile::tempdir().unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let dir = journal_dir(state.path()).unwrap();
        assert_eq!(mode(&dir), 0o700);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(mode(&journal_dir(state.path()).unwrap()), 0o700);
    }
}
