//! A world filling its heap cage is refused, as one passing its memory
//! limit is, and the other worlds go on. A test of its own: the cages'
//! capacity (`TECHNE_CAGE_MB`) is read once per process.

use techne_vm::vm::Vm;

#[test]
fn a_world_filling_the_cage_is_refused_and_the_others_go_on() {
    // SAFETY: before anything else of the process reads the environment.
    unsafe { std::env::set_var("TECHNE_CAGE_MB", "192") };
    let mut bystander = Vm::new();
    bystander.eval_source("(define kept (make-vector 1000 'kept))").unwrap();

    // No limit of its own: only the cage bounds it.
    let mut hog = Vm::new();
    hog.set_memory_limit(usize::MAX);
    let grow = "(define held '()) (let loop () (set! held (cons (make-vector 1000 0) held)) (loop))";
    let e = hog.eval_source(grow).unwrap_err();
    assert!(e.msg.contains("heap cage"), "{e}");
    // Catching the refusals and going on ends in a kill.
    let e = hog
        .eval_source("(let retry () (guard (e (#t (retry))) (let loop () (set! held (cons (make-vector 1000 0) held)) (loop))))")
        .unwrap_err();
    assert!(e.is_kill(), "{e}");

    // The other world allocates still, as much as the room left allows.
    let n = bystander.eval_source("(vector-length (vector-map (lambda (x) x) kept))").unwrap();
    assert_eq!(n.as_int(), Some(1000));

    // Dropping the hog frees its room, for a new world.
    drop(hog);
    let mut next = Vm::new();
    let v = next.eval_source("(length (let loop ((i 0) (l '())) (if (= i 10000) l (loop (+ i 1) (cons (make-vector 100 i) l)))))").unwrap();
    assert_eq!(v.as_int(), Some(10000));
}
