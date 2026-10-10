//! Small programs for Miri (`cargo +nightly miri test -p techne-vm --test
//! miri`, see runtime/TECHNE-VM.md), which checks the VM's unsafe code for
//! undefined behaviour: a VM without the prelude, the interpreter only,
//! collections forced at every allocation.

use techne_vm::vm::Vm;

fn vm() -> Vm {
    let mut vm = Vm::bare();
    vm.heap.stress = true;
    vm
}

fn eval(vm: &mut Vm, src: &str) -> String {
    techne_vm::builtins::repr(vm.eval_source(src).unwrap_or_else(|e| panic!("{src}: {e}")))
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
    vm.heap.stress_full = true;
    let src = "(list (vector-ref v 3) (hash-table-ref/default t 'k #f) (vector->list (make-vector 3 'x)))";
    assert_eq!(eval(&mut vm, src), "((1 2 3) #(4 5) (x x x))");
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
    vm.heap.stress_full = true;
    let src = "(bytevector-u8-set! b 39999 9) (set-car! p (list 5)) (hash-table-set! w (list 'k) 1) (hash-table-set! w p 2)
        (define b2 (make-bytevector 40000 1)) (set! b2 #f)
        (list (bytevector-u8-ref b 39999) (car p) (g) (hash-table-ref/default w p #f))";
    assert_eq!(eval(&mut vm, src), "(9 (5) (1 (2) \"s\") 2)");
}
