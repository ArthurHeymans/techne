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
  run FILE [ARGS]          run a Scheme file
  repl                     a Lisp REPL in this terminal
  node [ARGS]              serve the node protocol, or nREPL with --nrepl PORT
  lsp                      the language server, on stdin/stdout
  cloud dev FILE           serve a deployment locally, deploying on save
  help                     this text

`techne window --help`, `techne term --help` and `techne node --help`
describe their arguments.";

fn main() -> ExitCode {
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
        "run" => match rest() {
            file if file.is_empty() => usage(),
            file => techne_vm::cli::main_with(file, install),
        },
        "repl" => match rest() {
            none if none.is_empty() => techne_vm::cli::main_with(none, install),
            _ => usage(),
        },
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

/// What `run` and `repl` have beyond the language: sockets and processes.
fn install(vm: &mut techne_vm::vm::Vm) -> Result<(), techne_vm::vm::Error> {
    techne_net::install(vm)?;
    techne_process::install(vm)
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

#[cfg(feature = "term")]
fn term(args: Vec<String>) -> ExitCode {
    techne_term::cli::main(args)
}

#[cfg(not(feature = "term"))]
fn term(_: Vec<String>) -> ExitCode {
    eprintln!("techne: built without the terminal frontend");
    ExitCode::from(2)
}
