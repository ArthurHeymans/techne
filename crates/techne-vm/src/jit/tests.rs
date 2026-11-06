//! The bounded feedback and kind optimizations, including their exits.
use super::*;

fn eval(vm: &mut Vm, source: &str) -> String {
    let value = vm.eval_source(source).unwrap_or_else(|e| panic!("{source}: {e}"));
    vm.repr(&value)
}

/// Observe targets before the one synchronous compilation. Threshold 1
/// alone compiles a function before its first indirect site has run.
fn observed(source: &str, warm: &str) -> Vm {
    let mut vm = Vm::new();
    vm.set_jit(Some(1_000_000));
    eval(&mut vm, source);
    eval(&mut vm, warm);
    vm.set_jit(Some(1));
    vm
}

#[test]
fn register_iteration_is_sparse_ordered_and_crosses_words() {
    for n in [0, 1, 63, 64, 65, 129] {
        let mut regs = Regs::empty(n);
        assert_eq!(regs.iter().next(), None);
        let expected: Vec<_> = (0..n).filter(|r| r % 3 == 0 || r % 64 == 63).collect();
        for &r in &expected {
            regs.insert(r as Reg);
        }
        assert_eq!(regs.iter().collect::<Vec<_>>(), expected);
        assert_eq!(Regs::full(n).iter().collect::<Vec<_>>(), (0..n).collect::<Vec<_>>());
    }
}

#[test]
fn global_anticipation_includes_implicit_self_tail_edges() {
    for tail in [Op::TailCallG { base: 0, n: 0, g: 99 }, Op::TailCall { base: 0, n: 0 }] {
        let ops = [Op::CallG { base: 0, n: 0, g: 7 }, Op::CallG { base: 0, n: 0, g: 8 }, Op::Mov { dst: 0, src: 0 }, tail];
        let succs: Vec<_> = ops.iter().enumerate().map(|(pc, op)| successors(op, pc, ops.len())).collect();
        let inline = |pc| pc == 0;
        assert_eq!(anticipated(&ops, &[7], &succs, inline, false), [1, 0, 0, 0]);
        // The non-inline mutator at pc 1 must reload global 7 on return,
        // even though its next use is beyond the implicit pc-0 edge.
        assert_eq!(anticipated(&ops, &[7], &succs, inline, true), [1, 0, 1, 1]);
    }
}

#[test]
fn feedback_is_monomorphic_bounded_and_does_not_respecialize() {
    let mut feedback = CallFeedback::default();
    feedback.observe(12, CallTarget::Code(7));
    feedback.observe(12, CallTarget::Code(7));
    feedback.observe(20, CallTarget::Native(3));
    assert_eq!(feedback.iter().collect::<Vec<_>>(), [(12, CallTarget::Code(7)), (20, CallTarget::Native(3))]);
    feedback.observe(12, CallTarget::Code(8));
    feedback.observe(12, CallTarget::Code(7));
    feedback.observe(30, CallTarget::Code(9));
    feedback.observe(40, CallTarget::Other);
    feedback.observe(50, CallTarget::Code(10));
    assert_eq!(feedback.iter().collect::<Vec<_>>(), [(20, CallTarget::Native(3)), (30, CallTarget::Code(9))]);
}

#[test]
fn pair_facts_do_not_survive_overwriting_the_operand() {
    let ops =
        [Op::Cons { dst: 0, a: 1, b: 2 }, Op::Car { dst: 3, a: 0 }, Op::Cdr { dst: 0, a: 0 }, Op::Car { dst: 4, a: 0 }, Op::Ret { r: 4 }];
    let captures = HashMap::new();
    let shape = Shape {
        effects: ops.iter().map(|op| uses_defs(op, &captures)).collect(),
        succs: ops.iter().enumerate().map(|(pc, op)| successors(op, pc, ops.len())).collect(),
        entries: HashMap::new(),
        inlined: vec![false; ops.len()],
        results: HashMap::new(),
    };
    let facts = pairs(&ops, 5, &captures, &[0], &shape);
    assert!(!facts[0].contains(0));
    assert!(facts[1].contains(0));
    assert!(facts[2].contains(0));
    assert!(!facts[3].contains(0));
}

const DRIVE: &str = r#"
    (define (drive f n)
      (let loop ((i 0) (sum 0))
        (if (= i n) sum (loop (+ i 1) (+ sum (f i))))))
    (define (make-adder k) (lambda (x) (+ x k)))
    (define first (make-adder 1))
    (define second (make-adder 7))
"#;

#[test]
fn indirect_inline_uses_the_actual_closure_and_falls_back_on_other_code() {
    let mut vm = observed(DRIVE, "(drive first 8)");
    let _world = vm.enter_account();
    assert_eq!(eval(&mut vm, "(drive first 1024)"), "524800");
    assert!(code_for_test(&mut vm, "drive").jit.entry.get().is_some());
    let deps = code_for_test(&mut vm, "drive").jit.deps.borrow().clone();
    assert!(deps.iter().any(|&id| &*vm.code(id).name == "lambda"), "indirect callee retained");
    assert_eq!(eval(&mut vm, "(drive second 1024)"), "530944");
    assert_eq!(eval(&mut vm, "(drive (lambda (x) (* x 2)) 1024)"), "1047552");
    assert_eq!(eval(&mut vm, "(drive - 1024)"), "-523776");
    vm.full_collect();
    assert_eq!(eval(&mut vm, "(drive first 1024)"), "524800");
}

#[test]
fn native_indirect_sites_guard_the_target_and_keep_generic_arity_errors() {
    let source = "(define (fold f n) (let loop ((i 0) (a 0)) (if (= i n) a (loop (+ i 1) (f a i)))))";
    let mut vm = observed(source, "(fold + 8)");
    let _world = vm.enter_account();
    assert_eq!(eval(&mut vm, "(fold + 1024)"), "523776");
    assert!(code_for_test(&mut vm, "fold").jit.entry.get().is_some());
    assert_eq!(eval(&mut vm, "(fold - 1024)"), "-523776");
    assert_eq!(eval(&mut vm, "(guard (e (else 'caught)) (fold car 10))"), "caught");
    assert_eq!(eval(&mut vm, "(fold (lambda (a b) (+ a b 1)) 1024)"), "524800");
}

#[test]
fn an_indirect_inline_closure_stays_rooted_across_native_gc() {
    let source =
        format!("{DRIVE}\n(define (make-reader k) (lambda (x) (collect-garbage) (+ x (car k))))\n(define reader (make-reader (list 3)))");
    let mut vm = observed(&source, "(drive reader 8)");
    let _world = vm.enter_account();
    assert_eq!(eval(&mut vm, "(drive reader 512)"), "132352");
    assert!(code_for_test(&mut vm, "drive").jit.entry.get().is_some());
    assert_eq!(eval(&mut vm, "(drive (make-reader (list 9)) 512)"), "135424");
}

#[test]
fn indirect_inline_exits_after_effects_never_replay_the_call() {
    let source = format!(
        "{DRIVE}\n(define hits 0)\n(define (make-f k) (lambda (x) (set! hits (+ hits 1)) (+ x (car k))))\n(define good (make-f (list 2)))"
    );
    let mut vm = observed(&source, "(drive good 8)");
    let _world = vm.enter_account();
    assert_eq!(eval(&mut vm, "(drive good 1024)"), "525824");
    assert!(code_for_test(&mut vm, "drive").jit.entry.get().is_some());
    assert_eq!(eval(&mut vm, "(set! hits 0) (guard (e (else hits)) (drive (make-f (list 'bad)) 10))"), "1");
}

#[test]
fn indirect_inline_errors_keep_the_interpreters_logical_frames() {
    let source = format!(
        "{DRIVE}\n(define hits 0)\n(define (read-cell k) (car k))\n(define (make-f k) (lambda (x) (set! hits (+ hits 1)) (collect-garbage) (+ x (read-cell k))))\n(define good (make-f (list 2)))"
    );
    let run = |compiled: bool| {
        let mut vm = observed(&source, "(drive good 8)");
        let _world = vm.enter_account();
        if !compiled {
            vm.set_jit(None);
        }
        eval(&mut vm, "(drive good 1024)");
        let error = vm.eval_source("(drive (make-f (list 'bad)) 10)").unwrap_err();
        (error.msg.clone(), error.trace.clone())
    };
    assert_eq!(run(true), run(false));
}

#[test]
fn feedback_keeps_code_but_not_the_sampled_closure_or_its_captures() {
    let source = format!(
        "{DRIVE}\n(define weak (make-weak-hash-table))\n(define key (list 12))\n(define callback (let ((key key)) (lambda (x) (+ x (car key)))))\n(hash-table-set! weak key #t)"
    );
    let mut vm = observed(&source, "(drive callback 8)");
    let _world = vm.enter_account();
    assert_eq!(eval(&mut vm, "(drive callback 1024)"), "536064");
    let deps = code_for_test(&mut vm, "drive").jit.deps.borrow().clone();
    assert!(!deps.is_empty());
    eval(&mut vm, "(set! callback #f) (set! key #f)");
    vm.full_collect();
    vm.full_collect();
    assert_eq!(eval(&mut vm, "(hash-table-count weak)"), "0");
    assert!(deps.iter().all(|&id| vm.code_live(id)), "only code dependencies stay alive");
}

#[test]
fn a_boxed_capture_belongs_to_each_actual_closure() {
    let source =
        format!("{DRIVE}\n(define (make-counter) (let ((k 0)) (lambda (x) (set! k (+ k 1)) (+ x k))))\n(define counter (make-counter))");
    let mut vm = observed(&source, "(drive counter 8)");
    let _world = vm.enter_account();
    assert_eq!(eval(&mut vm, "(drive counter 512)"), "266240");
    vm.full_collect();
    assert_eq!(eval(&mut vm, "(drive (make-counter) 512)"), "262144");
}

#[test]
fn indirect_tail_calls_guard_code_but_do_not_grow_the_native_stack() {
    let source = r#"
        (define (tail f x) (f x))
        (define (up x) (+ x 1))
        (define (down x) (- x 1))
        (define (repeat f) (let loop ((i 0)) (if (= i 1024) (tail f i) (begin (tail f i) (loop (+ i 1))))))
    "#;
    let mut vm = observed(source, "(repeat up)");
    let _world = vm.enter_account();
    assert_eq!(eval(&mut vm, "(repeat up)"), "1025");
    assert!(code_for_test(&mut vm, "tail").jit.entry.get().is_some());
    assert_eq!(eval(&mut vm, "(repeat down)"), "1023");
    assert_eq!(eval(&mut vm, "(repeat -)"), "-1024");
}

#[test]
fn fixnum_inline_results_are_checked_after_a_target_change() {
    let source = format!("{DRIVE}\n(define (inc x) (+ x 1))");
    let mut vm = observed(&source, "(drive inc 8)");
    let _world = vm.enter_account();
    assert_eq!(eval(&mut vm, "(drive inc 1024)"), "524800");
    assert!(code_for_test(&mut vm, "drive").jit.entry.get().is_some());
    assert_eq!(eval(&mut vm, "(drive (lambda (x) (+ x 0.5)) 1024)"), "524288.0");
    assert_eq!(eval(&mut vm, "(drive inc 1024)"), "524800");
}

#[test]
fn float_inline_results_are_checked_after_a_target_change() {
    let source = r#"
        (define (float-result x) (+ x 0.5))
        (define (drive-float f n)
          (let loop ((i 0) (sum 0.0))
            (if (= i n) sum (loop (+ i 1) (+ sum (f i))))))
    "#;
    let mut vm = observed(source, "(drive-float float-result 8)");
    let _world = vm.enter_account();
    assert_eq!(eval(&mut vm, "(drive-float float-result 1024)"), "524288.0");
    assert!(code_for_test(&mut vm, "drive-float").jit.entry.get().is_some());
    assert_eq!(eval(&mut vm, "(drive-float (lambda (x) x) 1024)"), "523776.0");
    assert_eq!(eval(&mut vm, "(drive-float float-result 1024)"), "524288.0");
}
