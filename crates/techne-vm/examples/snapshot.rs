//! Worlds started from a snapshot of loaded code, as a serverless host
//! starts instances of a deployment, against loading the code into each
//! world. Loads FILES (by default the Org library in `lisp/org`) into a
//! world, snapshots it, then makes N worlds from the snapshot and runs a
//! small request in each. Prints, per world, the time and the memory each
//! way.
//!
//! cargo run --release --features counting-allocator --example snapshot -- [N] [FILES...]

use std::{path::PathBuf, time::Instant};

use techne_vm::{alloc::cage_used, vm::Vm};

/// Resident memory of the process, in bytes.
fn rss() -> usize {
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    statm.split_whitespace().nth(1).and_then(|p| p.parse::<usize>().ok()).unwrap_or(0) * 4096
}

const REQUEST: &str = "(length (org-file-headings (parse-org (list \"* TODO a\" \"** b :work:\" \"text\" \"* c\"))))";

fn main() {
    let mut args = std::env::args().skip(1).peekable();
    let n: usize = args.next_if(|a| a.parse::<usize>().is_ok()).map_or(1000, |a| a.parse().unwrap());
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let files: Vec<PathBuf> = match args.map(PathBuf::from).collect::<Vec<_>>() {
        f if f.is_empty() => ["date", "org", "agenda"].iter().map(|m| root.join(format!("lisp/org/{m}.scm"))).collect(),
        f => f,
    };
    let source: String = files.iter().map(|f| format!("(require {:?})\n", f.canonicalize().unwrap())).collect();
    let lines: usize = files.iter().map(|f| std::fs::read_to_string(f).unwrap().lines().count()).sum();
    let request = if files.iter().any(|f| f.ends_with("org.scm")) { REQUEST } else { "#t" };

    // The first world boots and makes the thread's template; not counted.
    let first = Vm::new();

    // Loading into each world: what a host without snapshots pays.
    let loads = 20;
    let start = Instant::now();
    for _ in 0..loads {
        let mut vm = Vm::new();
        vm.eval_source(&source).unwrap();
    }
    let load = start.elapsed() / loads;

    let mut loaded = Vm::new();
    loaded.eval_source(&source).unwrap();
    let start = Instant::now();
    let snapshot = loaded.snapshot().unwrap();
    let snapped = start.elapsed();

    let (rss0, cage0) = (rss(), cage_used());
    let start = Instant::now();
    let mut worlds: Vec<Vm> = (0..n).map(|_| snapshot.world()).collect();
    let made = start.elapsed();
    let (rss1, cage1) = (rss(), cage_used());

    let start = Instant::now();
    for vm in &mut worlds {
        vm.eval_source(request).unwrap();
    }
    let ran = start.elapsed();
    let (rss2, cage2) = (rss(), cage_used());
    drop(worlds);
    drop(first);

    let per = |d: std::time::Duration| d.as_secs_f64() * 1e6 / n as f64;
    let kib = |b: usize| b as f64 / 1024.0 / n as f64;
    println!("{} files, {lines} lines", files.len());
    println!("  load into a world  {:8.1} µs", load.as_secs_f64() * 1e6);
    println!("  snapshot           {:8.1} µs", snapped.as_secs_f64() * 1e6);
    println!("{n} worlds from the snapshot, per world:");
    println!("  make     {:8.1} µs  RSS {:7.1} KiB  cage {:7.1} KiB", per(made), kib(rss1 - rss0), kib(cage1 - cage0));
    println!("  request  {:8.1} µs  RSS {:7.1} KiB  cage {:7.1} KiB", per(ran), kib(rss2 - rss0), kib(cage2 - cage0));
}
