//! `techne run FILE` and `techne repl` (or the `techne-vm` binary): run a
//! Scheme file, or start a REPL without one.

use std::{
    path::Path,
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI32, Ordering},
    },
};

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
    let file = args.first().cloned();
    let stopped = signals(vm.interrupt_handle(), file.is_some());
    crate::stdlib::set_command_line(args);
    let result = match file {
        Some(path) => vm.eval_file(Path::new(&path)).map(|_| ()),
        None => {
            crate::repl::run(&mut vm);
            Ok(())
        }
    };
    vm.flush();
    vm.report_jit_stats();
    if std::env::var_os("TECHNE_GC_STATS").is_some() {
        eprintln!("{:?}", vm.heap().stats);
    }
    #[cfg(feature = "allocation-census")]
    if let Some(census) = vm.heap().allocation_census() {
        eprintln!("allocation-census: {census:?}");
    }
    // The process ends here: freeing the world object by object (its
    // symbols, codes, heap blocks) would only delay the exit.
    std::mem::forget(vm);
    // A script the signal stopped, its cleanup done, ends as one a signal
    // ended: 128 and the signal, as a shell says, saying nothing. One that
    // caught it ends as it chose to: its own exit, success, or its error.
    let signal = stopped.load(Ordering::SeqCst);
    if signal != 0
        && let Err(e) = &result
        && e.exit_code().is_none()
        && e.msg == crate::vm::INTERRUPTED
    {
        return ExitCode::from((128 + signal) as u8);
    }
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.exit_code().is_some() => ExitCode::from(e.exit_code().unwrap_or(1) as u8),
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// Watch the signals that ask this to stop, for the VM HANDLE interrupts.
///
/// Ctrl-C (SIGINT) interrupts the evaluation; a second one before that is
/// delivered (while a Rust native runs, say) exits. SIGTERM and SIGHUP
/// interrupt a SCRIPT too, so that it unwinds and its dynamic-wind
/// cleanup runs (call-with-process kills its process, say); a second of
/// them exits at once, cleanup or not. For a script, the last signal is
/// returned, to exit by if the interruption ends it. When no evaluation runs (the script is still being read, say),
/// there is nothing to unwind, and any of them exits. The REPL ends at
/// once on SIGTERM and SIGHUP, as before.
#[cfg(unix)]
fn signals(handle: crate::vm::InterruptHandle, script: bool) -> Arc<AtomicI32> {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
    let stopped = Arc::new(AtomicI32::new(0));
    let Ok(mut signals) = signal_hook::iterator::Signals::new([SIGINT, SIGTERM, SIGHUP]) else { return stopped };
    let seen = stopped.clone();
    let pending = AtomicBool::new(false);
    let terminated = AtomicBool::new(false);
    std::thread::spawn(move || {
        for signal in signals.forever() {
            if signal != SIGINT {
                // The REPL ends at once; a script on the second, even while
                // its cleanup runs.
                if !script || terminated.swap(true, Ordering::SeqCst) {
                    std::process::exit(128 + signal);
                }
            }
            // What a script ends by if the interruption ends it, Ctrl-C's
            // as SIGTERM's.
            if script {
                seen.store(signal, Ordering::SeqCst);
            }
            if pending.swap(true, Ordering::SeqCst) && handle.is_pending() {
                std::process::exit(128 + signal);
            }
            if !handle.interrupt() && script {
                std::process::exit(128 + signal);
            }
        }
    });
    stopped
}

/// Ctrl-C alone, where there are no Unix signals.
#[cfg(not(unix))]
fn signals(handle: crate::vm::InterruptHandle, _script: bool) -> Arc<AtomicI32> {
    let pending = AtomicBool::new(false);
    let _ = ctrlc::set_handler(move || {
        if pending.swap(true, Ordering::SeqCst) && handle.is_pending() {
            std::process::exit(130);
        }
        handle.interrupt();
    });
    Arc::new(AtomicI32::new(0))
}
