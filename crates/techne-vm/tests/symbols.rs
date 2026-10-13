//! Symbol tables are thread-local with a lifetime bounded per world:
//! interning while a world executes records it as a user, and its drop
//! frees every symbol no live world uses — so worlds on a reused host
//! thread (serverless workers, test threads) do not accumulate each
//! other's symbols, whether they run in sequence or overlap. Alone in
//! this binary so no other test's worlds share the thread's table
//! meanwhile — each test still runs on its own thread.

use techne_vm::{reader::interned_symbols, vm::Vm};

fn flood(vm: &mut Vm, n: usize, prefix: &str) {
    vm.eval_source(&format!("(do ((i 0 (+ i 1))) ((= i {n})) (string->symbol (string-append \"{prefix}-\" (number->string i))))")).unwrap();
}

/// What the table holds with no world of this test alive: measured
/// after a warmup world, once one-time gen-0 internings (builtin
/// registration outside any execution) have settled.
fn baseline() -> usize {
    drop(Vm::new());
    interned_symbols()
}

#[test]
fn tables_clear_when_the_last_world_drops() {
    let base = baseline();
    {
        let mut vm = Vm::new();
        flood(&mut vm, 10_000, "tenant-symbol");
        assert!(interned_symbols() > base + 10_000, "{} symbols", interned_symbols());
    }
    assert_eq!(interned_symbols(), base, "symbols survived their world");
    // A fresh world works with the table as it finds it.
    let mut vm = Vm::new();
    let v = vm.eval_source("(symbol->string 'hello-world)").unwrap();
    assert!(format!("{:?}", vm.root(v)).contains("hello-world"));
}

#[test]
fn a_dropped_worlds_symbols_go_while_another_lives() {
    let base = baseline();
    let mut a = Vm::new();
    let mut b = Vm::new();
    flood(&mut a, 1_000, "only-in-a");
    let held = interned_symbols();
    assert!(held > base + 1_000);
    drop(a);
    // A's tenant symbols are freed although B lives on; B's own stay.
    assert!(interned_symbols() < held - 500, "{} of {held} left", interned_symbols());
    let v = b.eval_source("(symbol->string 'still-here)").unwrap();
    assert!(format!("{:?}", b.root(v)).contains("still-here"));
    drop(b);
    assert_eq!(interned_symbols(), base);
}

#[test]
fn an_anchor_world_does_not_retain_other_worlds_symbols() {
    let base = baseline();
    let _anchor = Vm::new();
    let anchored = interned_symbols();
    for i in 0..5 {
        let mut vm = Vm::new();
        flood(&mut vm, 2_000, &format!("tenant-{i}"));
        assert!(interned_symbols() > anchored + 2_000);
        drop(vm);
        // Back to what the anchor itself holds, every round.
        assert_eq!(interned_symbols(), anchored, "round {i}");
    }
    drop(_anchor);
    assert_eq!(interned_symbols(), base);
}

#[test]
fn fresh_prelude_after_clear_works() {
    // A Vm::new after another world dropped must load the prelude and
    // run code that goes through the compiler's core-alias cache
    // (delay/force ride on it) — stale alias ids once made the prelude
    // fail to load; freed-and-reused ids must not bring that back.
    let base = baseline();
    for _ in 0..3 {
        let mut vm = Vm::new();
        let v = vm.eval_source("(force (delay (+ 20 22)))").unwrap();
        assert!(format!("{:?}", vm.root(v)).contains("42"));
    }
    assert_eq!(interned_symbols(), base);
}

#[test]
fn sequential_worlds_do_not_accumulate() {
    let base = baseline();
    let mut peak = 0;
    for i in 0..5 {
        let mut vm = Vm::new();
        flood(&mut vm, 2_000, &format!("round-{i}"));
        peak = peak.max(interned_symbols());
    }
    // Each world saw roughly one world's symbols, not five worlds'.
    assert!(peak < base + 20_000, "peak {peak} suggests accumulation across worlds");
    assert_eq!(interned_symbols(), base);
}
