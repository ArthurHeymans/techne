//! `techne run FILE` and `techne repl` (or the `techne-vm` binary): run a
//! Scheme file, or start a REPL without one.

use std::{
    path::{Path, PathBuf},
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

/// Options controlling how [`run_with`] resolves a script's package world.
#[derive(Default, Clone, Debug)]
pub struct RunOptions {
    /// Require and verify `techne.lock` strictly offline before running.
    pub locked: bool,
    /// Explicit package root directory instead of searching above the script.
    pub package_dir: Option<PathBuf>,
    /// Override the content-addressed package store directory.
    pub store_dir: Option<PathBuf>,
}

/// [`main`], in a world SETUP has added to: the other crates' natives,
/// for a command that has them.
pub fn main_with(args: Vec<String>, setup: impl FnOnce(&mut Vm) -> Result<(), crate::vm::Error>) -> ExitCode {
    run_with(args, RunOptions::default(), setup)
}

/// [`main_with`] with explicit package resolution [`RunOptions`].
pub fn run_with(args: Vec<String>, options: RunOptions, setup: impl FnOnce(&mut Vm) -> Result<(), crate::vm::Error>) -> ExitCode {
    let mut vm = Vm::new();
    if let Err(e) = setup(&mut vm) {
        eprintln!("{e}");
        return ExitCode::FAILURE;
    }
    let file = args.first().cloned();
    let stopped = signals(vm.interrupt_handle(), file.is_some());
    crate::stdlib::set_command_line(args);
    let result = match file {
        Some(path) => {
            let prepare = if options.locked {
                use_locked_package_of(&mut vm, Path::new(&path), options.package_dir.as_deref(), options.store_dir.as_deref())
            } else if let Some(pkg_dir) = options.package_dir.as_deref() {
                use_package_dir(&mut vm, pkg_dir)
            } else {
                use_package_of(&mut vm, Path::new(&path))
            };
            prepare.and_then(|()| vm.eval_file(Path::new(&path)).map(|_| ()))
        }
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

/// Format a Rust string as a valid R7RS Scheme string literal.
///
/// Uses the VM reader's own serializer rather than Rust's `{:?}`, which emits
/// `\'` and `\u{...}` escapes that Scheme does not accept.
pub fn scheme_string_literal(s: &str) -> String {
    crate::reader::string_repr(s)
}

/// The directory of the package the file at PATH is in: the nearest one
/// above it holding a manifest, `techne-package.scm`.
pub fn package_root(path: &Path) -> Option<PathBuf> {
    let file = path.canonicalize().ok()?;
    file.ancestors().skip(1).find(|dir| dir.join("techne-package.scm").is_file()).map(Path::to_path_buf)
}

/// Make the package in DIR known to VM via `(techne package)`.
pub fn use_package_dir(vm: &mut Vm, dir: &Path) -> Result<(), crate::vm::Error> {
    let root = dir.canonicalize().map_err(|e| crate::vm::Error::new(format!("package directory {}: {e}", dir.display())))?;
    vm.find_module(&crate::library::lisp_dir().join("techne/package.sld").to_string_lossy())?;
    vm.eval_source(&format!(
        "((eval 'use-package-directory! (environment '(techne package))) {})",
        scheme_string_literal(&root.display().to_string())
    ))
    .map(|_| ())
}

/// Make the package the file at PATH is in known to VM, with those it
/// depends on, as `(techne package)` reads its manifest: the file then
/// imports their libraries. A file in no package changes nothing, and
/// loads no code.
///
/// `(techne package)` is loaded from Techne's own Lisp
/// (`library::lisp_dir`), by its file: an import of it from here, which
/// is no file, would look in the working directory first, and run what a
/// `techne/package.sld` there says before the program asked for anything.
pub fn use_package_of(vm: &mut Vm, path: &Path) -> Result<(), crate::vm::Error> {
    let Some(root) = package_root(path) else { return Ok(()) };
    use_package_dir(vm, &root)
}

/// Verify `techne.lock` strictly offline and wire all locked packages into VM.
///
/// Loads `(techne lock)` directly from Techne's own Lisp directory (never CWD),
/// checks `techne.lock` against `techne-package.scm` and every locked package's
/// `tree-sha256` without running Git or touching the network, and registers
/// every package's library provider and direct import boundary.
pub fn use_locked_package_of(
    vm: &mut Vm,
    path: &Path,
    package_dir: Option<&Path>,
    store_dir: Option<&Path>,
) -> Result<(), crate::vm::Error> {
    let root = match package_dir {
        Some(dir) => dir.canonicalize().map_err(|e| crate::vm::Error::new(format!("package directory {}: {e}", dir.display())))?,
        None => package_root(path)
            .ok_or_else(|| crate::vm::Error::new(format!("techne run --locked: no techne-package.scm found for {}", path.display())))?,
    };
    if !root.join("techne-package.scm").is_file() {
        return Err(crate::vm::Error::new(format!("techne run --locked: no techne-package.scm in {}", root.display())));
    }
    vm.set_locked_libraries(true);
    vm.find_module(&crate::library::lisp_dir().join("techne/lock.sld").to_string_lossy())?;
    let root_lit = scheme_string_literal(&root.display().to_string());
    let call = match store_dir {
        Some(store) => {
            let abs_store = if store.is_absolute() {
                store.to_path_buf()
            } else {
                std::env::current_dir().map_err(|e| crate::vm::Error::new(format!("current directory: {e}")))?.join(store)
            };
            format!(
                "((eval 'use-locked-package! (environment '(techne lock))) {root_lit} #:store {})",
                scheme_string_literal(&abs_store.display().to_string())
            )
        }
        None => format!("((eval 'use-locked-package! (environment '(techne lock))) {root_lit})"),
    };
    vm.eval_source(&call).map(|_| ())
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
