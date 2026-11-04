//! `techne run FILE` and `techne repl` (or the `techne-vm` binary): run a
//! Scheme file, or start a REPL without one.

use std::{path::Path, process::ExitCode};

use crate::vm::Vm;

/// Run the file ARGS names, its other arguments being its `(command-line)`
/// after it; or, with no arguments, the REPL on the terminal.
pub fn main(args: Vec<String>) -> ExitCode {
    main_with(args, |_| Ok(()))
}

/// [`main`], in a world SETUP has added to: the other crates' natives,
/// for a command that has them.
pub fn main_with(args: Vec<String>, setup: impl FnOnce(&mut Vm) -> Result<(), crate::vm::Error>) -> ExitCode {
    let mut vm = Vm::new();
    if let Err(e) = setup(&mut vm) {
        eprintln!("{e}");
        return ExitCode::FAILURE;
    }
    // Ctrl-C interrupts the evaluation; a second one before it is delivered
    // (e.g. while a Rust native runs) exits.
    #[cfg(not(target_os = "hermit"))]
    {
        let handle = vm.interrupt_handle();
        let pending = std::sync::atomic::AtomicBool::new(false);
        let _ = ctrlc::set_handler(move || {
            if pending.swap(true, std::sync::atomic::Ordering::SeqCst) && handle.is_pending() {
                std::process::exit(130);
            }
            handle.interrupt();
        });
    }
    let file = args.first().cloned();
    crate::stdlib::set_command_line(args);
    let result = match file {
        Some(path) => vm.eval_file(Path::new(&path)).map(|_| ()),
        None => {
            crate::repl::run(&mut vm);
            Ok(())
        }
    };
    vm.flush();
    if std::env::var_os("TECHNE_GC_STATS").is_some() {
        eprintln!("{:?}", vm.heap().stats);
    }
    // The process ends here: freeing the world object by object (its
    // symbols, codes, heap blocks) would only delay the exit.
    std::mem::forget(vm);
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.exit_code().is_some() => ExitCode::from(e.exit_code().unwrap_or(1) as u8),
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
