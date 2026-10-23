//! Exercise the JIT's Rust slow paths under Miri without executing native code.

use super::*;
use crate::builtins::repr;

fn vm() -> Vm {
    let mut vm = Vm::bare();
    let _world = vm.enter_account();
    vm.heap.stress = true;
    vm.stack_top = 4;
    vm
}

#[test]
fn register_growth_refusal_preserves_the_stack() {
    let mut vm = Vm::bare();
    let _world = vm.enter_account();
    let budget = vm.regs.budget().clone();
    let needed = vm.regs.len() + 1;
    let before = (vm.regs.as_ptr(), vm.regs.len(), vm.moves, budget.held());
    vm.regs[0] = Value::TRUE;
    budget.set_limit(budget.held());
    assert!(vm.ensure_regs(needed).unwrap_err().msg.starts_with("out of memory"));
    assert_eq!((vm.regs.as_ptr(), vm.regs.len(), vm.moves, budget.held()), before);
    assert_eq!(vm.regs[0], Value::TRUE);
    budget.set_limit(usize::MAX);
    // The VM's admission policy still refuses before touching the allocator.
    vm.set_memory_limit(vm.held().total());
    let before = (vm.regs.as_ptr(), vm.regs.len(), vm.moves, budget.held());
    assert!(vm.ensure_regs(needed).unwrap_err().msg.starts_with("out of memory"));
    assert_eq!((vm.regs.as_ptr(), vm.regs.len(), vm.moves, budget.held()), before);
    assert_eq!(vm.regs[0], Value::TRUE);
}

#[test]
fn apply_growth_refusal_preserves_live_registers() {
    let mut vm = Vm::bare();
    let _world = vm.enter_account();
    let list = vm.make_list_raw(&[Value::TRUE; 4]);
    let budget = vm.regs.budget().clone();
    vm.regs = Slots::filled_in(Value::VOID, 16, budget.clone()).unwrap();
    vm.regs[12] = vm.apply_native;
    vm.regs[13] = Value::TRUE;
    vm.regs[14] = Value::FALSE;
    vm.regs[15] = list;
    vm.stack_top = 16;
    let before = vm.regs.to_vec();
    budget.set_limit(budget.held());
    assert!(vm.spread_apply(12, 3).unwrap_err().msg.starts_with("out of memory"));
    assert_eq!(&*vm.regs, before.as_slice());
    assert_eq!(vm.stack_top, 16);
}

#[test]
fn tail_call_growth_refusal_preserves_handler_registers() {
    // Exercise both the fixed-arity fast path and enter's extra register at
    // the capacity boundary for a rest-argument callee.
    for (source, size) in [("(lambda (x) x)", 32), ("(lambda args args)", 15)] {
        let mut vm = Vm::bare();
        let _world = vm.enter_account();
        vm.jit_threshold = u32::MAX;
        let callee = vm.eval_source_raw(source).unwrap();
        let callee = vm.root(callee);
        let caller = vm.eval_source_raw("(lambda () #f)").unwrap();
        let caller = vm.root(caller);
        // The closures name their codes by id; rewrite the codes in place.
        let callee_id = unsafe { field(callee.get().as_ptr(), 0) }.as_int() as usize;
        let caller_id = unsafe { field(caller.get().as_ptr(), 0) }.as_int() as usize;
        vm.code_mut(callee_id as u32).frame_size = size;
        {
            let code = vm.code_mut(caller_id as u32);
            code.frame_size = 6;
            code.consts = vec![callee.get()];
            code.ops = vec![
                Op::LoadI { dst: 0, i: 42 },
                Op::LoadK { dst: 4, k: 0 },
                Op::LoadI { dst: 5, i: 8 },
                Op::PushHandler { dst: 3, t: 5 },
                Op::TailCall { base: 4, n: 1 },
                Op::Ret { r: 0 },
            ]
            .into();
        }
        let caller_ptr = vm.closure_code(caller.get());
        let budget = vm.regs.budget().clone();
        vm.regs = Slots::filled_in(Value::VOID, 16, budget.clone()).unwrap();
        vm.regs[0] = caller.get();
        vm.stack_top = 7;
        budget.set_limit(budget.held());
        let result = unsafe { vm.dispatch(caller_ptr, 1) }.unwrap();
        assert_eq!(result, Value::fixnum(42).unwrap(), "{source}");
        assert!(vm.frames.is_empty());
        assert!(vm.handlers.is_empty());

        budget.set_limit(usize::MAX);
        vm.regs[0] = caller.get();
        let result = unsafe { vm.dispatch(caller_ptr, 1) }.unwrap();
        assert_eq!(repr(result), if size == 32 { "8" } else { "(8)" });
        assert!(vm.regs.len() > 16);
        // The synthetic caller's guard stays installed on a successful tail
        // return; real compiled code pops it before returning.
        vm.handlers.clear();
    }
}

#[test]
fn register_growth_charges_its_owner_not_the_entered_world() {
    let mut vm = Vm::bare();
    let _world = vm.enter_account();
    let other = crate::alloc::Account::new().unwrap();
    let budget = vm.regs.budget().clone();
    let before = (budget.held(), other.held());
    let bytes = vm.regs.len() * size_of::<Value>();
    let _entered = other.enter();
    vm.ensure_regs(vm.regs.len() + 1).unwrap();
    assert_eq!(budget.held(), before.0 + bytes);
    assert_eq!(other.held(), before.1);
}

#[test]
fn heap_values_survive_reserved_allocation() {
    let mut vm = vm();
    let _world = vm.enter_account();
    vm.heap.stress = false;
    vm.regs[1] = vm.make_string_raw("λ: scoped UTF-8");
    let raw = vm.regs[1];
    let before = (vm.heap.stats.minor, vm.heap.stats.full);
    vm.with_heap(|mut heap| {
        let value = heap.register(1);
        assert_eq!(heap.as_str(value), Some("λ: scoped UTF-8"));
        assert!(heap.immediate(raw).is_none());
        let pair = heap.try_cons(value, value).unwrap();
        heap.store(2, pair);
        let (a, b) = pair.as_pair().unwrap();
        let text = heap.as_str(value).unwrap();
        assert_eq!((heap.as_str(a), heap.as_str(b)), (Some(text), Some(text)));
        assert_eq!(text, "λ: scoped UTF-8");
    })
    .unwrap();
    assert_eq!((vm.heap.stats.minor, vm.heap.stats.full), before);
    vm.collect();
    vm.with_heap(|heap| {
        assert_eq!(heap.as_str(heap.register(1)), Some("λ: scoped UTF-8"));
        let (a, b) = heap.register(2).as_pair().unwrap();
        assert_eq!((heap.as_str(a), heap.as_str(b)), (Some("λ: scoped UTF-8"), Some("λ: scoped UTF-8")));
    })
    .unwrap();
}

#[test]
fn reserved_allocation_does_not_collect_when_it_cannot_fit() {
    let mut vm = vm();
    let _world = vm.enter_account();
    for stress in [false, true] {
        vm.heap.stress = stress;
        vm.heap.set_nursery_window(0);
        let before = (vm.heap.stats.minor, vm.heap.stats.full, vm.heap.committed());
        vm.with_heap(|mut heap| {
            let nil = heap.immediate(Value::NIL).unwrap();
            // The nursery window is clamped to a minimum size. Exhaust that
            // reservation through the no-GC API before checking failure.
            while heap.try_cons(nil, nil).is_some() {}
            assert!(heap.try_cons(nil, nil).is_none());
            assert!(heap.try_string("no room for a string").is_none());
        })
        .unwrap();
        assert_eq!((vm.heap.stats.minor, vm.heap.stats.full, vm.heap.committed()), before);
    }
    vm.heap.set_nursery_window(vm.heap.nursery_capacity());
    // Stress must still force the slow boundary, even with nursery space.
    vm.with_heap(|mut heap| {
        let nil = heap.immediate(Value::NIL).unwrap();
        assert!(heap.try_cons(nil, nil).is_none());
    })
    .unwrap();
}

#[test]
fn owned_data_can_leave_heap_access() {
    // The value and its world's cage, which it is decoded in.
    assert_eq!(std::mem::size_of::<HeapValue<'_>>(), 2 * std::mem::size_of::<Value>());
    let mut vm = vm();
    let _world = vm.enter_account();
    vm.heap.stress = false;
    let text = vm
        .with_heap(|mut heap| {
            let text = heap.try_string("owned copy").unwrap();
            heap.store(1, text);
            heap.as_str(text).unwrap().to_owned()
        })
        .unwrap();
    vm.regs[1] = Value::VOID;
    vm.collect();
    drop(vm);
    assert_eq!(text, "owned copy");
}

#[test]
fn slow_pair_and_box_operands_survive_collection() {
    let mut vm = vm();
    let _world = vm.enter_account();
    for full in [false, true] {
        vm.heap.stress_full = full;
        vm.regs[1] = vm.alloc_pair_raw(Value::fixnum(1).unwrap(), Value::fixnum(2).unwrap());
        unsafe {
            vm.jit_slow_op(1, Op::Cons { dst: 0, a: 0, b: 0 }).unwrap();
            assert_eq!(repr(vm.regs[1]), "((1 . 2) 1 . 2)");
            vm.jit_slow_op(1, Op::MkBox { r: 0 }).unwrap();
            assert!(is_kind(vm.regs[1], Kind::Box));
            assert_eq!(repr(field(vm.regs[1].as_ptr(), 0)), "((1 . 2) 1 . 2)");
        }
    }
}

#[test]
fn slow_numeric_destinations_follow_collection() {
    let mut vm = vm();
    let _world = vm.enter_account();
    let cases = [
        (Op::Add { dst: 0, a: 0, b: 1 }, 1 << 46, 1 << 46, "140737488355328"),
        (Op::AddI { dst: 0, a: 0, i: 1 }, (1 << 47) - 1, 0, "140737488355328"),
        (Op::Sub { dst: 0, a: 0, b: 1 }, -(1 << 47), 1, "-140737488355329"),
        (Op::Mul { dst: 0, a: 0, b: 1 }, 1 << 46, 4, "281474976710656"),
        (Op::Quo { dst: 0, a: 0, b: 1 }, -(1 << 47), -1, "140737488355328"),
    ];
    for full in [false, true] {
        vm.heap.stress_full = full;
        for (op, a, b, expected) in cases {
            vm.regs[1] = Value::fixnum(a).unwrap();
            vm.regs[2] = Value::fixnum(b).unwrap();
            unsafe { vm.jit_slow_op(1, op).unwrap() };
            assert_eq!(repr(vm.regs[1]), expected, "{op:?}, full={full}");
        }
    }
}

#[test]
fn native_tail_calls_keep_the_caller_and_roots() {
    for jit in if cfg!(miri) { vec![None] } else { vec![None, Some(1)] } {
        let mut vm = Vm::bare();
        vm.set_jit(jit);
        crate::natives! { vm;
            "(%delegate procedure . args)" => |vm: &mut Vm, a, n| {
                let args = vm.regs[a + 1..a + n].to_vec();
                let request = vm.native_tail_call(vm.regs[a], &args);
                // The request owns roots even before dispatch installs it.
                vm.make_string_raw("collect before returning");
                Err(request)
            };
        }
        crate::natives! { vm;
            "(%expand-delegate procedure value)" => |vm: &mut Vm, a, _| {
                let operands = vec![vm.regs[a + 1]; 256];
                Err(vm.native_tail_call(vm.regs[a], &operands))
            };
        }
        vm.set_gc_stress(true, true);
        for (source, expected) in [
            ("(%delegate (lambda (s) s) (string-copy \"rooted\"))", "\"rooted\""),
            ("(define (tail x) (%delegate (lambda (y) (+ y 1)) x)) (tail 41)", "42"),
            ("(define (middle x) (+ 1 (%delegate (lambda (y) (+ y 1)) x))) (middle 40)", "42"),
            ("(define d %delegate) (+ 1 (d + 20 21))", "42"),
            ("(+ 1 (%expand-delegate (lambda args (length args)) #t))", "257"),
            ("(let ((saved (string-copy \"kept\"))) (guard (e (#t saved)) (%expand-delegate (lambda args (car 1)) saved)))", "\"kept\""),
            ("(%delegate apply + '(20 22))", "42"),
            ("(define (loop n) (if (= n 0) 42 (%delegate loop (- n 1)))) (loop 200)", "42"),
        ] {
            let value = vm.eval_source(source).unwrap();
            assert_eq!(vm.repr(&value), expected, "{jit:?}: {source}");
        }
        assert!(vm.eval_source("(%delegate (lambda (x) x))").is_err());
    }
}

#[test]
fn cyclic_applicable_records_raise_promptly() {
    for jit in if cfg!(miri) { vec![None] } else { vec![None, Some(1)] } {
        let mut vm = Vm::bare();
        vm.set_jit(jit);
        let error = vm
            .eval_source(
                "(define type (%make-rtd 'callable '(procedure) 0))
             (define object (%record type #f))
             (%record-set! object type 0 object)
             (define (call-object) (object))
             (call-object)",
            )
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Violation, "{jit:?}");
        assert!(error.msg.starts_with("not a procedure: #<callable"), "{jit:?}: {error}");
    }
}

#[cfg(not(miri))]
#[test]
fn native_tail_calls_can_suspend_in_tasks() {
    for jit in [None, Some(1)] {
        let mut vm = Vm::new();
        vm.set_jit(jit);
        crate::natives! { vm;
            "(%delegate procedure . args)" => |vm: &mut Vm, a, n| {
                let args = vm.regs[a + 1..a + n].to_vec();
                Err(vm.native_tail_call(vm.regs[a], &args))
            };
        }
        for source in [
            "(task-join (spawn (lambda () (%delegate (lambda () (yield) 42)))))",
            "(task-join (spawn (lambda () (+ 1 (%delegate (lambda () (yield) 41))))))",
            "(task-join (spawn (lambda () (apply %delegate (list (lambda () (yield) 42))))))",
        ] {
            let value = vm.eval_source(source).unwrap();
            assert_eq!(vm.repr(&value), "42", "{jit:?}: {source}");
        }
        let native = vm.get_global("%delegate").unwrap();
        let thunk = vm.eval_source("(lambda () (yield) 42)").unwrap();
        let lifetime = vm.new_lifetime(crate::tasks::LifetimeLimits::default());
        let task = vm.spawn_in_with(lifetime, &native, &[thunk]).unwrap();
        for _ in 0..10 {
            vm.run_tasks_for(Duration::from_millis(10)).unwrap();
            if vm.task_result(&task).is_some() {
                break;
            }
        }
        let value = vm.task_result(&task).unwrap().unwrap();
        assert_eq!(vm.repr(&value), "42", "{jit:?}: native task entry");
    }
}

/// A chain of applicable records is followed as deep in a task's entry as
/// in any call: eight records, not nine.
#[test]
fn applicable_records_are_followed_as_deep_in_a_task() {
    let mut vm = Vm::new();
    let chain = |links: usize| {
        format!(
            "(let ((type (%make-rtd 'callable '(procedure) 0)))
               (let loop ((i 0) (f (lambda () 'called)))
                 (if (= i {links}) f (loop (+ i 1) (%record type f)))))"
        )
    };
    for (links, called) in [(8, true), (9, false)] {
        let direct = vm.eval_source(&format!("(guard (e (#t 'refused)) ({}))", chain(links))).unwrap();
        let task = vm.eval_source(&format!("(guard (e (#t 'refused)) (task-join (spawn {})))", chain(links))).unwrap();
        let expected = if called { "called" } else { "refused" };
        assert_eq!((vm.repr(&direct).as_str(), vm.repr(&task).as_str()), (expected, expected), "{links} links");
    }
}

/// Returning restores the root window of the call: a collection after a
/// deep recursion returned scans neither its frames nor what they held.
/// Interpreted, exactly; compiled code calling compiled code keeps the
/// window of at most a run of `jit::MAX_DEPTH` frames, a recursion past
/// it going on in the interpreter.
#[test]
fn a_collection_excludes_returned_frames() {
    for jit in if cfg!(miri) { vec![None] } else { vec![None, Some(1)] } {
        let mut vm = Vm::bare();
        vm.set_jit(jit);
        crate::natives! { vm;
            "(%observe-collection)" => |vm: &mut Vm, _, _| {
                let roots = vm.stack_top;
                vm.heap.stats = Default::default();
                vm.collect_raw();
                Ok(Value::int_unchecked(roots as i64))
            };
        }
        let depth = if cfg!(miri) { 128 } else { 20_000 };
        let value = vm
            .eval_source(&format!(
                "(define seen #f)
                 (define (deep n top)
                   (let ((held (make-vector 8 n)))
                     (if (= n 0) 0
                         (let ((result (deep (- n 1) top)))
                           ;; Back from the frames above it.
                           (when (= n top) (set! seen (%observe-collection)))
                           (+ (vector-ref held 0) result)))))
                 (deep {depth} {depth}) (deep {depth} {depth}) (deep {depth} {depth})
                 seen"
            ))
            .unwrap();
        // Without restoring, some 80,000 slots and 180,000 words copied.
        // Compiled, it would be at most a run of 1000 frames of four slots
        // and their vectors; past it the interpreter restores the window.
        let bound = if jit.is_none() { 64 } else { 8_000 };
        assert!(value.as_int().unwrap() < bound, "{jit:?}: root prefix {}", vm.repr(&value));
        // Only the outermost frame's vector was alive then.
        assert!(vm.heap.stats.max_copied_words < 2 * bound as usize, "{jit:?}: {:?}", vm.heap.stats);
    }
}

#[cfg(not(miri))]
#[test]
fn native_task_entry_unwinds_on_its_task_stack_and_keeps_the_raise_trace() {
    use crate::tasks::{LifetimeLimits, Progress};
    let mut reraised = Vec::new();
    for jit in [None, Some(1)] {
        let mut vm = Vm::new();
        vm.set_jit(jit);
        vm.set_gc_stress(true, true);
        crate::natives! { vm;
            "(%delegate procedure)" => |vm: &mut Vm, a, _| Err(vm.native_tail_call(vm.regs[a], &[]));
            "(%unwind-origin)" => |vm: &mut Vm, _, _| {
                let trace = vm.handlers.iter().rev().find_map(|h| match h {
                    Handler::Unwind { pending } => pending.borrow().as_ref().map(|e| e.trace.clone()),
                    _ => None,
                }).expect("the after-thunk has a pending transfer");
                let bad = trace.iter().filter(|f| f.starts_with("bad ")).count();
                let handler = trace.iter().any(|f| f.contains("%handle-raise"));
                Ok(vm.make_list_raw(&[Value::int_unchecked(bad as i64), Value::bool(handler)]))
            };
        }
        let thunk = vm
            .eval_source(
                "(define gate (make-channel 1)) (define finished #f)
            (define (original-raise) (error \"original\"))
            (lambda () (dynamic-wind (lambda () #f) original-raise
                (lambda () (channel-recv gate) (set! finished #t))))",
            )
            .unwrap();
        let native = vm.get_global("%delegate").unwrap();
        let lifetime = vm.new_lifetime(LifetimeLimits::default());
        let task = vm.spawn_in_with(lifetime, &native, &[thunk]).unwrap();
        assert_eq!(vm.run_tasks_for(Duration::from_secs(1)).unwrap(), Progress::Blocked, "{jit:?}");
        vm.full_collect();
        vm.eval_source("(channel-send gate #t)").unwrap();
        vm.run_tasks().unwrap();
        let error = vm.task_result(&task).unwrap().unwrap_err();
        assert_eq!(error.msg, "original");
        assert_eq!(error.trace.iter().filter(|f| f.starts_with("original-raise ")).count(), 1, "{:?}", error.trace);
        assert!(!error.trace.iter().any(|f| f.contains("<vm helper>")), "{:?}", error.trace);
        assert!(vm.get_global("finished").unwrap().is_truthy());

        // A wind outside a reraising handler must save the first raise's
        // frame limit before removing that handler's mask, on both the
        // uncaught path and a path landing in an enclosing guard.
        let body = "(dynamic-wind (lambda () #f)
            (lambda () (with-exception-handler (lambda (e) (raise e)) bad))
            (lambda () (set! seen (%unwind-origin))))";
        let source = format!("(define seen #f) (define (bad) (raise '(original))) {body}");
        let error = vm.eval_source(&source).unwrap_err();
        let uncaught =
            (error.trace.iter().filter(|f| f.starts_with("bad ")).count(), error.trace.iter().any(|f| f.contains("%handle-raise")));
        let landed = vm.eval_source(&format!("(guard (original (#t #f)) {body}) seen")).unwrap();
        reraised.push((jit, uncaught, vm.repr(&landed)));
    }
    assert_eq!(reraised, vec![(None, (1, false), "(1 #f)".into()), (Some(1), (1, false), "(1 #f)".into())]);
}
