//! Data copied out of one world and into another: every kind of data
//! arrives as it was, under collections at every allocation too; what is
//! not data is refused, and says what it is.

use techne_vm::{data::Data, vm::Vm};

const LIMIT: usize = 1 << 24;

/// Worlds that collect at every allocation, minor and full, and one that
/// does not.
fn worlds() -> Vec<(&'static str, Vm)> {
    let mut minor = Vm::new();
    minor.set_gc_stress(true, false);
    let mut full = Vm::new();
    full.set_gc_stress(true, true);
    vec![("plain", Vm::new()), ("minor", minor), ("full", full)]
}

/// SRC evaluated in one world, copied into another and written there.
fn copied(from: &mut Vm, to: &mut Vm, src: &str) -> String {
    let v = from.eval_source(src).unwrap_or_else(|e| panic!("{src}: {e}"));
    let data = from.to_data(&v, LIMIT).unwrap_or_else(|e| panic!("{src}: {e}"));
    let back = to.from_data(&data).unwrap_or_else(|e| panic!("{src}: {e}"));
    assert_eq!(to.to_data(&back, LIMIT).unwrap(), data, "{src}");
    // Written as the original is, a dict's entries in the same order.
    assert_eq!(to.repr(&back), from.repr(&v), "{src}");
    to.repr(&back)
}

#[test]
fn data_arrives_as_it_was() {
    for (mode, mut to) in worlds() {
        let mut from = Vm::new();
        for (src, written) in [
            ("#t", "#t"),
            ("-42", "-42"),
            ("4611686018427387904", "4611686018427387904"),
            ("-9223372036854775808", "-9223372036854775808"),
            ("1.5", "1.5"),
            ("#\\é", "#\\é"),
            ("\"naïve\"", "\"naïve\""),
            ("#u8(0 255)", "#u8(0 255)"),
            ("'sym", "sym"),
            ("'#:key", "#:key"),
            ("'()", "()"),
            ("'(1 (2 #(3 \"x\")) {a 1})", "(1 (2 #(3 \"x\")) {a 1})"),
            ("{\"headers\" {\"host\" \"x\"} body #u8(1)}", "{body #u8(1) \"headers\" {\"host\" \"x\"}}"),
        ] {
            assert_eq!(copied(&mut from, &mut to, src), written, "{mode}: {src}");
        }
        // A large copy, collected under as it is made.
        let big = "(let loop ((i 0) (acc '())) (if (= i 2000) acc (loop (+ i 1) (cons (vector i (number->string i) (dict 'i i)) acc))))";
        let v = from.eval_source(big).unwrap();
        let data = from.to_data(&v, LIMIT).unwrap();
        let back = to.from_data(&data).unwrap();
        assert_eq!(to.to_data(&back, LIMIT).unwrap(), data, "{mode}");
        let check =
            to.eval_source("(lambda (l) (and (= (length l) 2000) (equal? (car l) (vector 1999 \"1999\" (dict 'i 1999)))))").unwrap();
        assert!(to.call(&check, &[back]).unwrap().is_truthy(), "{mode}");
    }
}

/// What is not data is refused, cycles and deep nesting too.
#[test]
fn what_is_not_data() {
    let mut vm = Vm::new();
    for (src, says) in [
        ("car", "data"),
        ("(lambda () 1)", "data"),
        ("(begin (define-record-type point (point x) point? (x point-x)) (point 1))", "data"),
        ("(list 1 (lambda () 1))", "data"),
        ("(cons 1 2)", "proper list"),
        ("(let ((l (list 1 2))) (set-cdr! (cdr l) l) l)", "proper list"),
        ("(expt 2 64)", "integer of 64 bits"),
        ("1/2", "data"),
        ("(let loop ((i 0) (v '())) (if (= i 1001) v (loop (+ i 1) (list v))))", "nested more than"),
        ("(dict (list 1) 2)", "dict keyed by"),
        ("(let loop ((i 0) (d (dict))) (if (= i 4097) d (loop (+ i 1) (dict-set d i i))))", "more than 4096 entries"),
        // Forty levels of a vector holding the one below twice: a few
        // objects, a copy of 2^40 parts.
        ("(let loop ((i 0) (v '())) (if (= i 40) v (loop (+ i 1) (vector v v))))", "size limit"),
    ] {
        let v = vm.eval_source(src).unwrap_or_else(|e| panic!("{src}: {e}"));
        let e = vm.to_data(&v, LIMIT).expect_err(src);
        assert!(e.msg.contains(says), "{src}: {}", e.msg);
    }
    let mut deep = Data::List(vec![]);
    for _ in 0..1001 {
        deep = Data::List(vec![deep]);
    }
    assert!(vm.from_data(&deep).unwrap_err().msg.contains("nested more than"));
    let wide = Data::Dict((0..4097).map(|i| (Data::Int(i), Data::Int(i))).collect());
    assert!(vm.from_data(&wide).unwrap_err().msg.contains("more than 4096 entries"));
    let keyed = Data::Dict(vec![(Data::Vector(vec![]), Data::Int(1))]);
    assert!(vm.from_data(&keyed).unwrap_err().msg.contains("dict keyed by"));
    // Past the world's memory: refused before anything is made.
    vm.set_memory_limit(8 << 20);
    let huge = Data::List(vec![Data::String("x".repeat(1 << 20)); 64]);
    assert!(vm.from_data(&huge).unwrap_err().msg.contains("out of memory"));
}

/// What is copied out is counted against the limit as it would be made,
/// before a container's parts are listed: a long symbol's name too, and a
/// million-item list given a small limit.
#[test]
fn the_limit_of_a_copy() {
    let mut vm = Vm::new();
    let long = vm.eval_source("(string->symbol (make-string 1000 #\\a))").unwrap();
    assert!(vm.to_data(&long, 512).unwrap_err().msg.contains("size limit"));
    assert!(vm.to_data(&long, 2048).is_ok());
    let wide = vm.eval_source("(make-list 1000000 1)").unwrap();
    assert!(vm.to_data(&wide, 4096).unwrap_err().msg.contains("size limit"));
    // A world that refused data is as it was.
    let three = vm.eval_source("(+ 1 2)").unwrap();
    assert_eq!(vm.repr(&three), "3");
}

/// A value of another world is no value here.
#[test]
fn values_of_other_worlds() {
    let mut a = Vm::new();
    let mut b = Vm::new();
    let v = a.eval_source("(list 1 2)").unwrap();
    assert!(b.to_data(&v, LIMIT).is_err());
}
