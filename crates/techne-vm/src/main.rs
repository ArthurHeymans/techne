//! techne-vm: run a Scheme file, or start a REPL without arguments.
use std::{path::Path, process::ExitCode};

use techne_vm::vm::Vm;

fn main() -> ExitCode {
    let mut vm = Vm::new();
    // Ctrl-C interrupts the evaluation; a second one before it is delivered
    // (e.g. while a Rust native runs) exits.
    let handle = vm.interrupt_handle();
    let pending = std::sync::atomic::AtomicBool::new(false);
    let _ = ctrlc::set_handler(move || {
        if pending.swap(true, std::sync::atomic::Ordering::SeqCst) && handle.is_pending() {
            std::process::exit(130);
        }
        handle.interrupt();
    });
    let result = match std::env::args().nth(1) {
        Some(path) => vm.eval_file(Path::new(&path)).map(|_| ()),
        None => {
            techne_vm::repl::run(&mut vm);
            Ok(())
        }
    };
    vm.flush();
    if std::env::var_os("TECHNE_GC_STATS").is_some() {
        eprintln!("{:?}", vm.heap.stats);
    }
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
