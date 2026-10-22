//! The longest GC pauses of shapes of heap an interactive world may hold,
//! with the nursery window an interactive host sets (2 MiB, as the editor
//! does): the time of the longest pause, and its work in words, which does
//! not depend on the machine.
//!
//! `pauses` runs every shape; `pauses NAME` one. Each builds what it keeps,
//! then allocates garbage for a while, as a world does that keeps working:
//! the old generation's cycles mark what is kept in slices meanwhile.

use std::time::Instant;

use techne_vm::vm::Vm;

/// What a shape keeps, then the garbage made after (`churn`).
const SHAPES: &[(&str, &str)] = &[
    // A baseline: many small objects, as buffers and lists are.
    ("lists", "(define kept (let loop ((i 0) (acc '())) (if (= i 200000) acc (loop (+ i 1) (cons (list i (number->string i)) acc)))))"),
    // One vector of a million pointers: a slice must stop within it.
    (
        "big-vector",
        "(define kept (let ((v (make-vector 1000000 #f))) (do ((i 0 (+ i 1))) ((= i 1000000) v) (vector-set! v i (cons i i)))))",
    ),
    // A chain of ephemerons: each value is the key of the next, only the
    // first key held: a collection must not go over every entry per link.
    (
        "weak-chain",
        "(define table (make-weak-hash-table))
         (define first (list 0))
         (define kept (let loop ((i 0) (key first))
                        (if (= i 20000) table
                            (let ((next (list (+ i 1)))) (hash-table-set! table key next) (loop (+ i 1) next)))))",
    ),
    // Many weak tables, each a few entries: what identity tables and
    // inspectors keep.
    (
        "weak-tables",
        "(define kept (let loop ((i 0) (acc '()))
                        (if (= i 20000) acc
                            (let ((t (make-weak-hash-table)) (k (list i)))
                              (hash-table-set! t k (list k)) (loop (+ i 1) (cons (cons k t) acc))))))",
    ),
];

/// Work after: objects that outlive the nursery and die old, as a
/// world's do, so that old-generation cycles run while it works.
const CHURN: &str = "(let ((ring (make-vector 100000 #f)))
  (let loop ((i 0)) (when (< i 3000000) (vector-set! ring (modulo i 100000) (cons i (make-string 8 #\\x))) (loop (+ i 1)))))";

fn run(name: &str, keep: &str) {
    // Fresh stats: the pauses measured are of the building and the
    // churn both, the largest of a world that keeps working.
    let mut vm = Vm::new();
    vm.set_nursery_window(2 << 20);
    vm.eval_source(keep).unwrap_or_else(|e| panic!("{name}: {e}"));
    let built = vm.heap().stats.full;
    let start = Instant::now();
    vm.eval_source(CHURN).unwrap_or_else(|e| panic!("{name}: {e}"));
    let s = &vm.heap().stats;
    println!(
        "{name:12} {:7.2} ms longest pause  {:9} words marked in one  {:7} words copied in one  ({} cycles, {:.0} ms)",
        s.max_pause.as_secs_f64() * 1e3,
        s.max_slice_words,
        s.max_copied_words,
        s.full - built,
        start.elapsed().as_secs_f64() * 1e3
    );
}

fn main() {
    let only = std::env::args().nth(1);
    SHAPES.iter().filter(|(name, _)| only.as_deref().is_none_or(|o| o == *name)).for_each(|(name, keep)| run(name, keep));
}
