//! The life of many worlds on one thread: make N (from the thread's
//! template after the first), run a small request in each while all are
//! alive, then drop them. Prints, per world, the time of each phase, the
//! memory the process gained (RSS) and the room each takes in the thread's
//! cage (4 GiB, which bounds how many worlds a thread holds).
//!
//! cargo run --release --features counting-allocator --example worlds -- [N]

use std::time::Instant;

use techne_vm::{alloc::cage_used, vm::Vm};

/// Resident memory of the process, in bytes.
fn rss() -> usize {
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    statm.split_whitespace().nth(1).and_then(|p| p.parse::<usize>().ok()).unwrap_or(0) * 4096
}

fn main() {
    let n: usize = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(1000);
    // The first world boots and makes the template; it is not counted.
    let first = Vm::new();
    let (rss0, cage0) = (rss(), cage_used());

    let start = Instant::now();
    let mut worlds: Vec<Vm> = (0..n).map(|_| Vm::new()).collect();
    let made = start.elapsed();
    let (rss1, cage1) = (rss(), cage_used());

    let start = Instant::now();
    for vm in &mut worlds {
        let v = vm.eval_source("(let loop ((i 0) (acc '())) (if (= i 1000) (length acc) (loop (+ i 1) (cons (make-string 8 #\\a) acc))))");
        assert!(v.is_ok());
    }
    let ran = start.elapsed();
    let (rss2, cage2) = (rss(), cage_used());

    let start = Instant::now();
    drop(worlds);
    let dropped = start.elapsed();
    drop(first);

    let per = |d: std::time::Duration| d.as_secs_f64() * 1e6 / n as f64;
    let kib = |b: usize| b as f64 / 1024.0 / n as f64;
    println!("{n} worlds, per world:");
    println!("  make     {:8.1} µs  RSS {:7.1} KiB  cage {:7.1} KiB", per(made), kib(rss1 - rss0), kib(cage1 - cage0));
    println!("  request  {:8.1} µs  RSS {:7.1} KiB  cage {:7.1} KiB", per(ran), kib(rss2 - rss0), kib(cage2 - cage0));
    println!("  drop     {:8.1} µs", per(dropped));
    println!("  worlds a thread's cage holds after a request: {}", (4usize << 30) / ((cage2 - cage0) / n).max(1));
}
