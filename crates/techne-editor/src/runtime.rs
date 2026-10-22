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
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use techne_text::{Assoc, Document, Range, Recovery};
pub use techne_vm::tasks::Progress;
use techne_vm::{
    api::{Arg as Val, Foreign, FromValue, IntoValue, Value},
    vm::{Error, ExecId, Grants, InterruptHandle, Stop, Vm},
};

use crate::{
    Text, View,
    present::{
        Completion, CursorShape, Display, Highlight, Input, KeyHint, LineNumbers, Minibuffer, Output, Pane, Place, Recenter, Row, Run,
        Snapshot, ViewRequest,
    },
    server::Reply,
};

/// Where the editor's Lisp is: `editor/` in Techne's own
/// (`techne_vm::library::lisp_dir`).
pub fn lisp_dir() -> PathBuf {
    techne_vm::library::lisp_dir().join("editor")
}

/// The editor's own features that are packages (`lisp/editor/packages`), in
/// name order.
/// The editor needs them: a directory that cannot be read is an error,
/// not an editor without them.
pub fn builtin_packages() -> std::io::Result<Vec<PathBuf>> {
    let dir = lisp_dir().join("packages");
    let failed = |e: std::io::Error| std::io::Error::new(e.kind(), format!("{}: {e}", dir.display()));
    let entries =
        std::fs::read_dir(&dir).map_err(failed)?.map(|e| e.map(|e| e.path())).collect::<std::io::Result<Vec<_>>>().map_err(failed)?;
    let mut paths: Vec<PathBuf> = entries.into_iter().filter(|p| p.extension().is_some_and(|e| e == "scm")).collect();
    paths.sort();
    Ok(paths)
}

/// Load the editor's application, its interface for extensions
/// (`(techne editor)`), and its built-in packages, in that order. The
/// editor and process natives must already be installed.
pub fn load_lisp(vm: &mut Vm) -> Result<(), Error> {
    for file in ["main.scm", "api.scm"] {
        vm.eval_source(&format!("(require {:?})", lisp_dir().join(file).display().to_string()))?;
    }
    for path in builtin_packages().map_err(|e| Error::new(format!("the editor's packages: {e}")))? {
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        vm.eval_source(&format!("(load-package '{name} {:?})", path.display().to_string()))?;
    }
    Ok(())
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
    /// An input from an attached frontend, counted in its backlog until
    /// taken.
    Input(Client, Input, Admitted),
    /// Show the file at `path` in the session used last, for another
    /// program (`server`): `reply` gets `Opened`, or `Failed`, and with
    /// `wait`, `Done` once the file is done with. It is dropped after
    /// the last. `id` is the request's, unique to the process.
    Open { id: usize, path: PathBuf, wait: bool, reply: mpsc::Sender<Reply> },
    /// The program of request `id` stopped waiting (`Open`).
    Gone(usize),
    /// A background task woke. One is queued at a time, however many
    /// tasks wake before the runtime takes it.
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
    /// How long a call into Lisp for a part of a snapshot may run
    /// (`call`).
    deadline: Duration,
    watch: crate::deadline::Watch,
    /// The procedures that ran past their deadline, by name: not called
    /// again until redefined.
    stalled: HashMap<String, Value>,
    /// Errors no command raised, since `take_errors`: of an input outside
    /// its command, of a part of a snapshot, of listing the bindings.
    errors: Vec<String>,
}

/// How long a call into Lisp between inputs (a part of a snapshot, the
/// session's state, whether it quit) may run before it is killed: a
/// safety net against a procedure that does not end, not the keystroke
/// budget. Ten times that in a debug build.
pub const PART_DEADLINE: Duration = Duration::from_millis(if cfg!(debug_assertions) { 2500 } else { 250 });

/// How long the calls into Lisp that open files may run before they are
/// killed: starting a session, restoring one, opening a file for another
/// program. Ten times that in a debug build.
pub const OPEN_DEADLINE: Duration = Duration::from_millis(if cfg!(debug_assertions) { 100_000 } else { 10_000 });

/// How long a batch of inputs may take before the runtime answers with
/// snapshots, taking the inputs still queued after: a flood of inputs
/// does not keep the frontends from being drawn.
const BATCH: Duration = Duration::from_millis(50);

/// The nursery window of the editor's world (`Vm::set_nursery_window`):
/// minor collections short enough for interaction, at some throughput
/// (runtime/TECHNE-VM.md, "Results").
const NURSERY_WINDOW: usize = 2 << 20;

/// The errors no command raised a runtime keeps, at most: all a session
/// that never asks for them holds.
const ERRORS_MAX: usize = 64;

/// The error of a call into Lisp that was not made: its procedure ran past
/// its deadline before and has not been redefined since.
const STALLED: &str = "ran past its deadline; not called until redefined";

/// The call was killed at its deadline, or not made because it was before.
fn stalled(e: &Error) -> bool {
    e.is_kill() || e.msg == STALLED
}

/// What a part of a snapshot is: its value, its last good value if its
/// call stalled, else what `failed` makes of the error.
fn part<T>(result: Result<T, Error>, last: Option<T>, failed: impl FnOnce(Error) -> T) -> T {
    match result {
        Ok(v) => v,
        Err(e) if stalled(&e) && last.is_some() => last.expect("checked"),
        Err(e) => failed(e),
    }
}

/// A frontend attached, and its session.
struct Attached {
    session: Value,
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
    /// The last snapshot sent: a part whose call stalled keeps its value.
    last: Option<Snapshot>,
    /// Where its host counts the inputs it sends: those it refused are
    /// reported as it is answered. Known once one of its inputs is taken.
    backlog: Option<Arc<Backlog>>,
}

impl Attached {
    /// The runtime took `input`: the next snapshot answers it, whether it
    /// was handled or not (discarded by C-g, or its session failed), so
    /// the frontend never waits on it.
    fn taken(&mut self, input: &Input) {
        if let Input::Key { at, .. } | Input::Click { at, .. } = input {
            self.pending.push(*at);
        }
    }
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

/// What a frontend sent that its runtime has not taken yet: the inputs and
/// their bytes, which the host bounds (`host::Host::send`).
#[derive(Debug, Default)]
pub struct Backlog {
    inputs: AtomicUsize,
    bytes: AtomicUsize,
    /// Inputs refused since the runtime last reported them.
    discarded: AtomicUsize,
}

impl Backlog {
    /// The inputs, and their bytes.
    pub fn len(&self) -> (usize, usize) {
        (self.inputs.load(Ordering::Acquire), self.bytes.load(Ordering::Acquire))
    }

    /// Count `input`, until the `Admitted` returned is dropped (the runtime
    /// took it, or ended).
    pub fn admit(self: &Arc<Self>, input: &Input) -> Admitted {
        let bytes = weight(input);
        self.inputs.fetch_add(1, Ordering::AcqRel);
        self.bytes.fetch_add(bytes, Ordering::AcqRel);
        Admitted { backlog: self.clone(), bytes }
    }

    /// Count an input refused, which the runtime reports when it next
    /// answers the frontend (`take_discarded`).
    pub fn discard(&self) {
        self.discarded.fetch_add(1, Ordering::AcqRel);
    }

    /// The inputs refused since this was last asked.
    pub fn take_discarded(&self) -> usize {
        self.discarded.swap(0, Ordering::AcqRel)
    }
}

/// An input counted in its frontend's backlog (`Backlog::admit`).
#[derive(Debug)]
pub struct Admitted {
    backlog: Arc<Backlog>,
    bytes: usize,
}

impl Drop for Admitted {
    fn drop(&mut self) {
        self.backlog.inputs.fetch_sub(1, Ordering::AcqRel);
        self.backlog.bytes.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

/// The bytes an input holds while queued: what it allocated, spare
/// capacity included.
pub(crate) fn weight(input: &Input) -> usize {
    std::mem::size_of::<Input>()
        + match input {
            Input::Key { key, .. } => key.capacity(),
            Input::Unsendable { keys } => {
                keys.capacity() * std::mem::size_of::<String>() + keys.iter().map(String::capacity).sum::<usize>()
            }
            Input::Unrecognized { input: text } | Input::Clipboard { text } => text.capacity(),
            Input::Click { .. } | Input::Scroll { .. } | Input::Edge { .. } | Input::Close => 0,
        }
}

/// The Lisp procedures the runtime calls, by name: each is looked up when
/// it is called, so redefining one while running takes effect at once.
const PROCS: [&str; 29] = [
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
    "editor-take-violations!",
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
        Runtime::with_document_granted(doc, profile, Grants::ALL)
    }

    /// As `with_document`, the runtime's world granted only `grants`: what
    /// it is not granted its natives refuse (a fuzzer's runtime neither
    /// writes files nor starts processes).
    pub fn with_document_granted(doc: Document, profile: &str, grants: Grants) -> Result<Runtime, Error> {
        let mut runtime = Runtime::granted(grants)?;
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
        Runtime::granted(Grants::ALL)
    }

    /// A runtime whose world is granted only `grants`.
    pub fn granted(grants: Grants) -> Result<Runtime, Error> {
        let documents = crate::Documents::default();
        let mut vm = Vm::with_grants(grants);
        crate::install_with_documents(&mut vm, documents.clone());
        techne_process::install(&mut vm)?;
        techne_net::install(&mut vm)?;
        load_lisp(&mut vm)?;
        for name in PROCS {
            if vm.get_global(name).is_none() {
                return Err(Error::new(format!("main.scm does not define {name}")));
            }
        }
        // Loaded with the default window, for throughput; interactive
        // from here.
        vm.set_nursery_window(NURSERY_WINDOW);
        let watch = crate::deadline::Watch::new(vm.interrupt_handle());
        Ok(Runtime {
            vm,
            documents,
            doc: None,
            clients: BTreeMap::new(),
            serving: Client::FIRST,
            next_id: 0,
            waiting: HashMap::new(),
            deadline: PART_DEADLINE,
            watch,
            stalled: HashMap::new(),
            errors: Vec::new(),
        })
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
        let view = view.map(|v| Rc::new(RefCell::new(v)));
        let args = [view.clone().map_or(Arg::Bool(false), Arg::View), Arg::Str(profile.to_string())];
        let session = self.call_within("start-session", &args, OPEN_DEADLINE)?;
        let views = view.map(|v| {
            let id = v.borrow().id();
            (id, v)
        });
        let views = views.into_iter().collect();
        let attached =
            Attached { session, views, pending: Vec::new(), state: None, interrupts: Arc::default(), next: 0, last: None, backlog: None };
        self.clients.insert(client, attached);
        self.serving = client;
        Ok(())
    }

    /// Detach `client`: its session ends; the buffers it showed stay.
    pub fn detach(&mut self, client: Client) {
        if self.clients.contains_key(&client) {
            self.serving = client;
            if let Err(e) = self.call("editor-detach!", &[Arg::Session]) {
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
        self.call("editor-select!", &[Arg::Session]).map(drop)
    }

    /// Handle one input of the frontend acted for. Errors in Lisp outside
    /// a command become the session's message rather than ending the
    /// session. Returns `Output::Quit` when the session quits.
    pub fn handle(&mut self, input: Input) -> Option<Output> {
        if let Some(a) = self.clients.get_mut(&self.serving) {
            a.taken(&input);
        }
        let exec = self.vm.new_execution();
        self.handle_as(exec, input, false)
    }

    /// Handle `input` as the execution `exec` (`Vm::new_execution`), which
    /// stops reach (`Interrupts`), the session acted for selected first
    /// with `select`; its error, if any, is shown after it.
    fn handle_as(&mut self, exec: ExecId, input: Input, select: bool) -> Option<Output> {
        if matches!(input, Input::Close) {
            self.vm.discard_execution(exec);
            return Some(Output::Quit);
        }
        self.vm.enter_execution(exec);
        let selected = if select { self.call("editor-select!", &[Arg::Session]).map(drop) } else { Ok(()) };
        let result = selected.and_then(|()| self.handle_input(input));
        self.vm.leave_execution(exec);
        if let Err(e) = result {
            if !e.is_kill() && !e.is_interrupt() {
                self.note_error(format!("input: {e}"));
            }
            let _ = self.message(&if e.is_kill() { "Quit (killed)".to_string() } else { e.to_string() });
        }
        self.quitting().then_some(Output::Quit)
    }

    fn handle_input(&mut self, input: Input) -> Result<(), Error> {
        let views = self.clients.get(&self.serving).map(|a| a.views.clone()).unwrap_or_default();
        match input {
            Input::Key { key, .. } => self.call("editor-press", &[Arg::Session, Arg::Str(key)]).map(drop),
            Input::Click { view, revision, pos, extend, .. } => {
                // Re-resolve a click on an older snapshot; refuse one whose
                // text is gone rather than apply it to other text.
                match views.get(&view).cloned() {
                    None => self.message("The pane clicked on is gone"),
                    Some(v) => {
                        let mapped = v.borrow().text().map_pos(pos, Assoc::Before, revision);
                        match mapped {
                            Some((p, false)) => {
                                let args = [Arg::Session, Arg::View(v), Arg::Int(p), Arg::Bool(extend)];
                                self.call("editor-click", &args).map(drop)
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
                        (Ok(()), Some((p, _))) => self.call("editor-paged!", &[Arg::Session, Arg::View(v), Arg::Int(p)]).map(drop),
                        (result, _) => result,
                    }
                }
                None => Ok(()),
            },
            Input::Edge { view, end } => match views.get(&view) {
                Some(_) => self.message(if end { "End of buffer" } else { "Beginning of buffer" }),
                None => Ok(()),
            },
            Input::Unsendable { keys } => self.call("editor-unsendable!", &[Arg::Session, Arg::Strs(keys)]).map(drop),
            Input::Unrecognized { input } => self.message(&format!("Unrecognized input: {input}")),
            Input::Clipboard { text } => self.call("editor-clipboard!", &[Arg::Session, Arg::Str(text)]).map(drop),
            Input::Close => Ok(()),
        }
    }

    /// Whether the session acted for has been asked to end.
    fn quitting(&mut self) -> bool {
        self.call("session-quit?", &[Arg::Session]).is_ok_and(|v| v.is_truthy())
    }

    /// The key sequences the session binds, in Emacs notation, sorted.
    pub fn bindings(&mut self) -> Vec<String> {
        let keys = self.call("bound-keys", &[Arg::Session]).and_then(|v| self.vm.get(&v));
        let mut keys: Vec<String> = match keys {
            Ok(keys) => keys,
            Err(e) => {
                self.note_error(format!("bound-keys: {e}"));
                Vec::new()
            }
        };
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
        // A task waking queues `Wake` unless one is queued already: wakes
        // as often as output comes do not pile up.
        let woken = Arc::new(AtomicBool::new(false));
        let theirs = woken.clone();
        self.vm.set_wake_notifier(move || {
            if !theirs.swap(true, Ordering::AcqRel) {
                let _ = wake.send(Msg::Wake);
            }
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
            // The inputs queued, for `BATCH` at most: those after wait for
            // the next batch, the frontends answered between.
            let batch = Instant::now() + BATCH;
            let queued = first.into_iter().chain(std::iter::from_fn(|| (Instant::now() < batch).then(|| inputs.try_recv().ok()).flatten()));
            for msg in queued {
                match msg {
                    Msg::Wake => woken.store(false, Ordering::Release),
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
                            let _ = self.call("editor-forget!", &[Arg::Int(id)]);
                        }
                    }
                    Msg::Input(client, input, admitted) => {
                        let Some(a) = self.clients.get_mut(&client) else { continue };
                        // What it refused is reported when it is answered.
                        a.backlog.get_or_insert_with(|| admitted.backlog.clone());
                        // Taken: out of the frontend's backlog.
                        drop(admitted);
                        if matches!(input, Input::Close) {
                            self.detach(client);
                            send(client, Output::Quit);
                            continue;
                        }
                        a.taken(&input);
                        let (k, interrupts) = (a.next, a.interrupts.clone());
                        a.next += 1;
                        let exec = self.vm.new_execution();
                        self.serving = client;
                        if interrupts.start(k, exec) {
                            // Selected within the input: C-g reaches it.
                            self.handle_as(exec, input, true);
                            interrupts.end();
                        } else {
                            self.vm.discard_execution(exec);
                        }
                    }
                }
            }
            progress = self.run_tasks(Duration::ZERO);
            // A native panicked: the world refuses to run anything more,
            // answering the frontends included. End, as a crash would, for
            // the host to start another runtime that replays the journals.
            if self.vm.poisoned().is_some() {
                return;
            }
            self.answer(&mut greet, &send);
        }
    }

    /// Show the file at `path` for another program (`Msg::Open`).
    fn open_for(&mut self, id: usize, path: &Path, wait: bool, reply: mpsc::Sender<Reply>) {
        let id = wait.then_some(id);
        let args = [Arg::Str(path.display().to_string()), id.map_or(Arg::Bool(false), Arg::Int)];
        match self.call_within("editor-open!", &args, OPEN_DEADLINE) {
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
        let done = self.call("editor-take-done!", &[]).and_then(|v| self.vm.get::<Vec<usize>>(&v)).unwrap_or_default();
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
            let discarded = self.clients.get(&client).and_then(|a| a.backlog.as_ref()).map_or(0, |b| b.take_discarded());
            if discarded > 0 {
                let _ = self.message(&format!("Discarded {discarded} inputs sent while the editor was busy"));
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
        let v = self.call("editor-clipboard-out", &[Arg::Session]).ok()?;
        self.vm.get::<Option<String>>(&v).ok().flatten()
    }

    /// The session's state for coming back after a crash, if it changed
    /// since it was last asked for.
    pub fn changed_state(&mut self) -> Option<String> {
        let state = self.call("editor-session-state", &[Arg::Session]).and_then(|v| self.vm.get::<String>(&v)).ok()?;
        let a = self.clients.get_mut(&self.serving)?;
        (a.state.as_ref() != Some(&state)).then(|| {
            a.state = Some(state.clone());
            state
        })
    }

    /// Bring back a session from its state (`Output::Session`): its files
    /// are opened again, with their unsaved edits, in its panes.
    pub fn restore(&mut self, state: &str) -> Result<(), Error> {
        self.call_within("editor-restore!", &[Arg::Session, Arg::Str(state.to_string())], OPEN_DEADLINE).map(drop)
    }

    /// Run background Lisp tasks for about `budget`: with none, one task's
    /// time slice.
    pub fn run_tasks(&mut self, budget: Duration) -> Progress {
        self.vm.run_tasks_for(budget)
    }

    /// Evaluate source in the user module, as the session's code would (for
    /// hosts and tests); the result as `write` shows it.
    pub fn eval(&mut self, source: &str) -> Result<String, Error> {
        self.vm.eval_source(source).map(|v| self.vm.repr(&v))
    }

    /// Bytes the world holds (`techne_vm::vm::Vm::held`).
    pub fn held(&self) -> usize {
        self.vm.held().total()
    }

    /// Set the most bytes the world is to hold (`Vm::set_memory_limit`).
    pub fn set_memory_limit(&mut self, bytes: usize) {
        self.vm.set_memory_limit(bytes);
    }

    /// Start a Lisp task from source evaluating to a procedure of no
    /// arguments (used to put the editor under load).
    pub fn spawn(&mut self, source: &str) -> Result<(), Error> {
        let f = self.vm.eval_source(source)?;
        self.vm.try_spawn(&f).map(drop)
    }

    /// Grant or revoke `cap`, one checked when used (`Vm::set_grant`):
    /// a fuzzer revokes loading once the editor has loaded.
    pub fn set_grant(&mut self, cap: techne_vm::vm::Capability, granted: bool) {
        self.vm.set_grant(cap, granted);
    }

    /// Read no file outside `dir` in the world (`Vm::confine_reading`).
    pub fn confine_reading(&mut self, dir: &Path) -> Result<(), Error> {
        self.vm.confine_reading(dir)
    }

    /// What the frontend acted for shows now.
    pub fn snapshot(&mut self) -> Snapshot {
        self.next_id += 1;
        let client = self.serving;
        let previous = self.clients.get_mut(&client).and_then(|a| a.last.take());
        let last = previous.as_ref();
        // Failing that, the last snapshot's panes, in order.
        let last_views = |rt: &Runtime| -> Vec<Rc<RefCell<View>>> {
            let Some(a) = rt.clients.get(&client) else { return Vec::new() };
            let ordered: Vec<_> = last.into_iter().flat_map(|s| &s.panes).filter_map(|p| a.views.get(&p.view).cloned()).collect();
            if ordered.is_empty() { a.views.values().take(1).cloned().collect() } else { ordered }
        };
        let views = self.pane_views();
        let views = self.noted("editor-panes", views).unwrap_or_else(|_| last_views(self));
        if let Some(a) = self.clients.get_mut(&client) {
            a.views = views.iter().map(|v| (v.borrow().id(), v.clone())).collect();
        }
        let places = self.places();
        let places =
            part(self.noted("editor-pane-places", places), last.map(|s| s.panes.iter().map(|p| p.place).collect()), |_| Vec::new());
        let panes = views
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let id = v.borrow().id();
                let was = last.and_then(|s| s.panes.iter().find(|p| p.view == id));
                Pane { place: places.get(i).copied().unwrap_or(Place::WHOLE), ..self.pane(v, was) }
            })
            .collect();
        let focus = self.call("editor-focus", &[Arg::Session]).and_then(|v| self.vm.get::<usize>(&v));
        let focus = part(self.noted("editor-focus", focus), last.map(|s| s.focus), |_| 0).min(views.len().saturating_sub(1));
        let echo = self.call("echo-line", &[Arg::Session]).and_then(|v| self.vm.get::<String>(&v));
        let echo = part(self.noted("echo-line", echo), last.map(|s| s.echo.clone()), |e| format!("echo-line: {e}"));
        let minibuffer = self.minibuffer();
        let minibuffer = part(self.noted("editor-minibuffer", minibuffer), last.map(|s| s.minibuffer.clone()), |e| {
            Some(Minibuffer {
                prompt: format!("editor-minibuffer: {e} "),
                input: String::new(),
                caret: 0,
                rows: Arc::default(),
                selected: None,
                input_selected: false,
            })
        });
        let key_hints = self.key_hints();
        let key_hints = part(self.noted("editor-key-hints", key_hints), last.map(|s| s.key_hints.clone()), |_| Vec::new());
        let completion = self.completion();
        let completion = part(self.noted("editor-completion", completion), last.map(|s| s.completion.clone()), |_| None);
        let answers = self.clients.get_mut(&client).map(|a| std::mem::take(&mut a.pending)).unwrap_or_default();
        // Under memory pressure the echo area says so, which needs no Lisp.
        let shown = match self.vm.pressure() {
            Some(held) => {
                let note = format!("Out of memory: {} MB held, past the limit of {} MB", held.total() >> 20, self.vm.memory_limit() >> 20);
                if echo.is_empty() { note } else { format!("{echo} ({note})") }
            }
            None => echo.clone(),
        };
        let snapshot = Snapshot { id: self.next_id, panes, focus, echo: shown, minibuffer, completion, key_hints, answers };
        if let Some(a) = self.clients.get_mut(&client) {
            a.last = Some(Snapshot { answers: Vec::new(), echo, ..snapshot.clone() });
        }
        snapshot
    }

    /// The open minibuffer: Lisp gives `(prompt input-view rows selected)`
    /// or #f.
    fn minibuffer(&mut self) -> Result<Option<Minibuffer>, Error> {
        let v = self.call("editor-minibuffer", &[Arg::Session])?;
        if v.is_false() {
            return Ok(None);
        }
        let (prompt, view, rows, selected, input_selected) = decode(&mut self.vm, &v, |v| {
            let [prompt, view, rows, selected, input_selected] = list(v)?[..] else {
                return Err(Error::new("the minibuffer is (prompt view rows selected input-selected?)"));
            };
            let rows = list(rows)?.into_iter().map(row).collect::<Result<Vec<_>, _>>()?;
            Ok((
                String::from_arg(prompt)?,
                Foreign::<RefCell<View>>::from_arg(view)?,
                rows,
                Option::<usize>::from_arg(selected)?,
                input_selected.is_truthy(),
            ))
        })?;
        let (input, caret) = {
            let mut v = view.borrow_mut();
            let s = v.selection();
            let caret = s.ranges()[s.primary_index()].head;
            (v.text().rope().to_string(), caret)
        };
        Ok(Some(Minibuffer { prompt, input, caret, selected: selected.filter(|&i| i < rows.len()), input_selected, rows: rows.into() }))
    }

    /// In-buffer completion's popup: Lisp gives `(view at rows selected)`
    /// or #f.
    fn completion(&mut self) -> Result<Option<Completion>, Error> {
        let v = self.call("editor-completion", &[Arg::Session])?;
        if v.is_false() {
            return Ok(None);
        }
        decode(&mut self.vm, &v, |v| {
            let [view, at, rows, selected] = list(v)?[..] else {
                return Err(Error::new("a completion is (view at rows selected)"));
            };
            let view = Foreign::<RefCell<View>>::from_arg(view)?.borrow().id();
            let rows = list(rows)?.into_iter().map(row).collect::<Result<Vec<_>, _>>()?;
            let selected = Option::<usize>::from_arg(selected)?.filter(|&i| i < rows.len());
            Ok(Some(Completion { view, at: usize::from_arg(at)?, rows: rows.into(), selected }))
        })
    }

    /// The keys which-key shows: Lisp gives `(key description prefix?)`.
    fn key_hints(&mut self) -> Result<Vec<KeyHint>, Error> {
        let list = self.call("editor-key-hints", &[Arg::Session])?;
        decode(&mut self.vm, &list, |hints| {
            self::list(hints)?
                .into_iter()
                .map(|h| match self::list(h)?[..] {
                    [key, description, prefix] => {
                        Ok(KeyHint { key: String::from_arg(key)?, description: String::from_arg(description)?, prefix: prefix.is_truthy() })
                    }
                    _ => Err(Error::new("a key hint is (key description prefix?)")),
                })
                .collect()
        })
    }

    /// Where the panes are: Lisp gives `(x y w h)` for each.
    fn places(&mut self) -> Result<Vec<Place>, Error> {
        let list = self.call("editor-pane-places", &[Arg::Session])?;
        self.vm
            .get::<Vec<Vec<f64>>>(&list)?
            .into_iter()
            .map(|p| match p[..] {
                [x, y, w, h] => Ok(Place { x: x as f32, y: y as f32, w: w as f32, h: h as f32 }),
                _ => Err(Error::new("a place is (x y w h)")),
            })
            .collect()
    }

    /// The views the session shows, in order.
    fn pane_views(&mut self) -> Result<Vec<Rc<RefCell<View>>>, Error> {
        let list = self.call("editor-panes", &[Arg::Session])?;
        let views = self.vm.get::<Vec<Foreign<RefCell<View>>>>(&list)?;
        if views.is_empty() {
            return Err(Error::new("editor-panes: no panes"));
        }
        Ok(views.into_iter().map(|v| v.0).collect())
    }

    /// The pane of `view`, its parts whose call stalled as they were in
    /// `was`, its pane in the last snapshot.
    fn pane(&mut self, view: &Rc<RefCell<View>>, was: Option<&Pane>) -> Pane {
        let (id, selections, primary, scroll) = {
            let mut v = view.borrow_mut();
            let s = v.selection();
            let selections = s.ranges().iter().map(|r: &Range| (r.anchor, r.head)).collect();
            let primary = s.primary_index();
            (v.id(), selections, primary, v.scroll())
        };
        let status = self.call("pane-status", &[Arg::Session, Arg::View(view.clone())]).and_then(|v| self.vm.get::<String>(&v));
        let status = part(self.noted("pane-status", status), was.map(|p| p.status.clone()), |e| format!("pane-status: {e}"));
        let cursor = self.call("cursor-shape", &[Arg::Session, Arg::View(view.clone())]).map(|v| {
            match v.symbol_name().as_deref() == Some("block") {
                true => CursorShape::Block,
                false => CursorShape::Bar,
            }
        });
        let cursor = part(self.noted("cursor-shape", cursor), was.map(|p| p.cursor), |_| CursorShape::Bar);
        let text = view.borrow().text().clone();
        let len = text.len();
        let end = (scroll + LAYER_WINDOW).min(len);
        let window = [Arg::Session, Arg::View(view.clone()), Arg::Int(scroll), Arg::Int(end)];
        let layers = self.call("pane-layers", &window).and_then(|v| decode(&mut self.vm, &v, highlights));
        let layers = match self.noted("pane-layers", layers) {
            // Last good layers are for the text as it was: kept only for it.
            Err(e) if stalled(&e) && was.is_some_and(|p| p.revision == text.revision() && p.scroll == scroll) => {
                was.expect("checked").layers.clone()
            }
            found => {
                // A presentation's own faces first, the layers' over them.
                let own = match &text {
                    Text::Presentation(p) => p.borrow().highlights(scroll, end),
                    Text::Document(_) => Vec::new(),
                };
                let mut layers: Vec<Highlight> =
                    own.into_iter().chain(found.unwrap_or_default()).filter(|h| h.from < h.to && h.to <= len).collect();
                layers.sort_by_key(|h| h.from);
                layers
            }
        };
        let request =
            self.call("editor-take-request!", &[Arg::Session, Arg::View(view.clone())]).and_then(|v| self.vm.inspect(&v, request));
        let request = self.noted("editor-take-request!", request).ok().flatten();
        let display = self.call("pane-display", &[Arg::Session, Arg::View(view.clone())]).and_then(|v| decode(&mut self.vm, &v, display));
        let display = part(self.noted("pane-display", display), was.map(|p| p.display.clone()), |_| Default::default());
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
        self.call("editor-message!", &[Arg::Session, Arg::Str(text.to_string())]).map(drop)
    }

    /// Call the Lisp procedure `name`. Within an input, as the input's
    /// execution, which C-g stops. Between inputs (a part of a snapshot,
    /// the session's state, whether it quit), as an execution of its own,
    /// killed past the runtime's deadline (`PART_DEADLINE`): then its
    /// procedure is not called again until it is redefined, and *Messages*
    /// says so; the snapshot keeps that part as it last was. So nothing the
    /// runtime calls between inputs can keep it from the next.
    fn call(&mut self, name: &str, args: &[Arg]) -> Result<Value, Error> {
        self.call_within(name, args, self.deadline)
    }

    /// `call`, killed between inputs past `deadline`.
    fn call_within(&mut self, name: &str, args: &[Arg], deadline: Duration) -> Result<Value, Error> {
        if self.vm.running_execution() != ExecId(0) {
            return self.call_now(name, args);
        }
        let f = self.vm.get_global(name).ok_or_else(|| Error::new(format!("{name} is not defined")))?;
        match self.stalled.get(name) {
            Some(g) if *g == f => return Err(Error::new(STALLED)),
            Some(_) => drop(self.stalled.remove(name)),
            None => {}
        }
        let exec = self.vm.new_execution();
        self.watch.arm(exec, Instant::now() + deadline);
        self.vm.enter_execution(exec);
        let result = self.call_now(name, args);
        self.vm.leave_execution(exec);
        if result.as_ref().is_err_and(Error::is_kill) {
            self.stalled.insert(name.to_string(), f);
            // With a deadline too: a reporter that stalls is not called
            // again either.
            let text = format!("{name} {STALLED} ({} ms)", deadline.as_millis());
            let _ = self.call("editor-message!", &[Arg::Session, Arg::Str(text)]);
        }
        result
    }

    /// `result`, its error kept for `take_errors` as the failure of the
    /// snapshot's part `what`; but for a call not made because it stalled
    /// before, whose kill was kept then.
    fn noted<T>(&mut self, what: &str, result: Result<T, Error>) -> Result<T, Error> {
        if let Err(e) = &result
            && e.msg != STALLED
        {
            self.note_error(format!("{what}: {}", if e.is_kill() { "killed at its deadline" } else { &e.msg }));
        }
        result
    }

    /// Keep `error` for `take_errors`, the latest `ERRORS_MAX` of them.
    fn note_error(&mut self, error: String) {
        if self.errors.len() == ERRORS_MAX {
            self.errors.remove(0);
        }
        self.errors.push(error);
    }

    /// What went wrong outside commands since the last call, and the
    /// violations commands and hooks raised (`take-violations!`): mistakes
    /// in the editor, which a command reporting a failure is not.
    pub fn take_errors(&mut self) -> Vec<String> {
        let violations = self.call("editor-take-violations!", &[Arg::Session]).and_then(|v| self.vm.get::<Vec<String>>(&v));
        let violations = violations.unwrap_or_else(|e| vec![format!("editor-take-violations!: {e}")]);
        std::mem::take(&mut self.errors).into_iter().chain(violations).collect()
    }

    /// Change how long a call into Lisp for a part of a snapshot may run
    /// (`PART_DEADLINE`).
    pub fn set_part_deadline(&mut self, deadline: Duration) {
        self.deadline = deadline;
    }

    /// Call the Lisp procedure `name` as what runs now: only `call_within`
    /// does, which bounds it.
    fn call_now(&mut self, name: &str, args: &[Arg]) -> Result<Value, Error> {
        let f = self.vm.get_global(name).ok_or_else(|| Error::new(format!("{name} is not defined")))?;
        let mut values = Vec::with_capacity(args.len());
        for a in args {
            let v = match a {
                Arg::Session => match self.clients.get(&self.serving) {
                    Some(a) => a.session.clone(),
                    None => return Err(Error::new(format!("no frontend {} is attached", self.serving.0))),
                },
                Arg::Str(s) => s.as_str().into_value(&mut self.vm)?,
                Arg::Int(n) => (*n).into_value(&mut self.vm)?,
                Arg::Bool(b) => (*b).into_value(&mut self.vm)?,
                Arg::Strs(v) => v.clone().into_value(&mut self.vm)?,
                Arg::View(v) => Foreign(v.clone()).into_value(&mut self.vm)?,
            };
            values.push(v);
        }
        self.vm.call(&f, &values)
    }
}

/// Decode `v` with `f`, looking at it in place.
fn decode<T>(vm: &mut Vm, v: &Value, f: impl FnOnce(Val<'_>) -> Result<T, Error>) -> Result<T, Error> {
    vm.inspect(v, f)?
}

/// The elements of a list (or a vector).
fn list(v: Val<'_>) -> Result<Vec<Val<'_>>, Error> {
    v.items().ok_or_else(|| v.type_error("list"))
}

fn name(v: Val<'_>) -> Option<String> {
    v.symbol_name().map(|s| s.to_string())
}

/// Highlights from Lisp: a list of `(from to face)`, the face a symbol.
fn highlights(v: Val<'_>) -> Result<Vec<Highlight>, Error> {
    list(v)?
        .into_iter()
        .map(|h| match list(h)?[..] {
            [from, to, face] if face.is_symbol() => {
                Ok(Highlight { from: usize::from_arg(from)?, to: usize::from_arg(to)?, face: name(face).expect("a symbol") })
            }
            _ => Err(Error::new("a highlight is (from to face)")),
        })
        .collect()
}

/// A pane's display options from Lisp: `(line-numbers eob-marker)`, the
/// first #f, `absolute` or `relative`, the second a string or #f.
fn display(v: Val<'_>) -> Result<Display, Error> {
    let [numbers, marker] = list(v)?[..] else {
        return Err(Error::new("a display is (line-numbers eob-marker)"));
    };
    let line_numbers = match name(numbers).as_deref() {
        Some("absolute") => LineNumbers::Absolute,
        Some("relative") => LineNumbers::Relative,
        _ => LineNumbers::Off,
    };
    Ok(Display { line_numbers, eob_marker: Option::<String>::from_arg(marker)? })
}

/// A view request from Lisp: `(page screens context)`, `(recenter where)`,
/// or #f.
fn request(v: Val<'_>) -> Option<ViewRequest> {
    let items = list(v).ok()?;
    match (name(*items.first()?)?.as_str(), &items[1..]) {
        ("page", &[screens, context]) => {
            Some(ViewRequest::Page { screens: f64::from_arg(screens).ok()? as f32, context: usize::from_arg(context).ok()? })
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
fn row(v: Val<'_>) -> Result<Row, Error> {
    let run = |r: Val<'_>| -> Result<Run, Error> {
        if let Some(text) = r.as_str() {
            return Ok(Run { text: text.to_owned(), face: None });
        }
        match list(r)?[..] {
            [text, face] => Ok(Run { text: String::from_arg(text)?, face: name(face) }),
            _ => Err(Error::new("a run is a string or (text face)")),
        }
    };
    let columns = list(v)?
        .into_iter()
        .map(|c| if c.as_str().is_some() { run(c).map(|r| vec![r]) } else { list(c)?.into_iter().map(run).collect() })
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
