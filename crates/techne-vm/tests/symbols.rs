//! Symbol tables are thread-local with a bounded lifetime: the last
//! `Vm` dropped on a thread clears them, so sequential worlds on a
//! reused host thread (serverless workers, test threads) do not
//! accumulate each other's symbols forever. Alone in this binary so no
//! other test's worlds share the thread's table meanwhile — each test
//! still runs on its own thread.

use techne_vm::{reader::interned_symbols, vm::Vm};

fn flood(vm: &mut Vm, n: usize) {
    vm.eval_source(&format!("(do ((i 0 (+ i 1))) ((= i {n})) (string->symbol (string-append \"tenant-symbol-\" (number->string i))))"))
        .unwrap();
}

#[test]
fn tables_clear_when_the_last_world_drops() {
    assert_eq!(interned_symbols(), 0, "test thread starts cleared");
    {
        let mut vm = Vm::new();
        flood(&mut vm, 10_000);
        assert!(interned_symbols() > 10_000, "{} symbols", interned_symbols());
    }
    assert_eq!(interned_symbols(), 0, "symbols survived their world");
    // A fresh world interns from an empty table and works.
    let mut vm = Vm::new();
    let v = vm.eval_source("(symbol->string 'hello-world)").unwrap();
    assert!(format!("{:?}", vm.root(v)).contains("hello-world"));
}

#[test]
fn tables_live_while_any_world_lives() {
    let mut a = Vm::new();
    let mut b = Vm::new();
    flood(&mut a, 1_000);
    let held = interned_symbols();
    drop(a);
    assert_eq!(interned_symbols(), held, "cleared while a world still lives");
    // The surviving world's symbols still resolve.
    let v = b.eval_source("(symbol->string 'still-here)").unwrap();
    assert!(format!("{:?}", b.root(v)).contains("still-here"));
    drop(b);
    assert_eq!(interned_symbols(), 0);
}

#[test]
fn fresh_prelude_after_clear_works() {
    // A Vm::new after a clear must load the prelude and run code that
    // goes through the compiler's core-alias cache (delay/force ride
    // on it) — stale alias ids once made the prelude fail to load.
    for _ in 0..3 {
        let mut vm = Vm::new();
        let v = vm.eval_source("(force (delay (+ 20 22)))").unwrap();
        assert!(format!("{:?}", vm.root(v)).contains("42"));
    }
    assert_eq!(interned_symbols(), 0);
}

#[test]
fn sequential_worlds_do_not_accumulate() {
    let mut peak = 0;
    for _ in 0..5 {
        let mut vm = Vm::new();
        flood(&mut vm, 2_000);
        peak = peak.max(interned_symbols());
    }
    // Each world saw roughly one world's symbols, not five worlds'.
    assert!(peak < 20_000, "peak {peak} suggests accumulation across worlds");
    assert_eq!(interned_symbols(), 0);
}
