//! Small programs for Miri (`cargo +nightly miri test -p techne-vm --test
//! miri`, see runtime/TECHNE-VM.md), which checks the VM's unsafe code for
//! undefined behaviour: a VM without the prelude, the interpreter only,
//! collections forced at every allocation.

use techne_vm::vm::Vm;

fn vm() -> Vm {
    let mut vm = Vm::bare();
    vm.set_gc_stress(true, false);
    vm
}

fn eval(vm: &mut Vm, src: &str) -> String {
    let v = vm.eval_source(src).unwrap_or_else(|e| panic!("{src}: {e}"));
    vm.repr(&v)
}

#[test]
fn pair_arguments_are_reread_after_collection() {
    let mut vm = vm();
    assert_eq!(eval(&mut vm, "(define p (cons 1 2)) (cons p p)"), "((1 . 2) 1 . 2)");
}

#[test]
fn arithmetic_destinations_follow_collection() {
    let mut vm = vm();
    let src = "(define (calc x)
        (list (+ x x) (+ x 1) (- x 1) (* x x)
              (quotient x 562949953421312)
              (remainder x 562949953421312)
              (modulo (- x) 562949953421312)))
        (calc 1267650600228229682971679916032)";
    let expected = concat!(
        "(2535301200456459365943359832064 1267650600228229682971679916033 ",
        "1267650600228229682971679916031 ",
        "1606938044258990989165808445321182359827701982867954570625024 ",
        "2251799813685248 281474976710656 281474976710656)"
    );
    for full in [false, true] {
        vm.set_gc_stress(true, full);
        assert_eq!(eval(&mut vm, src), expected);
    }
}

#[test]
fn pairs_survive_collections() {
    let mut vm = vm();
    let src = "(define (build i acc) (if (= i 0) acc (build (- i 1) (cons i acc)))) (apply + (build 60 '()))";
    assert_eq!(eval(&mut vm, src), "1830");
}

/// Old objects pointing at young ones (the write barrier), and full
/// collections marking and sweeping them.
#[test]
fn old_objects_hold_young_ones() {
    let mut vm = vm();
    eval(&mut vm, "(define v (make-vector 8 0)) (define t (make-hash-table))");
    vm.collect();
    eval(&mut vm, "(vector-set! v 3 (list 1 2 3)) (hash-table-set! t 'k (vector 4 5))");
    vm.set_gc_stress(true, true);
    let src = "(list (vector-ref v 3) (hash-table-ref/default t 'k #f) (vector->list (make-vector 3 'x)))";
    assert_eq!(eval(&mut vm, src), "((1 2 3) #(4 5) (x x x))");
}

/// Dicts: trie nodes copied while collections move their keys.
#[test]
fn dicts_survive_collections() {
    let mut vm = vm();
    let src = "(define (build i d) (if (= i 0) d (build (- i 1) (dict-set d (list i) (* i i)))))
        (define d (build 40 (dict)))
        (define e (dict-delete (dict-delete d (list 3)) (list 40)))
        (list (dict-size e) (dict-ref e (list 7)) (dict-contains? e (list 3)) (equal? d (dict-set e (list 3) 9))
              {a #{1}})";
    assert_eq!(eval(&mut vm, src), "(38 49 #f #f {a #{1}})");
}

#[test]
fn strings_symbols_and_closures() {
    let mut vm = vm();
    let src = "(define (counter) (let ((n 0)) (lambda () (set! n (+ n 1)) n)))
        (define c (counter)) (c) (c)
        (list (c) (string-append \"ab\" (symbol->string 'cd) (list->string (list #\\e)))
              (string->symbol (substring \"xyzzy\" 1 3)) (number->string (expt 2 70)))";
    assert_eq!(eval(&mut vm, src), "(3 \"abcde\" yz \"1180591620717411303424\")");
}

#[test]
fn tasks_and_errors() {
    let mut vm = vm();
    let src = "(define ch (%make-channel 1 #f))
        (define t (spawn (lambda () (channel-send ch (list 'sent)) (+ 1 2))))
        (list (channel-recv ch) (task-join t) (guard (e (#t (error-object-message e))) (error \"boom\" 1)))";
    assert_eq!(eval(&mut vm, src), "((sent) 3 \"boom\")");
}

/// Large objects (outside the blocks), weak tables, an old pair pointing at
/// young data, and a procedure's constants.
#[test]
fn large_and_weak_objects() {
    let mut vm = vm();
    eval(
        &mut vm,
        "(define b (make-bytevector 40000 7)) (define w (make-weak-hash-table)) (define p (cons 1 2)) (define (g) '(1 (2) \"s\"))",
    );
    vm.collect();
    vm.set_gc_stress(true, true);
    let src = "(bytevector-u8-set! b 39999 9) (set-car! p (list 5)) (hash-table-set! w (list 'k) 1) (hash-table-set! w p 2)
        (define b2 (make-bytevector 40000 1)) (set! b2 #f)
        (list (bytevector-u8-ref b 39999) (car p) (g) (hash-table-ref/default w p #f))";
    assert_eq!(eval(&mut vm, src), "(9 (5) (1 (2) \"s\") 2)");
}

/// The host's values across worlds: refused by another world and after
/// their own is dropped, rooted where a conversion keeps one and then
/// fails, and looked at in place (`Vm::inspect`, `HeapAccess::import`,
/// an async native's `Args`).
#[test]
fn host_values_across_worlds() {
    use std::cell::RefCell;
    use techne_vm::{
        api::{Arg, FromValue, Value},
        vm::Error,
    };
    let mut a = vm();
    let mut b = vm();
    let v = a.eval_source("(list \"in a\" (vector 1 2) 3)").unwrap();
    assert!(b.call_global("car", std::slice::from_ref(&v)).unwrap_err().msg.contains("another world"));
    assert!(b.with_heap(|heap| heap.import(&v).is_none()).unwrap());
    assert_eq!(a.inspect(&v, |l| l.items().map(|items| items[0].as_str().map(str::to_owned))).unwrap(), Some(Some("in a".into())));
    assert!(a.with_heap(|heap| heap.import(&v).and_then(|p| p.as_pair()).is_some()).unwrap());

    // Kept by a conversion that fails: rooted all the same.
    struct Keep;
    thread_local!(static KEPT: RefCell<Option<Value>> = const { RefCell::new(None) });
    impl FromValue for Keep {
        fn from_arg(arg: Arg<'_>) -> Result<Self, Error> {
            KEPT.with(|k| *k.borrow_mut() = Some(arg.value()));
            Err(Error::new("kept, then failed"))
        }
    }
    let s = a.eval_source("(string-append \"ke\" \"pt\")").unwrap();
    assert!(a.get::<Keep>(&s).is_err());
    drop(s);
    eval(&mut a, "(define junk (list (make-vector 50 0) (make-vector 50 1)))");
    a.full_collect();
    let kept = KEPT.with(|k| k.borrow_mut().take()).unwrap();
    assert_eq!(a.repr(&kept), "\"kept\"");

    // An async native reads its arguments from their registers.
    a.register_async("twice", 1, |vm: &mut Vm, args| {
        let n = args.inspect(vm, 0, |s| s.as_str().map(str::len)).ok().flatten();
        std::future::ready(n.map(|n| (2 * n) as i64).ok_or("expected a string"))
    });
    assert_eq!(eval(&mut a, "(twice (make-string 21 #\\a))"), "42");

    // Its world dropped, a value is refused, not read.
    drop(a);
    assert_eq!(b.repr(&v), "#<a value of a dropped world>");
    assert!(b.call_global("car", &[v]).unwrap_err().msg.contains("dropped world"));
}
