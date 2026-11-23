//! `techne`: one binary for the editor, the Lisp runtime and its servers.
//!
//! The first argument names a subcommand; anything else (none, an option,
//! a file) goes to the editor, so `techne FILE` and `EDITOR="techne
//! --wait"` work as they read. A file named like a subcommand opens with
//! `techne window FILE` (or `./FILE`). The editor's frontends are features:
//! a server built without them has the other subcommands only, and its
//! `techne FILE` is an error.

mod cloud;

use std::process::ExitCode;

const USAGE: &str = "\
usage: techne [SUBCOMMAND] [ARGS...]

  techne [--wait] [FILE]   the editor, in a window (else in this terminal)
  window [ARGS]            the editor in a window
  term [ARGS]              the editor in this terminal
  eval [ARGS] FORM         evaluate FORM in the editor running
  daemon                   the editor with no frontend, for frontends to attach
  attach --stdio [--start] relay to the editor running here (what ssh runs)
  run [ARGS] FILE [ARGS]   run a Scheme file (--locked, --package DIR, --store DIR)
  lock [DIR] [FLAGS]       resolve, hydrate, or verify techne.lock (--check, --frozen, --update)
  repl                     a Lisp REPL in this terminal
  tool ROOT [FORM [SESSION]] bounded tools for callers; tool --help to begin
  node [ARGS]              serve the node protocol, or nREPL with --nrepl PORT
  lsp                      the language server, on stdin/stdout
  cloud dev FILE           serve a deployment locally, deploying on save
  help                     this text

`techne window --help`, `techne term --help`, `techne eval --help`,
`techne run --help`, `techne lock --help` and `techne node --help` describe
their arguments.";

const RUN_USAGE: &str = "\
usage: techne run [--locked] [--package DIR] [--store DIR] FILE [ARGS...]

Run the Scheme FILE with ARGS as its (command-line).

  --locked        resolve all package dependencies strictly offline from
                  techne.lock and the content-addressed tree-sha256 store,
                  verifying every package's tree-sha256 before running
  --package DIR   use DIR as the root package directory instead of searching
                  ancestors of FILE
  --store DIR     override the content-addressed store directory (requires
                  --locked; otherwise $TECHNE_PACKAGE_STORE or
                  $XDG_DATA_HOME/techne/packages/store)";

const LOCK_USAGE: &str = "\
usage: techne lock [DIR] [--check | --frozen | --update] [--store DIR]

Resolve techne-package.scm in DIR (default .), materialize Git dependencies
into the content-addressed tree-sha256 store, reject diamond conflicts and
cycles, and write techne.lock beside techne-package.scm. Existing valid pins
in techne.lock are preserved for unchanged edges unless --update is given.

  --check       verify strictly offline that techne.lock exists, matches
                techne-package.scm and root overrides, and every locked
                package's tree-sha256 matches its directory, without modifying
                techne.lock
  --frozen      materialize missing store entries by the exact pinned commits
                in techne.lock, verify every tree-sha256 and freshness against
                techne-package.scm, and fail if techne.lock is missing or
                stale, without modifying techne.lock
  --update      re-resolve dependencies from their declared refs instead of
                preserving existing pins in techne.lock
  --store DIR   override the content-addressed store directory (otherwise
                $TECHNE_PACKAGE_STORE or $XDG_DATA_HOME/techne/packages/store)";

const EVAL_USAGE: &str = "\
usage: techne eval [--detached] [--module MODULE] [--as NAME] [--timeout SECONDS] FORM

Evaluate FORM, Scheme source (- reads it from stdin), in the editor
running, as emacsclient --eval does: in the session used last, so that
what it shows is shown there, or with --detached in one no frontend
shows. It is evaluated in MODULE, \"user\" by default, as an operation
run for NAME (\"techne eval\" by default). It is killed after SECONDS
(10 by default, at most 300), or by C-g in the editor, which waits for
it meanwhile.

What FORM writes is printed, then its value. An error goes to stderr
with the frames it was raised in, and the exit status is 1, as for one
killed; 2 when no editor is running, or it refused. *Server log* in the
editor says what was evaluated; the option server-eval turns evaluation
off.";

fn main() -> ExitCode {
    // A world past its memory ceiling ends, not the process.
    techne_vm::alloc::contain_out_of_memory();
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let sub = args.first().map(String::as_str).unwrap_or_default().to_owned();
    // A subcommand's `(command-line)` is what follows its name.
    let mut rest = || {
        let rest = args.split_off(1);
        techne_vm::stdlib::set_command_line(rest.clone());
        rest
    };
    match sub.as_str() {
        "window" => window(rest()),
        "term" => term(rest()),
        "eval" => eval(rest()),
        "daemon" => daemon(rest()),
        "attach" => attach(rest()),
        "run" => run_cmd(args.split_off(1)),
        "lock" => lock_cmd(args.split_off(1)),
        "repl" => match rest() {
            none if none.is_empty() => techne_vm::cli::main_with(none, install),
            _ => usage(),
        },
        "tool" => tools(args.split_off(1)),
        "node" => techne_node::cli::main(&["techne", "node"], rest()),
        "cloud" => cloud::main(rest()),
        "lsp" => {
            // Its arguments, `--stdio` as editors give it, change nothing.
            rest();
            techne_lsp::main()
        }
        "help" | "-h" | "--help" => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => editor(args),
    }
}

fn absolutize(path: &std::path::Path) -> Result<std::path::PathBuf, std::io::Error> {
    if path.is_absolute() { Ok(path.to_path_buf()) } else { Ok(std::env::current_dir()?.join(path)) }
}

/// `techne run [--locked] [--package DIR] [--store DIR] FILE [ARGS...]`
fn run_cmd(args: Vec<String>) -> ExitCode {
    let bad = || {
        eprintln!("{RUN_USAGE}");
        ExitCode::from(2)
    };
    let mut options = techne_vm::cli::RunOptions::default();
    let mut it = args.into_iter();
    let mut script_args: Vec<String> = Vec::new();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--locked" => options.locked = true,
            "--package" => {
                let Some(dir) = it.next().filter(|v| !v.is_empty()) else { return bad() };
                options.package_dir = Some(std::path::PathBuf::from(dir));
            }
            "--store" => {
                let Some(dir) = it.next().filter(|v| !v.is_empty()) else { return bad() };
                options.store_dir = Some(std::path::PathBuf::from(dir));
            }
            "-h" | "--help" => {
                println!("{RUN_USAGE}");
                return ExitCode::SUCCESS;
            }
            "--" => {
                script_args.extend(it);
                break;
            }
            s if s.starts_with('-') => return bad(),
            _ => {
                script_args.push(arg);
                script_args.extend(it);
                break;
            }
        }
    }
    if script_args.is_empty() {
        return bad();
    }
    if options.store_dir.is_some() && !options.locked {
        eprintln!("techne run: --store requires --locked");
        return ExitCode::from(2);
    }
    techne_vm::cli::run_with(script_args, options, install)
}

/// `techne lock [DIR] [--check | --frozen | --update] [--store DIR]`
fn lock_cmd(args: Vec<String>) -> ExitCode {
    let bad = || {
        eprintln!("{LOCK_USAGE}");
        ExitCode::from(2)
    };
    let mut dir: Option<std::path::PathBuf> = None;
    let mut check = false;
    let mut frozen = false;
    let mut update = false;
    let mut store: Option<std::path::PathBuf> = None;
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--check" => check = true,
            "--frozen" => frozen = true,
            "--update" => update = true,
            "--store" => {
                let Some(path) = it.next().filter(|v| !v.is_empty()) else { return bad() };
                store = Some(std::path::PathBuf::from(path));
            }
            "-h" | "--help" => {
                println!("{LOCK_USAGE}");
                return ExitCode::SUCCESS;
            }
            s if s.starts_with('-') => return bad(),
            _ if dir.is_none() => dir = Some(std::path::PathBuf::from(arg)),
            _ => return bad(),
        }
    }
    if (check as u8) + (frozen as u8) + (update as u8) > 1 {
        eprintln!("techne lock: --check, --frozen, and --update are mutually exclusive");
        return ExitCode::from(2);
    }
    let raw_dir = dir.unwrap_or_else(|| std::path::PathBuf::from("."));
    let root = match raw_dir.canonicalize() {
        Ok(path) => path,
        Err(e) => {
            eprintln!("techne lock: {}: {e}", raw_dir.display());
            return ExitCode::FAILURE;
        }
    };
    let abs_store = match store {
        Some(s) => match absolutize(&s) {
            Ok(path) => Some(path),
            Err(e) => {
                eprintln!("techne lock: store {}: {e}", s.display());
                return ExitCode::FAILURE;
            }
        },
        None => None,
    };
    let mut vm = techne_vm::vm::Vm::new();
    if let Err(e) = install(&mut vm) {
        eprintln!("{e}");
        return ExitCode::FAILURE;
    }
    vm.set_locked_libraries(true);
    let lock_sld = techne_vm::library::lisp_dir().join("techne/lock.sld");
    if let Err(e) = vm.find_module(&lock_sld.to_string_lossy()) {
        eprintln!("{e}");
        return ExitCode::FAILURE;
    }
    let root_lit = techne_vm::cli::scheme_string_literal(&root.display().to_string());
    let store_arg = match abs_store {
        Some(store_path) => format!(" #:store {}", techne_vm::cli::scheme_string_literal(&store_path.display().to_string())),
        None => String::new(),
    };
    let call = if check {
        format!("((eval 'check-lockfile (environment '(techne lock))) {root_lit}{store_arg})")
    } else if frozen {
        format!("((eval 'lock-project! (environment '(techne lock))) {root_lit} #:frozen #t{store_arg})")
    } else if update {
        format!("((eval 'lock-project! (environment '(techne lock))) {root_lit} #:update #t{store_arg})")
    } else {
        format!("((eval 'lock-project! (environment '(techne lock))) {root_lit}{store_arg})")
    };
    let res = vm.eval_source(&call);
    vm.flush();
    std::mem::forget(vm);
    match res {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// What `run` and `repl` have beyond the language: sockets, processes
/// and nodes.
fn install(vm: &mut techne_vm::vm::Vm) -> Result<(), techne_vm::vm::Error> {
    techne_net::install(vm)?;
    techne_node::install(vm)
}

/// Scheme owns the tool interface and help; this only selects its entry point.
fn tools(args: Vec<String>) -> ExitCode {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            eprintln!("techne tool: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut call = vec![techne_vm::library::lisp_dir().join("tool.scm").to_string_lossy().into_owned(), exe.to_string_lossy().into_owned()];
    call.extend(args);
    techne_vm::cli::main_with(call, install)
}

fn usage() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::from(2)
}

/// The editor in the frontend there is: a window if built with one.
fn editor(args: Vec<String>) -> ExitCode {
    if cfg!(feature = "window") { window(args) } else { term(args) }
}

#[cfg(feature = "window")]
fn window(args: Vec<String>) -> ExitCode {
    techne_window::main(args)
}

#[cfg(not(feature = "window"))]
fn window(_: Vec<String>) -> ExitCode {
    eprintln!("techne: built without the window frontend");
    ExitCode::from(2)
}

/// `techne eval`: evaluate in the editor running, through its socket.
#[cfg(any(feature = "window", feature = "term"))]
fn eval(args: Vec<String>) -> ExitCode {
    use std::io::{Read, Write};
    use techne_editor::server::{self, OpenError, Request};
    let usage = || {
        eprintln!("{EVAL_USAGE}");
        ExitCode::from(2)
    };
    let (mut source, mut module, mut detached, mut principal, mut timeout_ms) = (None, None, false, None, None);
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--detached" => detached = true,
            "--module" | "--as" => {
                let Some(value) = it.next().filter(|v| !v.is_empty()) else { return usage() };
                *(if arg == "--module" { &mut module } else { &mut principal }) = Some(value);
            }
            "--timeout" => match it.next().and_then(|s| s.parse::<f64>().ok()).filter(|s| *s > 0.0) {
                Some(seconds) => timeout_ms = Some((seconds * 1000.0).ceil() as u64),
                None => return usage(),
            },
            "-h" | "--help" => {
                println!("{EVAL_USAGE}");
                return ExitCode::SUCCESS;
            }
            "-" if source.is_none() => {
                let mut text = String::new();
                if let Err(e) = std::io::stdin().read_to_string(&mut text) {
                    eprintln!("techne eval: stdin: {e}");
                    return ExitCode::from(2);
                }
                source = Some(text);
            }
            _ if source.is_none() && !arg.starts_with("--") => source = Some(arg),
            _ => return usage(),
        }
    }
    let Some(source) = source else { return usage() };
    let Some(socket) = server::socket() else {
        eprintln!("techne eval: no editor is running ($XDG_RUNTIME_DIR is not set)");
        return ExitCode::from(2);
    };
    match server::eval(&socket, &Request::Eval { source, module, detached, principal, timeout_ms }) {
        Ok(evaluation) => {
            print!("{}", evaluation.output);
            let _ = std::io::stdout().flush();
            match evaluation.result {
                Ok(value) => {
                    if !value.is_empty() {
                        println!("{value}");
                    }
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::FAILURE
                }
            }
        }
        Err(OpenError::NotRunning) => {
            eprintln!("techne eval: no editor is running");
            ExitCode::from(2)
        }
        Err(OpenError::Failed(e)) => {
            eprintln!("techne eval: {e}");
            ExitCode::from(2)
        }
    }
}

#[cfg(not(any(feature = "window", feature = "term")))]
fn eval(_: Vec<String>) -> ExitCode {
    eprintln!("techne: built without the editor");
    ExitCode::from(2)
}

const DAEMON_USAGE: &str = "\
usage: techne daemon

Run the editor with no frontend, as emacs --daemon does, until SIGTERM,
SIGINT or SIGHUP: windows and terminals attach to it (techne window
--attach, techne term --attach, here or through ssh), and it runs on when
the last detaches. It listens on the editor's socket, as an editor started
in a window does; another editor running there already, it ends. What it
has to say goes to stderr. techne window --attach --start starts one when
none runs.";

/// `techne daemon`: the editor with no frontend, until a signal stops it.
#[cfg(any(feature = "window", feature = "term"))]
fn daemon(args: Vec<String>) -> ExitCode {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use techne_editor::{host::Daemon, server};
    if !args.is_empty() {
        eprintln!("{DAEMON_USAGE}");
        return ExitCode::from(if matches!(args[0].as_str(), "-h" | "--help") { 0 } else { 2 });
    }
    let Some(socket) = server::socket() else {
        eprintln!("techne daemon: no XDG_RUNTIME_DIR for the editor's socket");
        return ExitCode::FAILURE;
    };
    let stop = Arc::new(AtomicBool::new(false));
    for signal in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT, signal_hook::consts::SIGHUP] {
        if let Err(e) = signal_hook::flag::register(signal, stop.clone()) {
            eprintln!("techne daemon: {e}");
            return ExitCode::FAILURE;
        }
    }
    let daemon = Daemon::start(|rt| {
        if let Err(e) = rt.keep_histories(None) {
            eprintln!("techne daemon: history: {e}");
        }
    });
    let listener = match daemon.listen(&socket) {
        Ok(Some(listener)) => listener,
        Ok(None) => {
            eprintln!("techne daemon: an editor is running already on {}", socket.display());
            daemon.stop();
            return ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("techne daemon: {}: {e}", socket.display());
            daemon.stop();
            return ExitCode::FAILURE;
        }
    };
    eprintln!("techne daemon: listening on {} (pid {})", socket.display(), std::process::id());
    let ran = daemon.run(|| stop.load(Ordering::Relaxed));
    listener.stop_accepting();
    daemon.stop();
    // The runtime enqueued the stop notices; their socket writers live in
    // the request threads. Let those threads finish before process exit.
    listener.drain(std::time::Duration::from_secs(1));
    match ran {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("techne daemon: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(any(feature = "window", feature = "term")))]
fn daemon(_: Vec<String>) -> ExitCode {
    eprintln!("techne: built without the editor");
    ExitCode::from(2)
}

const ATTACH_USAGE: &str = "\
usage: techne attach --stdio [--start]

Relay standard input and output to the editor running on this machine:
what techne window --attach --ssh HOST runs on HOST, through ssh. With
--start, an editor is started first (techne daemon) if none runs.";

/// `techne attach --stdio`: relay to the editor's socket.
#[cfg(any(feature = "window", feature = "term"))]
fn attach(args: Vec<String>) -> ExitCode {
    let start = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["--stdio"] => false,
        ["--stdio", "--start"] | ["--start", "--stdio"] => true,
        ["-h" | "--help"] => {
            println!("{ATTACH_USAGE}");
            return ExitCode::SUCCESS;
        }
        _ => {
            eprintln!("{ATTACH_USAGE}");
            return ExitCode::from(2);
        }
    };
    match techne_editor::attach::relay(start) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("techne attach: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(any(feature = "window", feature = "term")))]
fn attach(_: Vec<String>) -> ExitCode {
    eprintln!("techne: built without the editor");
    ExitCode::from(2)
}

#[cfg(feature = "term")]
fn term(args: Vec<String>) -> ExitCode {
    techne_term::cli::main(args)
}

#[cfg(not(feature = "term"))]
fn term(_: Vec<String>) -> ExitCode {
    eprintln!("techne: built without the terminal frontend");
    ExitCode::from(2)
}
