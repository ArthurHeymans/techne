//! A world filling its heap cage is refused, as one passing its memory
//! limit is, and the other worlds go on. A test of its own: the cages'
//! capacity (`TECHNE_CAGE_MB`) is read once per process.

use techne_vm::vm::Vm;

/// Cages of 192 MiB for every test here, set before any is made.
fn small_cages() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    // SAFETY: before anything else of the process reads the environment;
    // the tests that may run beside it wait here.
    ONCE.call_once(|| unsafe { std::env::set_var("TECHNE_CAGE_MB", "192") });
}

#[test]
fn a_world_filling_the_cage_is_refused_and_the_others_go_on() {
    small_cages();
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

/// Room is what one allocation can have: with the cage's free bytes in
/// pieces, an object larger than any of them is refused as one past the
/// room left, not taken for a corrupt heap (fuzz seed 103807918).
#[test]
fn an_object_larger_than_every_free_piece_is_refused() {
    let mut vm = fragmented();
    let e = vm.eval_source("(string-length (make-string (* 100 1024 1024) #\\c))").unwrap_err();
    assert!(e.msg.contains("out of memory"), "{e}");
    // The world goes on, and what fits is made.
    let n = vm.eval_source("(string-length (make-string (* 8 1024 1024) #\\d))").unwrap();
    assert_eq!(n.as_int(), Some(8 << 20));
}

/// A string not ASCII has an index, an eighth more than its bytes, which
/// its admission counts: one whose bytes fit the largest piece but not
/// with its index is refused, not made past the room.
#[test]
fn a_string_is_admitted_with_its_index() {
    let mut vm = fragmented();
    let e = vm.eval_source("(string-length (make-string (* 44 1024 1024) #\\é))").unwrap_err();
    assert!(e.msg.contains("out of memory"), "{e}");
    let n = vm.eval_source("(string-length (make-string 1000 #\\é))").unwrap();
    assert_eq!(n.as_int(), Some(1000));
}

/// A world whose cage's free bytes are in pieces: six of 16 MiB freed,
/// each between objects kept, the total free past 100 MiB with the
/// reserve, no piece and not the rest.
fn fragmented() -> Vm {
    small_cages();
    let mut vm = Vm::new();
    vm.set_memory_limit(usize::MAX);
    vm.eval_source(
        "(define kept '())
         (let loop ((i 0))
           (when (< i 6)
             (make-string (* 16 1024 1024) #\\a)
             (set! kept (cons (make-string (* 1024 1024) #\\b) kept))
             (loop (+ i 1))))
         (collect-garbage 'full)",
    )
    .unwrap();
    vm
}
