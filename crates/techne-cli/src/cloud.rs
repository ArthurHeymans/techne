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
//! The deployment's bindings are those its manifest names
//! (`manifest.scm` beside FILE, read as `lisp/cloud/manifest.scm` reads
//! it, with the secrets of `.secrets.scm`), given to its requests as
//! their `env`. Its KV namespaces are kept by the supervisor, in files
//! under `.techne/kv` beside FILE, by a task of the front world
//! (`lisp/cloud/kv.scm`): they outlive the worker and this process. The
//! manifest is read again at each deployment; a secret changed is seen
//! at the next.
//!
//! Deploying is `lisp/cloud/control.scm`'s, in the front world: each
//! deployment is prepared, its bundle kept under `.techne/bundles`, and
//! published if nothing overtook it, and recorded in
//! `.techne/deployments.scm`. With `--ops PORT`, the operations
//! `deployments`, `deploy` and `rollback` are served there as JSON over
//! HTTP (`POST /v1/ops`) on 127.0.0.1:PORT, for a token that allows them,
//! such as the one printed at the start and kept in `.techne/token`;
//! `techne cloud deployments FILE`, `techne cloud deploy FILE` and
//! `techne cloud rollback FILE [ID]` ask them of the `techne cloud dev`
//! serving FILE, with that token or the one in `TECHNE_TOKEN`. Rolled
//! back, a deployment's code and bindings serve again, until a file is
//! next saved; what was kept in storage stays.
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
//! With `--auth` too, a REPL client authenticates before anything it
//! sends reaches the worker: its first message is `authenticate`, with a
//! token, which a task of the front world checks
//! (`lisp/cloud/auth.scm`, `lisp/cloud/tokens.scm`) against the key kept
//! in `.techne/token-key` beside FILE; it must allow `eval` on the
//! deployment, or `prod` for a debug instance. A token that does, good
//! for thirty days, is printed at the start. A client then evaluates in
//! sessions of its own (`clone`), which are its principal's alone
//! (`Remote::serve_stream_as`). A connection that does not authenticate
//! within ten seconds, or in its first kibibytes, is closed; few wait to
//! at once.
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
use techne_vm::{
    data::Data,
    vm::{Error, ROOT_MODULE, Vm},
};

const USAGE: &str = "usage: techne cloud dev FILE [--port PORT] [--ops PORT] [--nrepl PORT [--debug] [--auth]]
       techne cloud deployments FILE
       techne cloud deploy FILE
       techne cloud rollback FILE [ID]
       techne cloud serve --state DIR --domain DOMAIN [--port PORT] [--ops PORT] [--nrepl PORT] [--live N] [--idle SECONDS] [--unconfined]
       techne cloud invite --state DIR TENANT
       techne cloud deploy FILE --to URL --tenant TENANT --project PROJECT
       techne cloud deployments|rollback [ID]|secret NAME VALUE --to URL --tenant TENANT --project PROJECT

Serve the deployment whose entry module is FILE on 127.0.0.1:PORT (8787
by default; 0 picks one), deploying again whenever a Lisp file under
FILE's directory changes. With --nrepl, serve it from a world kept across
requests, with an nREPL server into it on that port. With --debug too,
serve it as in production, and put the nREPL server in a debug instance
beside, which serves the requests carrying the header techne-debug-route
with the credential printed. With --auth, an nREPL client must
authenticate first, with a token such as the one printed. With --ops,
serve the operations on its deployments on that port, which the other
commands ask of it: the deployments published, deploying again, and
going back to the one before or to ID.

serve runs a cloud of tenants, kept in DIR: each project invited tenants
deploy is served at PROJECT.TENANT.DOMAIN on PORT, by a worker for each
tenant, stopped when idle. invite gives a tenant a token for its
projects. With --to, the address of a serve's operations, the commands
ask it, with the token in TECHNE_TOKEN: deploy uploads FILE's directory.";

/// How long a deployment may take, loading and all.
const DEPLOYING: Duration = Duration::from_secs(90);

/// Connections that may be authenticating at once: more are closed.
const AUTHENTICATING: usize = 16;

/// How long a connection has to authenticate, and how many bytes its
/// first message may be.
const AUTHENTICATE: Duration = Duration::from_secs(10);
const FIRST_MESSAGE: usize = 8 << 10;

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
        Some((serve, rest)) if serve == "serve" => done(serve_fleet(rest).map(|()| String::new())),
        Some((invite, rest)) if invite == "invite" => done(invite_tenant(rest)),
        Some((op, rest)) if rest.iter().any(|a| a == "--to") => done(operate_remote(op, rest)),
        Some((op, rest)) if matches!(op.as_str(), "deployments" | "deploy" | "rollback") => {
            let id = rest.get(1).filter(|_| op == "rollback");
            match rest.first() {
                Some(file) if rest.len() <= 1 + usize::from(id.is_some()) => match operate(op, Path::new(file), id.map(String::as_str)) {
                    Ok(answer) => {
                        println!("{answer}");
                        ExitCode::SUCCESS
                    }
                    Err(e) => {
                        eprintln!("techne cloud: {e}");
                        ExitCode::FAILURE
                    }
                },
                _ => usage(),
            }
        }
        _ => usage(),
    }
}

/// Say `answer`, or why there is none: how a command ended.
fn done(answer: Result<String, String>) -> ExitCode {
    match answer {
        Ok(answer) => {
            if !answer.is_empty() {
                println!("{answer}");
            }
            ExitCode::SUCCESS
        }
        Err(e) if e.is_empty() => usage(),
        Err(e) => {
            eprintln!("techne cloud: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The value after `flag` in `args`, and `args` without both.
fn flag(args: &mut Vec<String>, flag: &str) -> Option<String> {
    let at = args.iter().position(|a| a == flag)?;
    (at + 1 < args.len()).then(|| args.drain(at..at + 2).nth(1)).flatten()
}

/// `techne cloud serve --state DIR --domain DOMAIN [--port PORT] [--ops
/// PORT] [--live N] [--idle SECONDS] [--unconfined]`: a fleet of the
/// tenants invited (`lisp/cloud/serve.scm`), each project served at
/// `project.tenant.DOMAIN` on 127.0.0.1:PORT, the operations on them on
/// 127.0.0.1:PORT of `--ops`, for something in front that has the
/// network and TLS. Workers are sandboxed and their resources bounded,
/// or it does not start: but with `--unconfined`, for trying it where
/// they cannot be.
fn serve_fleet(args: &[String]) -> Result<(), String> {
    use techne_host::{
        fleet::{Fleet, FleetOptions},
        supervisor::{Options as WorkerOptions, ResourceConfinement, Sandboxing},
    };
    let mut args = args.to_vec();
    let number = |args: &mut Vec<String>, name: &str, default: u64| match flag(args, name) {
        Some(n) => n.parse::<u64>().map_err(|_| String::new()),
        None => Ok(default),
    };
    let (state, domain) = (flag(&mut args, "--state").ok_or("")?, flag(&mut args, "--domain").ok_or("")?);
    let (port, ops) = (number(&mut args, "--port", 8787)?, number(&mut args, "--ops", 8788)?);
    let (live, idle) = (number(&mut args, "--live", 64)?, number(&mut args, "--idle", 300)?);
    let nrepl = flag(&mut args, "--nrepl").map(|p| p.parse::<u16>().map_err(|_| String::new())).transpose()?;
    let unconfined = args.iter().position(|a| a == "--unconfined").map(|at| args.remove(at)).is_some();
    if !args.is_empty() || domain.contains(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-')) {
        return Err(String::new());
    }
    std::fs::create_dir_all(&state).map_err(|e| format!("{state}: {e}"))?;
    let state = Path::new(&state).canonicalize().map_err(|e| e.to_string())?;
    let options = FleetOptions {
        worker: WorkerOptions {
            program: std::env::current_exe().map_err(|e| e.to_string())?,
            args: vec!["cloud".into(), "worker".into()],
            env: Vec::new(),
            sandboxing: if unconfined { Sandboxing::IfAvailable } else { Sandboxing::Required },
            resources: if unconfined { ResourceConfinement::IfAvailable } else { ResourceConfinement::Required },
            cgroup: techne_sandbox::cgroup::Limits { memory: 512 << 20, pids: 256, cpu: None },
            wall: Limits::default().wall,
            dev: false,
            egress: Default::default(),
        },
        live: live as usize,
        idle: Duration::from_secs(idle),
    };
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async {
        let fleet = Fleet::new(options).map_err(|e| e.to_string())?;
        // Before it listens: a worker can be started, confined as said.
        fleet.preflight().await.map_err(|e| format!("no worker can be started: {e}"))?;
        let mut front = Vm::new();
        let failed = |e: Error| e.msg.clone();
        techne_net::install_on(&mut front, tokio::runtime::Handle::current()).map_err(failed)?;
        // Before the Lisp that names them is loaded.
        fleet.install(&mut front);
        fleet.install_store(&mut front);
        fleet.install_control(&mut front);
        let sessions = techne_host::ask::install(&mut front, "debug", AUTHENTICATING);
        let lisp = techne_vm::library::lisp_dir();
        let file = |name: &str| lisp.join(name).display().to_string();
        front
            .eval_source(&format!(
                r#"(require {:?} {:?} {:?} {:?})
                   (define cloud (make-cloud {:?}))
                   (spawn (lambda () (kv-serve-tenants {:?})))
                   (spawn (lambda () (cloud-debug-serve cloud)))
                   (spawn (lambda ()
                            (cloud-restore! cloud)
                            (let ((ops (tcp-listen {ops})) (listener (tcp-listen {port})))
                              (display (string-append "Operations on http://127.0.0.1:"
                                                      (number->string (cadr (tcp-local-address ops))) "\n"))
                              (display (string-append "Serving " {domain:?} " on http://127.0.0.1:"
                                                      (number->string (cadr (tcp-local-address listener))) "\n"))
                              (flush-output-port)
                              (spawn (lambda () (http-serve ops (cloud-ops-handler cloud) #:max-body (* 32 1024 1024)
                                                            #:max-connections 64 #:stream-bodies #t)))
                              (http-serve listener
                                          (cloud-front-handler cloud {domain:?}
                                                               (lambda (target request debug) (host-fetch target request #:debug debug)))
                                          #:stream-bodies #t))))"#,
                file("cloud/serve.scm"),
                file("cloud/kv.scm"),
                file("cloud/hosts.scm"),
                file("http/server.scm"),
                state.display().to_string(),
                state.join("kv").display().to_string(),
            ))
            .map_err(failed)?;
        if let Some(port) = nrepl {
            let bound = serve_debug_sessions(fleet.clone(), &format!("127.0.0.1:{port}"), sessions)
                .await
                .map_err(|e| format!("nREPL on port {port}: {e}"))?;
            println!("nREPL server started on port {} on host {} - nrepl://{bound}", bound.port(), bound.ip());
        }
        fleet.run(&mut front).await.map_err(failed)
    })
}

/// `techne cloud invite --state DIR TENANT`: invite TENANT to the cloud
/// kept in DIR (`cloud-invite!`): a token for all its projects.
fn invite_tenant(args: &[String]) -> Result<String, String> {
    let mut args = args.to_vec();
    let state = flag(&mut args, "--state").ok_or("")?;
    let [tenant] = &args[..] else { return Err(String::new()) };
    let mut vm = Vm::new();
    let failed = |e: Error| e.msg.clone();
    let serve = techne_vm::library::lisp_dir().join("cloud/serve.scm").display().to_string();
    vm.eval_source(&format!("(require {serve:?})")).map_err(failed)?;
    let token = vm.eval_source(&format!("(cloud-invite! (make-cloud {state:?}) {tenant:?})")).map_err(failed)?;
    vm.get::<String>(&token).map_err(failed)
}

/// `S` as a JSON string.
fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `bytes` in base64, padded.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            match i <= chunk.len() {
                true => out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char),
                false => out.push('='),
            }
        }
    }
    out
}

/// A command with `--to URL --tenant TENANT --project PROJECT`: the
/// operation `op` asked of the `techne cloud serve` whose operations
/// are at URL (`http://HOST:PORT`), with the token in `TECHNE_TOKEN`:
/// `deploy FILE`, the files under FILE's directory uploaded;
/// `deployments`; `rollback [ID]`; `secret NAME VALUE`.
fn operate_remote(op: &str, args: &[String]) -> Result<String, String> {
    let mut args = args.to_vec();
    let need = |args: &mut Vec<String>, name: &str| flag(args, name).ok_or(String::new());
    let (to, tenant, project) = (need(&mut args, "--to")?, need(&mut args, "--tenant")?, need(&mut args, "--project")?);
    let addr = to.strip_prefix("http://").ok_or("--to is http://HOST:PORT")?.trim_end_matches('/').to_string();
    let token = std::env::var("TECHNE_TOKEN").map_err(|_| "no token: set TECHNE_TOKEN")?;
    let mut fields = vec![("tenant", json_string(&tenant)), ("project", json_string(&project))];
    match (op, &args[..]) {
        ("deployments", []) | ("rollback", []) => fields.push(("op", json_string(op))),
        ("rollback", [id]) => fields.extend([("op", json_string(op)), ("id", json_string(id))]),
        ("secret", [name, value]) => {
            fields.extend([("op", json_string("secret-put")), ("name", json_string(name)), ("value", json_string(value))])
        }
        ("deploy", [file]) => {
            let entry = Path::new(file).canonicalize().map_err(|e| format!("{file}: {e}"))?;
            let dir = entry.parent().ok_or("an entry in no directory")?;
            let name = entry.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let files: Vec<String> = techne_host::supervisor::bundle(dir)?
                .iter()
                .map(|(p, b)| format!("{}:{}", json_string(p), json_string(&base64(b))))
                .collect();
            fields.extend([("op", json_string(op)), ("entry", json_string(&name)), ("files", format!("{{{}}}", files.join(",")))]);
        }
        _ => return Err(String::new()),
    }
    let body: Vec<String> = fields.iter().map(|(k, v)| format!("{}:{v}", json_string(k))).collect();
    let named = |name: &str| !name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !(named(&tenant) && named(&project)) {
        return Err("not a tenant's or a project's name".into());
    }
    post(&addr, &token, Some(&format!("{tenant}/{project}")), &format!("{{{}}}", body.join(",")))
}

/// POST `body`, an operation, to the operations at `addr`, with `token`,
/// on `project` (`tenant/project`) if a cloud's: the answer, or what was
/// answered instead.
fn post(addr: &str, token: &str, project: Option<&str>, body: &str) -> Result<String, String> {
    use std::io::{Read, Write};
    if token.bytes().any(|b| b.is_ascii_control() || b == b' ') || addr.bytes().any(|b| b.is_ascii_control() || b == b' ' || b == b'/') {
        return Err("not a token, or not an address".into());
    }
    let mut c = std::net::TcpStream::connect(addr).map_err(|e| format!("{addr}: {e}"))?;
    let io = |e: std::io::Error| e.to_string();
    c.set_read_timeout(Some(DEPLOYING + Duration::from_secs(10))).map_err(io)?;
    write!(
        c,
        "POST /v1/ops HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nAuthorization: Bearer {token}\r\n{}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        project.map(|p| format!("Techne-Project: {p}\r\n")).unwrap_or_default(),
        body.len()
    )
    .map_err(io)?;
    // Refused from its head, the rest may not be taken: what was
    // answered is read all the same.
    let sent = c.write_all(body.as_bytes());
    let mut answer = String::new();
    let read = c.read_to_string(&mut answer);
    let Some((head, body)) = answer.split_once("\r\n\r\n") else {
        return Err(match (sent, read) {
            (Err(e), _) | (_, Err(e)) => format!("refused, the connection closed: {e}"),
            _ => "no answer".into(),
        });
    };
    match head.split_whitespace().nth(1) {
        Some("200") => Ok(body.to_string()),
        _ => Err(body.to_string()),
    }
}

/// Ask the `techne cloud dev --ops` serving `file` for operation `op`
/// (on deployment `id`, if given), with the token in `TECHNE_TOKEN` or
/// the one it keeps: its answer, JSON, or why there is none.
fn operate(op: &str, file: &Path, id: Option<&str>) -> Result<String, String> {
    let entry = file.canonicalize().map_err(|e| format!("{}: {e}", file.display()))?;
    let kept = entry.parent().unwrap_or(Path::new("/")).join(".techne");
    let read = |name: &str| std::fs::read_to_string(kept.join(name)).map(|s| s.trim().to_string());
    let port: u16 =
        read("ops-port").ok().and_then(|p| p.parse().ok()).ok_or("no techne cloud dev --ops serves it (no .techne/ops-port)")?;
    let token = std::env::var("TECHNE_TOKEN").or_else(|_| read("token")).map_err(|_| "no token: set TECHNE_TOKEN")?;
    let body = match id {
        Some(id) => format!(r#"{{"op":{},"id":{}}}"#, json_string(op), json_string(id)),
        None => format!(r#"{{"op":{}}}"#, json_string(op)),
    };
    post(&format!("127.0.0.1:{port}"), &token, None, &body)
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
    /// nREPL clients authenticate with a token.
    auth: bool,
    /// The port the operations on deployments are served on.
    ops: Option<u16>,
}

/// The options, from `FILE [--port PORT] [--nrepl PORT [--debug]]` in any
/// order.
fn parse(args: &[String]) -> Option<Options> {
    let mut options = Options { file: PathBuf::new(), port: 8787, nrepl: None, debug: false, auth: false, ops: None };
    let mut file = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" => options.port = args.next()?.parse().ok()?,
            "--nrepl" => options.nrepl = Some(args.next()?.parse().ok()?),
            "--debug" => options.debug = true,
            "--auth" => options.auth = true,
            "--ops" => options.ops = Some(args.next()?.parse().ok()?),
            _ if file.is_none() && !arg.starts_with("--") => file = Some(PathBuf::from(arg)),
            _ => return None,
        }
    }
    options.file = file?;
    // A debug instance is reached through its REPL alone.
    if (options.debug || options.auth) && options.nrepl.is_none() {
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
        let mut front = Vm::new();
        techne_net::install_on(&mut front, tokio::runtime::Handle::current())?;
        supervisor.install(&mut front);
        supervisor.install_store(&mut front);
        supervisor.install_control(&mut front);
        let control = techne_host::ask::install(&mut front, "control", 4);
        let cloud = techne_vm::library::lisp_dir().join("cloud");
        front.eval_source(&format!(
            r#"(require {:?})
               (define control (make-control {NAME:?} {:?} {:?}))
               (spawn (lambda () (control-serve control)))"#,
            cloud.join("control.scm").display().to_string(),
            entry.display().to_string(),
            dir.join(".techne").display().to_string(),
        ))?;
        let deploy = || Data::List(vec![Data::String("deploy".into())]);
        // The first deployment, the front world driven until it is
        // there: nothing is served before.
        let first = {
            let asked = control.ask(deploy(), DEPLOYING);
            tokio::pin!(asked);
            tokio::select! {
                deployed = &mut asked => deployed,
                ended = supervisor.run(&mut front) => Err(ended.err().map_or("the front world ended".into(), |e| e.msg.clone())),
            }
        };
        first.map_err(Error::new)?;
        // Before the Lisp that asks through it is loaded: a module names
        // the natives there are when it is.
        let auth = options.auth.then(|| techne_host::ask::install(&mut front, "auth", AUTHENTICATING));
        if options.auth || options.ops.is_some() {
            tokens(&mut front, &dir)?;
        }
        if let Some(port) = options.ops {
            front.eval_source(&format!(
                r#"(require {:?})
                   (let ((listener (tcp-listen {port})))
                     (replace-file {:?} (number->string (cadr (tcp-local-address listener))))
                     (display (string-append "Operations on http://127.0.0.1:"
                                             (number->string (cadr (tcp-local-address listener))) "\n"))
                     (flush-output-port)
                     (spawn (lambda () (http-serve listener (control-ops-handler control token-key)))))"#,
                techne_vm::library::lisp_dir().join("http/server.scm").display().to_string(),
                dir.join(".techne/ops-port").display().to_string(),
            ))?;
        }
        if let Some(port) = options.nrepl {
            let addr = format!("127.0.0.1:{port}");
            let bound = match auth {
                Some(auth) => {
                    let scope = if options.debug { "prod" } else { "eval" };
                    front.eval_source(&format!(
                        r#"(require {:?}) (spawn (lambda () (auth-serve token-key {NAME:?} {scope:?})))"#,
                        cloud.join("auth.scm").display().to_string()
                    ))?;
                    serve_repl_authenticated(supervisor.clone(), &addr, auth).await
                }
                None => supervisor.serve_repl(&addr).await,
            };
            let bound = bound.map_err(|e| Error::new(format!("nREPL on port {port}: {e}")))?;
            // The port file first: clients may act on the line at once.
            let _ = std::fs::write(dir.join(".nrepl-port"), bound.port().to_string());
            println!("nREPL server started on port {} on host {} - nrepl://{bound}", bound.port(), bound.ip());
        }
        serve_front(&mut front, port, &entry, options.debug)?;
        tokio::task::spawn_local(watch_files(dir, seen, move || {
            let (control, entry) = (control.clone(), entry.clone());
            async move {
                eprintln!("techne cloud: loading {}", entry.display());
                match control.ask(deploy(), DEPLOYING).await {
                    Ok(_) => eprintln!("techne cloud: deployed {}", entry.display()),
                    Err(e) => eprintln!("techne cloud: not deployed, the last deployment keeps serving: {e}"),
                }
            }
        }));
        supervisor.run(&mut front).await?;
        Ok(())
    })
}

/// Define `token-key` in `front`, the key kept in `.techne` under `dir`
/// (`lisp/cloud/auth.scm`), and print a token made with it that allows
/// everything on the deployment for thirty days, kept in `.techne/token`
/// too, for the commands that ask this one.
fn tokens(front: &mut Vm, dir: &Path) -> Result<(), Error> {
    let cloud = techne_vm::library::lisp_dir().join("cloud");
    front.eval_source(&format!(
        r#"(require {:?} {:?})
           (define token-key (token-key-in {:?}))
           (let ((token (make-token token-key "dev" #:projects (list {NAME:?}) #:scopes token-scopes
                                    #:expires (+ (current-second) (* 30 24 3600)))))
             (replace-file {:?} token)
             (display (string-append "Token: " token "\n"))
             (flush-output-port))"#,
        cloud.join("auth.scm").display().to_string(),
        cloud.join("tokens.scm").display().to_string(),
        dir.join(".techne").display().to_string(),
        dir.join(".techne/token").display().to_string(),
    ))?;
    Ok(())
}

/// Accept REPL clients on `addr`, each relayed by `supervisor` once it
/// has authenticated (`authenticate`), for the principal `auth` answered:
/// the address bound.
async fn serve_repl_authenticated(
    supervisor: techne_host::supervisor::Supervisor,
    addr: &str,
    auth: techne_host::ask::Asker,
) -> std::io::Result<std::net::SocketAddr> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let bound = listener.local_addr()?;
    let places = std::sync::Arc::new(tokio::sync::Semaphore::new(AUTHENTICATING));
    tokio::task::spawn_local(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            // Too many at it already: closed.
            let Ok(place) = places.clone().try_acquire_owned() else { continue };
            let _ = stream.set_nodelay(true);
            let (supervisor, auth) = (supervisor.clone(), auth.clone());
            tokio::task::spawn_local(async move {
                let admitted = tokio::time::timeout(AUTHENTICATE, authenticate(&mut stream, &auth)).await.ok().flatten();
                drop(place);
                if let Some((principal, sent)) = admitted {
                    supervisor.relay(stream, Some(principal), sent);
                }
            });
        }
    });
    Ok(bound)
}

/// The first message of `stream`, an nREPL client, and what the client
/// sent after it: none for what is no message, or one past what a first
/// one may be, which is answered nothing.
async fn first_message(stream: &mut tokio::net::TcpStream) -> Option<(techne_node::bencode::B, Vec<u8>)> {
    use techne_node::bencode::parse;
    use tokio::io::AsyncReadExt;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    let (message, used) = loop {
        match parse(&buf) {
            Ok(Some((message, used))) if used <= FIRST_MESSAGE => break (message, used),
            Ok(None) if buf.len() < FIRST_MESSAGE => {}
            _ => return None,
        }
        match stream.read(&mut chunk).await {
            Ok(n @ 1..) => buf.extend_from_slice(&chunk[..n]),
            _ => return None,
        }
    };
    Some((message, buf[used..].to_vec()))
}

/// What `message`, a client's first, has as `field`, if it is an
/// `authenticate`.
fn authenticating(message: &techne_node::bencode::B, field: &str) -> Option<String> {
    use techne_node::bencode::B;
    (message.get("op").and_then(B::as_str) == Some("authenticate"))
        .then(|| message.get(field).and_then(B::as_str).map(str::to_string))
        .flatten()
}

/// Answer `message`, an `authenticate` (or what came in its place, if
/// none), on `stream`: `done` with `fields` if `admitted`, else
/// `unauthenticated`. Whether it was written.
async fn answer_first(
    stream: &mut tokio::net::TcpStream,
    message: Option<&techne_node::bencode::B>,
    admitted: bool,
    fields: Vec<(&str, techne_node::bencode::B)>,
) -> bool {
    use techne_node::bencode::{B, dict};
    use tokio::io::AsyncWriteExt;
    let statuses: &[&str] = if admitted { &["done"] } else { &["error", "unauthenticated", "done"] };
    let mut all = vec![("status", B::List(statuses.iter().map(B::str).collect()))];
    all.extend(fields);
    if let Some(id) = message.and_then(|m| m.get("id")) {
        all.push(("id", id.clone()));
    }
    let mut reply = Vec::new();
    dict(all).encode(&mut reply);
    stream.write_all(&reply).await.is_ok()
}

/// Read the first message of `stream`, an nREPL client: an
/// `authenticate` with a `token` that `auth` says holds, answered
/// `done`; anything else is answered `unauthenticated`. The principal,
/// and what the client sent after the message.
async fn authenticate(stream: &mut tokio::net::TcpStream, auth: &techne_host::ask::Asker) -> Option<(String, Vec<u8>)> {
    let (message, sent) = first_message(stream).await?;
    let principal = match authenticating(&message, "token") {
        Some(token) => match auth.ask(Data::String(token), AUTHENTICATE).await {
            Ok(Data::String(principal)) => Some(principal),
            _ => None,
        },
        None => None,
    };
    let written = answer_first(stream, Some(&message), principal.is_some(), Vec::new()).await;
    principal.filter(|_| written).map(|p| (p, sent))
}

/// Accept REPL clients of a cloud's projects on `addr`: each
/// authenticates with a first message, an `authenticate` with its
/// `token` and the `project` it is for (`tenant/project`), which
/// `sessions` opens a debugging session for (`cloud-debug-serve`) or
/// refuses; answered `done` with the session's `techne-debug-route`
/// credential, it is relayed to the project's debug instance, attached
/// for it. The session is closed when the connection ends. The address
/// bound.
async fn serve_debug_sessions(
    fleet: techne_host::fleet::Fleet,
    addr: &str,
    sessions: techne_host::ask::Asker,
) -> std::io::Result<std::net::SocketAddr> {
    use techne_node::bencode::B;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let bound = listener.local_addr()?;
    let places = std::sync::Arc::new(tokio::sync::Semaphore::new(AUTHENTICATING));
    tokio::task::spawn_local(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let Ok(place) = places.clone().try_acquire_owned() else { continue };
            let _ = stream.set_nodelay(true);
            let (fleet, sessions) = (fleet.clone(), sessions.clone());
            tokio::task::spawn_local(async move {
                let text = |s: &str| Data::String(s.into());
                let opened = tokio::time::timeout(AUTHENTICATE, async {
                    let (message, sent) = first_message(&mut stream).await?;
                    let asked = authenticating(&message, "token").zip(authenticating(&message, "project"));
                    let session = match &asked {
                        Some((token, target)) => {
                            match sessions.ask(Data::List(vec![text("open"), text(token), text(target)]), AUTHENTICATE).await {
                                Ok(Data::List(session)) => match &session[..] {
                                    [Data::String(principal), Data::String(credential)] => Some((principal.clone(), credential.clone())),
                                    _ => None,
                                },
                                _ => None,
                            }
                        }
                        None => None,
                    };
                    Some((message, sent, asked.map(|(_, target)| target).zip(session)))
                })
                .await
                .ok()
                .flatten();
                drop(place);
                // What is no message is answered nothing; one refused,
                // that it is.
                let Some((message, sent, session)) = opened else { return };
                let Some((target, (principal, credential))) = session else {
                    answer_first(&mut stream, Some(&message), false, Vec::new()).await;
                    return;
                };
                let close = || sessions.ask(Data::List(vec![text("close"), text(&target), text(&credential)]), AUTHENTICATE);
                let supervisor = target.split_once('/').and_then(|(tenant, project)| Some((fleet.known(tenant)?, project.to_string())));
                let answered =
                    answer_first(&mut stream, Some(&message), supervisor.is_some(), vec![("techne-debug-route", B::str(&credential))])
                        .await;
                let ended = match supervisor.filter(|_| answered) {
                    Some((supervisor, project)) => supervisor.attach(&project, stream, principal, sent).await.ok(),
                    None => None,
                };
                // Until the connection ends: then the session does, for
                // sure, however busy what closes it is.
                if let Some(ended) = ended {
                    let _ = ended.await;
                }
                while close().await.is_err() {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            });
        }
    });
    Ok(bound)
}

/// Start `lisp/http`'s server in `front` on `port`, handing requests to
/// `host-fetch`, saying where; bodies stream, both ways. With `debug`,
/// requests carrying a credential made here, and said, are routed to the
/// deployment's debug instance (`lisp/http/debug-route.scm`). A task
/// keeps the deployment's KV namespaces under `.techne/kv` beside its
/// entry (`lisp/cloud/kv.scm`).
fn serve_front(front: &mut Vm, port: u16, entry: &Path, debug: bool) -> Result<(), Error> {
    let http = techne_vm::library::lisp_dir().join("http");
    let kv = entry.parent().unwrap_or(Path::new("/")).join(".techne/kv");
    front.eval_source(&format!(
        r#"(require {:?})
           (spawn (lambda () (kv-serve (make-kv-store {:?}))))"#,
        techne_vm::library::lisp_dir().join("cloud/kv.scm").display().to_string(),
        kv.display().to_string(),
    ))?;
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
/// deployment loaded has a dev world (`Host::publish_dev`), and the nREPL
/// server is in the world of the one published (`activate`), to which it
/// moves from the one before, its clients staying connected; they are
/// relayed by the supervisor. With `debug`, each deployment is served as
/// in production instead, and the server is in the debug instance
/// (`Host::attach_debug`) of the one published.
#[derive(Default)]
pub struct DevWorker {
    repl: Rc<RefCell<Repl>>,
    /// How many deployments were loaded, and the number of each still
    /// served, by its name.
    deployed: u64,
    names: std::collections::HashMap<String, u64>,
    debug: bool,
    /// The entry module of each deployment served as in production, by
    /// its name; the debug instance attached to each that has one, for
    /// its REPL clients; and the deployment each such client is of.
    entries: std::collections::HashMap<String, String>,
    debugged: std::collections::HashMap<String, Debugged>,
    clients: std::collections::HashMap<u64, String>,
    /// The clients whose debug instance is gone, to be closed (`gone`).
    orphans: Vec<u64>,
}

impl DevWorker {
    /// The deployment published as `name` has its debug instance no
    /// longer: its clients are to be closed.
    fn orphan(&mut self, name: &str) {
        self.debugged.remove(name);
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.clients).into_iter().partition(|(_, of)| of == name);
        self.clients = kept.into_iter().collect();
        self.orphans.extend(gone.into_iter().map(|(cid, _)| cid));
    }
}

/// A deployment's debug instance, attached for REPL clients.
struct Debugged {
    /// Its number in the host.
    id: u64,
    /// The nREPL server in it.
    server: Rc<RefCell<Option<Server>>>,
    clients: usize,
}

/// The nREPL server of a debug instance attached for clients, as
/// attached to its world.
struct DebugRepl(Rc<RefCell<Option<Server>>>);

impl Attached for DebugRepl {
    fn poll(&mut self, vm: &mut Vm) -> bool {
        self.0.borrow().as_ref().is_some_and(|server| server.poll(vm))
    }

    fn reap(&mut self, vm: &mut Vm) {
        if let Some(server) = &*self.0.borrow() {
            server.reap(vm);
        }
    }
}

/// The nREPL server of a worker, and where it is and is to be.
#[derive(Default)]
struct Repl {
    /// The server, in the world of the deployment it is numbered with.
    server: Option<(u64, Server)>,
    /// The number of the deployment published, whose world is the
    /// server's: it moves there when that world is next driven.
    active: u64,
    /// Clients that connected while the server was in no world yet.
    waiting: Vec<(tokio::io::DuplexStream, Option<String>)>,
}

/// Clients that may wait for the server to be in a world.
const WAITING: usize = 16;

impl techne_host::worker::Dev for DevWorker {
    fn publish(&mut self, host: &Host, name: &str, entry: &Path, deployment: Deployment) -> Result<(), Error> {
        self.deployed += 1;
        self.names.insert(name.into(), self.deployed);
        let waker: std::sync::Arc<dyn Fn() + Send + Sync> = std::sync::Arc::new(host.waker());
        let world = DevRepl { repl: self.repl.clone(), generation: self.deployed, entry: entry.display().to_string(), waker };
        // The server stays where it is: a deployment loaded is not yet
        // the one served.
        let attach = move |_: &mut Vm| -> Result<Box<dyn Attached>, Error> { Ok(Box::new(world)) };
        if self.debug {
            // Served by its instances whether or not a REPL reaches it.
            host.publish(name, deployment);
            host.attach_debug(name, attach).map(drop)
        } else {
            host.publish_dev(name, deployment, attach)
        }
    }

    fn activate(&mut self, host: &Host, name: &str) {
        if let Some(generation) = self.names.get(name) {
            self.repl.borrow_mut().active = *generation;
            // Its world is driven, and the server moves.
            (host.waker())();
        }
    }

    fn retire(&mut self, name: &str) {
        self.names.remove(name);
        // Its debug instance went with it, in the host: its clients go.
        self.entries.remove(name);
        self.orphan(name);
    }

    fn gone(&mut self, host: &Host) -> Vec<u64> {
        // Poisoned, say: the host has it no longer.
        let lost: Vec<String> =
            self.debugged.iter().filter(|(name, d)| host.debugging(name) != Some(d.id)).map(|(name, _)| name.clone()).collect();
        for name in lost {
            self.orphan(&name);
        }
        std::mem::take(&mut self.orphans)
    }

    fn loaded(&mut self, name: &str, entry: &Path) {
        self.entries.insert(name.into(), entry.display().to_string());
    }

    fn attach(
        &mut self,
        host: &Host,
        name: &str,
        cid: u64,
        stream: tokio::io::DuplexStream,
        principal: Option<String>,
    ) -> Result<(), Error> {
        // One the host has no longer is not attached to: another is.
        if self.debugged.get(name).is_some_and(|d| host.debugging(name) != Some(d.id)) {
            self.orphan(name);
        }
        if !self.debugged.contains_key(name) {
            let entry = self.entries.get(name).cloned().ok_or_else(|| Error::new("no such deployment"))?;
            let server = Rc::new(RefCell::new(None));
            let (installed, waker) = (server.clone(), host.waker());
            let id = host.attach_debug(name, move |vm| {
                let server = Server::install(vm, waker)?;
                if let Some(module) = vm.loaded_module(&entry) {
                    server.set_module(module);
                }
                // A failing request routed there breaks into the
                // subscribed client's debugger.
                vm.eval_in(ROOT_MODULE, "<techne cloud>", "(set! %host-debugger %nrepl-debug-request)")?;
                *installed.borrow_mut() = Some(server);
                Ok(Box::new(DebugRepl(installed)))
            })?;
            self.debugged.insert(name.into(), Debugged { id, server, clients: 0 });
        }
        let debugged = self.debugged.get_mut(name).ok_or_else(|| Error::new("no debug instance"))?;
        match &*debugged.server.borrow() {
            Some(server) => server.remote().serve_stream_as(stream, principal),
            None => return Err(Error::new("no REPL in the debug instance")),
        }
        debugged.clients += 1;
        self.clients.insert(cid, name.into());
        Ok(())
    }

    fn closed(&mut self, host: &Host, cid: u64) {
        let Some(name) = self.clients.remove(&cid) else { return };
        let Some(debugged) = self.debugged.get_mut(&name) else { return };
        debugged.clients = debugged.clients.saturating_sub(1);
        // Its last client gone: the debug instance is retired.
        if debugged.clients == 0 {
            host.detach_debug(&name, Some(debugged.id));
            self.debugged.remove(&name);
        }
    }

    fn connect(&mut self, stream: tokio::io::DuplexStream, principal: Option<String>) {
        let mut repl = self.repl.borrow_mut();
        match &repl.server {
            Some((_, server)) => server.remote().serve_stream_as(stream, principal),
            None if repl.waiting.len() < WAITING => repl.waiting.push((stream, principal)),
            None => {}
        }
    }
}

/// The nREPL server as attached to one deployment's dev world (or debug
/// instance): it runs there while that deployment is the one published,
/// and is moved there, when the world is driven, once it is.
struct DevRepl {
    repl: Rc<RefCell<Repl>>,
    generation: u64,
    /// The deployment's entry module, which sessions evaluate in.
    entry: String,
    waker: std::sync::Arc<dyn Fn() + Send + Sync>,
}

impl DevRepl {
    /// Move the server to `vm`, this deployment's world, from where it
    /// is, or start it there: clients stay connected, and those that
    /// waited for it are served.
    fn take_server(&self, repl: &mut Repl, vm: &mut Vm) {
        let waker = self.waker.clone();
        let moved = match repl.server.take() {
            Some((before, server)) => server.replace(vm, "another deployment was published").map_err(|(server, e)| {
                // Kept in the world before.
                repl.server = Some((before, server));
                e
            }),
            None => Server::install(vm, move || waker()),
        };
        let server = match moved {
            Ok(server) => server,
            Err(e) => {
                eprintln!("techne cloud: the REPL is not in the deployment published: {}", e.msg);
                // Not tried at each turn.
                repl.active = repl.server.as_ref().map_or(0, |(g, _)| *g);
                return;
            }
        };
        if let Some(module) = vm.loaded_module(&self.entry) {
            server.set_module(module);
        }
        // A failing request breaks into the subscribed client's debugger.
        if let Err(e) = vm.eval_in(ROOT_MODULE, "<techne cloud>", "(set! %host-debugger %nrepl-debug-request)") {
            eprintln!("techne cloud: requests do not break into the debugger: {}", e.msg);
        }
        for (stream, principal) in repl.waiting.drain(..) {
            server.remote().serve_stream_as(stream, principal);
        }
        repl.server = Some((self.generation, server));
    }
}

impl Attached for DevRepl {
    fn poll(&mut self, vm: &mut Vm) -> bool {
        let mut repl = self.repl.borrow_mut();
        if repl.active == self.generation && repl.server.as_ref().is_none_or(|(g, _)| *g != self.generation) {
            self.take_server(&mut repl, vm);
        }
        match &repl.server {
            Some((g, server)) if *g == self.generation => server.poll(vm),
            _ => false,
        }
    }

    fn reap(&mut self, vm: &mut Vm) {
        if let Some((g, server)) = &self.repl.borrow().server
            && *g == self.generation
        {
            server.reap(vm);
        }
    }
}
