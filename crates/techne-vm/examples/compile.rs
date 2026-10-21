//! The front end's cost, by phase: reading the prelude's source, and
//! loading it (reading, expanding, compiling and running its definitions)
//! into bare worlds. The prelude is the workload because it is large,
//! define- and macro-heavy and self-contained: a bare world plus the
//! prelude is what `Vm::boot` builds, and what loading any shipped Lisp
//! library does file by file.
//!
//! `compile --list` names the workloads; `compile NAME` runs one and
//! prints how many top-level forms it timed. Under callgrind, collecting
//! only in `timed_forms`, that run counts their instructions, which
//! `runtime/bench/icount.sh` reports per form. Without arguments, it
//! prints each workload's wall time per form.
//!
//! The worlds of a timed run are made before it, so installing the
//! builtins is not counted; their symbols and globals are the builtins'
//! only, as on a first load. Reading reuses the thread's symbol table,
//! so after the first pass its symbols are interned already: the `read`
//! workload measures scanning and building data, not interning new names.

use std::time::Instant;

use techne_vm::{
    PRELUDE, PRELUDE_PATH, reader,
    vm::{ROOT_MODULE, Vm},
};

struct Workload {
    name: &'static str,
    /// Passes over the prelude in the timed run, after one warm-up pass.
    passes: usize,
    load: bool,
}

const WORKLOADS: &[Workload] = &[Workload { name: "read", passes: 40, load: false }, Workload { name: "load", passes: 12, load: true }];

fn read_pass() -> usize {
    reader::read_located(PRELUDE).expect("the prelude reads").len()
}

/// A bare world with the JIT off: what loading measures is the front
/// end and running the definitions, not native compilation of the few
/// prelude procedures a load happens to call often.
fn bare() -> Vm {
    let mut vm = Vm::bare();
    vm.set_jit(None);
    vm
}

fn load_pass(vm: &mut Vm) {
    vm.eval_in(ROOT_MODULE, PRELUDE_PATH, PRELUDE).expect("the prelude loads");
}

/// What the benchmark counts: `passes` over the prelude, in worlds (for
/// `load`) made beforehand, one per pass.
#[inline(never)]
fn timed_forms(w: &Workload, worlds: &mut [Vm]) {
    if w.load {
        for vm in worlds.iter_mut().take(w.passes) {
            load_pass(vm);
        }
    } else {
        for _ in 0..w.passes {
            read_pass();
        }
    }
}

/// The forms of `w`'s timed run (after a warm-up pass), and its wall time.
fn run(w: &Workload) -> (usize, f64) {
    let per_pass = read_pass();
    if w.load {
        load_pass(&mut bare());
    }
    let mut worlds: Vec<Vm> = if w.load { (0..w.passes).map(|_| bare()).collect() } else { Vec::new() };
    let start = Instant::now();
    timed_forms(w, &mut worlds);
    (per_pass * w.passes, start.elapsed().as_secs_f64())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--list") {
        WORKLOADS.iter().for_each(|w| println!("{}", w.name));
        return;
    }
    if let Some(name) = args.first() {
        let w = WORKLOADS.iter().find(|w| w.name == name).unwrap_or_else(|| panic!("no workload {name}"));
        // The number of forms timed, for dividing instructions by.
        println!("{}", run(w).0);
        return;
    }
    for w in WORKLOADS {
        let (forms, secs) = run(w);
        println!("{:<10} {:>5} forms {:>8.2} µs per form", w.name, forms, secs * 1e6 / forms as f64);
    }
}
