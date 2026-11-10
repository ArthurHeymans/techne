//! `techne cloud dev FILE`: a deployment served on a local port as the
//! service serves it (doc/serverless.md, milestone 2), without accounts.
//!
//! FILE is the deployment's entry module, which provides `fetch`. This
//! process is the supervisor (`techne_host::supervisor`): the front,
//! `lisp/http`'s server in a world of its own, hands each request with
//! `host-fetch` to a worker process (`techne cloud worker`), sandboxed
//! where the kernel allows it and in a cgroup of its own where one can be
//! had, which loads the deployment from FILE's directory's files, sent to
//! it, as `techne-host` loads one, and runs each request in an instance
//! started from its snapshot, within the default limits. A worker that a
//! native holds past a kill, or that dies, is replaced, and given the
//! deployment again. Saving any `.scm` or `.sld` file under
//! FILE's directory (but in hidden directories, `target` and
//! `node_modules`) deploys again: requests from then on go to the new
//! deployment; one that fails to load is reported and the one before keeps
//! serving, also while the new one loads. Failures and what requests print
//! go to standard error, marked with the request.
//!
//! With `--nrepl PORT`, the deployment is served instead by one world
//! kept across requests, its dev world (`Host::publish_dev`) in the
//! worker, with an nREPL server in it ([`DevWorker`]), whose clients the
//! supervisor accepts on 127.0.0.1:PORT (written to `.nrepl-port` beside
//! FILE, where Emacs's `techne-connect` finds it) and relays to it: what a client evaluates, in
//! FILE's module unless it names another, changes the next request. A
//! deployment deployed again on save starts a new dev world, and the
//! server moves to it: clients stay connected, their sessions now in the
//! new world, what they were running there failed. The dev world has the
//! deployment's grants, no more: it reads no files nor loads modules not
//! loaded already.
//!
//! A request failing in the dev world breaks into the debugger of the
//! client last connected (Emacs's `techne-connect` subscribes), where it
//! failed, and waits: fix `fetch` or what it calls and `retry`, which
//! serves the request again; `use-value` answers it with a response
//! given; abort answers it 500. Other requests wait meanwhile (the dev
//! world serves one at a time), up to its deadline, a minute
//! (`DEV_WALL`) rather than 30 seconds.
//!
//! With `--debug` too, the deployment is served as in production, by
//! instances started from its snapshot, and the nREPL server is in a
//! debug instance beside them (`Host::attach_debug`): a world from the
//! same snapshot that serves only the requests routed to it, those
//! carrying the header `techne-debug-route` with the credential printed at
//! the start (`lisp/http/debug-route.scm`, in the front). What a client
//! evaluates changes the debug instance alone; a routed request that
//! fails breaks into the debugger as above, while the others are served
//! from the code as deployed, within the usual 30 seconds. A response
//! from the debug instance says so (`techne-debug`). Deployed again on
//! save, the deployment has a new debug instance, and the server moves to
//! it.

use std::{
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

use std::{cell::RefCell, rc::Rc};

use techne_host::{Attached, Deployment, Host, Limits};
use techne_node::nrepl::Server;
use techne_vm::vm::{Error, ROOT_MODULE, Vm};

const USAGE: &str = "usage: techne cloud dev FILE [--port PORT] [--nrepl PORT [--debug]]

Serve the deployment whose entry module is FILE on 127.0.0.1:PORT (8787
by default; 0 picks one), deploying again whenever a Lisp file under
FILE's directory changes. With --nrepl, serve it from a world kept across
requests, with an nREPL server into it on that port. With --debug too,
serve it as in production, and put the nREPL server in a debug instance
beside, which serves the requests carrying the header techne-debug-route
with the credential printed.";

/// How long a request may take in the dev world, paused in a debugger
/// say.
const DEV_WALL: Duration = Duration::from_secs(60);

/// The name the deployment is published under.
const NAME: &str = "dev";

/// How often files are looked at for changes.
const POLL: Duration = Duration::from_millis(250);

pub fn main(args: Vec<String>) -> ExitCode {
    match args.split_first() {
        // First, before anything makes a thread: it sandboxes itself.
        Some((worker, rest)) if worker == "worker" => {
            let fd = rest.iter().position(|a| a == "--fd").and_then(|i| rest.get(i + 1)).and_then(|f| f.parse().ok());
            match fd {
                Some(fd) => {
                    let dev = DevWorker { debug: rest.iter().any(|a| a == "--debug"), ..DevWorker::default() };
                    techne_host::worker::main_with(fd, !rest.iter().any(|a| a == "--unsandboxed"), Some(Box::new(dev)))
                }
                None => usage(),
            }
        }
        Some((dev, rest)) if dev == "dev" => match parse(rest) {
            Some(options) => match dev_serve(&options) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("techne cloud: {}", e.msg);
                    ExitCode::FAILURE
                }
            },
            None => usage(),
        },
        _ => usage(),
    }
}

fn usage() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::from(2)
}

/// What `techne cloud dev` is asked to do.
struct Options {
    file: PathBuf,
    port: u16,
    nrepl: Option<u16>,
    /// The nREPL server in a debug instance, the deployment served as in
    /// production.
    debug: bool,
}

/// The options, from `FILE [--port PORT] [--nrepl PORT [--debug]]` in any
/// order.
fn parse(args: &[String]) -> Option<Options> {
    let mut options = Options { file: PathBuf::new(), port: 8787, nrepl: None, debug: false };
    let mut file = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" => options.port = args.next()?.parse().ok()?,
            "--nrepl" => options.nrepl = Some(args.next()?.parse().ok()?),
            "--debug" => options.debug = true,
            _ if file.is_none() && !arg.starts_with("--") => file = Some(PathBuf::from(arg)),
            _ => return None,
        }
    }
    options.file = file?;
    // A debug instance is reached through its REPL alone.
    if options.debug && options.nrepl.is_none() {
        return None;
    }
    Some(options)
}

/// Serve with a worker, as the module says.
fn dev_serve(options: &Options) -> Result<(), Error> {
    use techne_host::supervisor::{Options as WorkerOptions, ResourceConfinement, Sandboxing, Supervisor};
    let entry = options.file.canonicalize().map_err(|e| Error::new(format!("{}: {e}", options.file.display())))?;
    let dir = entry.parent().unwrap_or(Path::new("/")).to_path_buf();
    // The files as they were loaded: a save from now on deploys again.
    let seen = lisp_files(&dir);
    let program = std::env::current_exe().map_err(|e| Error::new(e.to_string()))?;
    let worker = WorkerOptions {
        program,
        args: ["cloud", "worker"].into_iter().chain(options.debug.then_some("--debug")).map(Into::into).collect(),
        env: Vec::new(),
        sandboxing: Sandboxing::IfAvailable,
        resources: ResourceConfinement::IfAvailable,
        cgroup: techne_sandbox::cgroup::Limits { memory: 512 << 20, pids: 256, cpu: None },
        wall: limits(options).wall,
        dev: options.nrepl.is_some(),
        egress: Default::default(),
    };
    let port = options.port;
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| Error::new(e.to_string()))?;
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async {
        let supervisor = Supervisor::start(worker).await.map_err(|e| Error::new(format!("no worker: {e}")))?;
        supervisor.deploy(NAME, &entry).await.map_err(Error::new)?;
        if let Some(port) = options.nrepl {
            let bound =
                supervisor.serve_repl(&format!("127.0.0.1:{port}")).await.map_err(|e| Error::new(format!("nREPL on port {port}: {e}")))?;
            // The port file first: clients may act on the line at once.
            let _ = std::fs::write(dir.join(".nrepl-port"), bound.port().to_string());
            println!("nREPL server started on port {} on host {} - nrepl://{bound}", bound.port(), bound.ip());
        }
        let mut front = Vm::new();
        techne_net::install_on(&mut front, tokio::runtime::Handle::current())?;
        supervisor.install(&mut front);
        serve_front(&mut front, port, &entry, options.debug)?;
        let redeploying = supervisor.clone();
        tokio::task::spawn_local(watch_files(dir, seen, move || {
            let (supervisor, entry) = (redeploying.clone(), entry.clone());
            async move {
                eprintln!("techne cloud: loading {}", entry.display());
                match supervisor.deploy(NAME, &entry).await {
                    Ok(_) => eprintln!("techne cloud: deployed {}", entry.display()),
                    Err(e) => eprintln!("techne cloud: not deployed, the last deployment keeps serving: {e}"),
                }
            }
        }));
        supervisor.run(&mut front).await?;
        Ok(())
    })
}

/// Start `lisp/http`'s server in `front` on `port`, handing requests to
/// `host-fetch`, saying where; bodies stream, both ways. With `debug`,
/// requests carrying a credential made here, and said, are routed to the
/// deployment's debug instance (`lisp/http/debug-route.scm`).
fn serve_front(front: &mut Vm, port: u16, entry: &Path, debug: bool) -> Result<(), Error> {
    let http = techne_vm::library::lisp_dir().join("http");
    front.eval_source(&format!(
        r#"(require {:?} {:?} {:?})
           (let* ((listener (tcp-listen {port}))
                  (credential (and {} (make-debug-credential)))
                  (serve (lambda (request debug) (host-fetch {NAME:?} request #:debug debug)))
                  (handler (if credential
                               (debug-routing serve credential)
                               (lambda (request) (serve request #f)))))
             (display (string-append "Serving " {:?} " on http://127.0.0.1:"
                                     (number->string (cadr (tcp-local-address listener))) "\n"))
             (when credential
               (display (string-append "Debug instance: it serves requests with the header "
                                       debug-route-header ": " credential "\n")))
             (flush-output-port)
             (spawn (lambda () (http-serve listener handler #:stream-bodies #t))))"#,
        http.join("http.scm").display().to_string(),
        http.join("server.scm").display().to_string(),
        http.join("debug-route.scm").display().to_string(),
        if debug { "#t" } else { "#f" },
        entry.display().to_string(),
    ))?;
    Ok(())
}

/// Call `changed` whenever a Lisp file under `dir` changes from what was
/// `seen`.
async fn watch_files<F: std::future::Future<Output = ()>>(dir: PathBuf, mut seen: Vec<(PathBuf, Option<u64>)>, changed: impl Fn() -> F) {
    let scan = |dir: PathBuf| async move { tokio::task::spawn_blocking(move || lisp_files(&dir)).await.unwrap_or_default() };
    loop {
        tokio::time::sleep(POLL).await;
        let now = scan(dir.clone()).await;
        if now != seen {
            seen = now;
            changed().await;
        }
    }
}

/// The deployment's limits: a dev world's requests may wait in a debugger
/// (a debug instance's have their own allowance, `Limits::debug_pause`).
fn limits(options: &Options) -> Limits {
    match options.nrepl {
        Some(_) if !options.debug => Limits { wall: DEV_WALL, ..Limits::default() },
        _ => Limits::default(),
    }
}

/// The Lisp files under `dir` and a hash of each one's contents, in
/// order (a save need not change a file's length or, within the clock's
/// resolution, its time); not those in hidden directories, `target` or
/// `node_modules`, which hold what is generated or fetched, nor through
/// symbolic links to directories.
fn lisp_files(dir: &Path) -> Vec<(PathBuf, Option<u64>)> {
    let mut files = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(d) = dirs.pop() {
        for entry in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(t) if t.is_dir() => {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    if !(name.starts_with('.') || name == "target" || name == "node_modules") {
                        dirs.push(path);
                    }
                }
                Ok(_) if path.extension().is_some_and(|e| e == "scm" || e == "sld") => {
                    let hash = std::fs::read(&path).ok().map(|bytes| {
                        let mut h = std::hash::DefaultHasher::new();
                        std::hash::Hash::hash(&bytes, &mut h);
                        std::hash::Hasher::finish(&h)
                    });
                    files.push((path, hash));
                }
                _ => {}
            }
        }
    }
    files.sort();
    files
}

/// What a worker of `techne cloud dev --nrepl` does for development: each
/// deployment served from a dev world (`Host::publish_dev`) with an nREPL
/// server in it, which moves to the next dev world on a deployment anew;
/// clients relayed by the supervisor. With `debug`, each deployment is
/// served as in production instead, and the server is in its debug
/// instance (`Host::attach_debug`).
#[derive(Default)]
pub struct DevWorker {
    /// The server, in the dev world (or debug instance) of the deployment
    /// it is numbered with.
    slot: Rc<RefCell<Option<(u64, Server)>>>,
    /// How many deployments were published.
    deployed: u64,
    debug: bool,
}

impl techne_host::worker::Dev for DevWorker {
    fn publish(&mut self, host: &Host, name: &str, entry: &Path, deployment: Deployment) -> Result<(), Error> {
        self.deployed += 1;
        let (generation, slot, waker) = (self.deployed, self.slot.clone(), host.waker());
        let entry = entry.display().to_string();
        let attach = move |vm: &mut Vm| -> Result<Box<dyn Attached>, Error> {
            let taken = slot.borrow_mut().take();
            let server = match taken {
                Some((before, server)) => match server.replace(vm, "the deployment was deployed again") {
                    Ok(server) => server,
                    // Kept in the world before, still the deployment's.
                    Err((server, e)) => {
                        *slot.borrow_mut() = Some((before, server));
                        return Err(e);
                    }
                },
                None => Server::install(vm, waker)?,
            };
            if let Some(module) = vm.loaded_module(&entry) {
                server.set_module(module);
            }
            // A failing request breaks into the subscribed client's debugger.
            vm.eval_in(ROOT_MODULE, "<techne cloud>", "(set! %host-debugger %nrepl-debug-request)")?;
            *slot.borrow_mut() = Some((generation, server));
            Ok(Box::new(DevRepl { slot, generation }))
        };
        if self.debug {
            // Served by its instances whether or not a REPL reaches it.
            host.publish(name, deployment);
            host.attach_debug(name, attach).map(drop)
        } else {
            host.publish_dev(name, deployment, attach)
        }
    }

    fn connect(&mut self, stream: tokio::io::DuplexStream) {
        if let Some((_, server)) = &*self.slot.borrow() {
            server.remote().serve_stream(stream);
        }
    }
}

/// The nREPL server as attached to one dev world: it runs there only
/// while that world is the deployment's, before it moves to the next.
struct DevRepl {
    slot: Rc<RefCell<Option<(u64, Server)>>>,
    generation: u64,
}

impl Attached for DevRepl {
    fn poll(&mut self, vm: &mut Vm) -> bool {
        match &*self.slot.borrow() {
            Some((g, server)) if *g == self.generation => server.poll(vm),
            _ => false,
        }
    }

    fn reap(&mut self, vm: &mut Vm) {
        if let Some((g, server)) = &*self.slot.borrow()
            && *g == self.generation
        {
            server.reap(vm);
        }
    }
}
