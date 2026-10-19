//! Each world has symbols of its own: what one interns, another's table
//! never holds, and they go with the world. A world made from a template
//! starts with a copy of the template's. Symbols cross worlds by name.

use techne_vm::{reader::interned_symbols, vm::Vm};

fn flood(vm: &mut Vm, n: usize, prefix: &str) {
    vm.eval_source(&format!("(do ((i 0 (+ i 1))) ((= i {n})) (string->symbol (string-append \"{prefix}-\" (number->string i))))")).unwrap();
}

/// How many symbols `vm` holds.
fn symbols(vm: &Vm) -> usize {
    let _world = vm.enter_account();
    interned_symbols()
}

#[test]
fn a_worlds_symbols_are_its_own() {
    let mut a = Vm::new();
    let b = Vm::new();
    let (in_a, in_b) = (symbols(&a), symbols(&b));
    assert_eq!(in_a, in_b, "both copies of the template's");
    flood(&mut a, 2_000, "only-in-a");
    assert!(symbols(&a) >= in_a + 2_000);
    assert_eq!(symbols(&b), in_b, "another world's symbols are not B's");
    // Nor a world made after: it starts from the template, not from A.
    assert_eq!(symbols(&Vm::new()), in_b);
}

#[test]
fn host_registration_belongs_to_the_receiving_world() {
    // Registering in B while A is entered: the name is B's, as host-facing
    // methods enter the world they are called on.
    let a = Vm::new();
    let mut b = Vm::new();
    {
        let _entered = a.enter_account();
        b.register_fn("only-in-b-xyz", |x: i64| x + 1);
        b.set_global("only-in-b-var", &techne_vm::api::Value::from(7)).unwrap();
    }
    drop(a);
    let v = b.eval_source("(+ (only-in-b-xyz 41) only-in-b-var)").unwrap();
    assert_eq!(v.as_int(), Some(49));
}

#[test]
fn symbols_cross_worlds_by_name() {
    let mut a = Vm::new();
    let mut b = Vm::new();
    flood(&mut b, 100, "only-in-b");
    let name = a.eval_source("'made-in-a").unwrap();
    let f = b.eval_source("(lambda (s) (list (symbol? s) (symbol->string s) (eq? s 'made-in-a)))").unwrap();
    let v = b.call(&f, &[name]).unwrap();
    assert_eq!(b.repr(&v), "(#t \"made-in-a\" #t)");
}

#[test]
fn macros_work_in_every_world() {
    // The compiler refers to root-module names by aliases each world
    // makes in its own table (`delay` and `force` ride on them).
    let mut a = Vm::new();
    a.eval_source("(define in-a (delay (+ 1 2)))").unwrap();
    let mut b = Vm::new();
    b.eval_source("(define (twice x) (force (delay (+ x x))))").unwrap();
    drop(a);
    let v = b.eval_source("(twice 21)").unwrap();
    assert_eq!(v.as_int(), Some(42));
}
