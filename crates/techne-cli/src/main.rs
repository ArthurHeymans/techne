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
  run FILE [ARGS]          run a Scheme file
  repl                     a Lisp REPL in this terminal
  tool ROOT [FORM [SESSION]] bounded tools for callers; tool --help to begin
  node [ARGS]              serve the node protocol, or nREPL with --nrepl PORT
  lsp                      the language server, on stdin/stdout
  cloud dev FILE           serve a deployment locally, deploying on save
  help                     this text

`techne window --help`, `techne term --help`, `techne eval --help` and
`techne node --help` describe their arguments.";

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
        "run" => match rest() {
            file if file.is_empty() => usage(),
            file => techne_vm::cli::main_with(file, install),
        },
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

#[cfg(feature = "term")]
fn term(args: Vec<String>) -> ExitCode {
    techne_term::cli::main(args)
}

#[cfg(not(feature = "term"))]
fn term(_: Vec<String>) -> ExitCode {
    eprintln!("techne: built without the terminal frontend");
    ExitCode::from(2)
}
