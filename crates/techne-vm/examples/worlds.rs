//! The life of many worlds on one thread: make N (from the thread's
//! template after the first), run a small request in each while all are
//! alive, then drop them. Prints, per world, the time of each phase, the
//! memory the process gained (RSS) and the address space it took (its
//! cage and guard, mostly, which bound how many worlds a process holds).
//!
//! cargo run --release --features counting-allocator --example worlds -- [N]

use std::time::Instant;

use techne_vm::vm::Vm;

/// Resident memory of the process, and its address space, in bytes.
fn memory() -> (usize, usize) {
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    let pages: Vec<usize> = statm.split_whitespace().take(2).filter_map(|p| p.parse().ok()).collect();
    (pages.get(1).copied().unwrap_or(0) * 4096, pages.first().copied().unwrap_or(0) * 4096)
}

fn main() {
    let n: usize = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(1000);
    // The first world boots and makes the template; it is not counted.
    let first = Vm::new();
    let (rss0, space0) = memory();

    let start = Instant::now();
    let mut worlds: Vec<Vm> = (0..n).map(|_| Vm::new()).collect();
    let made = start.elapsed();
    let (rss1, space1) = memory();

    let start = Instant::now();
    for vm in &mut worlds {
        let v = vm.eval_source("(let loop ((i 0) (acc '())) (if (= i 1000) (length acc) (loop (+ i 1) (cons (make-string 8 #\\a) acc))))");
        assert!(v.is_ok());
    }
    let ran = start.elapsed();
    let (rss2, _) = memory();

    let start = Instant::now();
    drop(worlds);
    let dropped = start.elapsed();
    drop(first);

    let per = |d: std::time::Duration| d.as_secs_f64() * 1e6 / n as f64;
    let kib = |b: usize| b as f64 / 1024.0 / n as f64;
    println!("{n} worlds, per world:");
    println!(
        "  make     {:8.1} µs  RSS {:7.1} KiB  address space {:7.1} GiB",
        per(made),
        kib(rss1 - rss0),
        kib(space1 - space0) / (1 << 20) as f64
    );
    println!("  request  {:8.1} µs  RSS {:7.1} KiB", per(ran), kib(rss2 - rss0));
    println!("  drop     {:8.1} µs", per(dropped));
}
