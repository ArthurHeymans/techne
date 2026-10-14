//! Exercise the JIT's Rust slow paths under Miri without executing native code.

use super::*;
use crate::builtins::repr;

fn vm() -> Vm {
    let mut vm = Vm::bare();
    vm.heap.stress = true;
    vm.stack_top = 4;
    vm
}

#[test]
fn register_growth_refusal_preserves_the_stack() {
    let mut vm = Vm::bare();
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
        vm.jit_threshold = u32::MAX;
        let callee = vm.eval_source_raw(source).unwrap();
        let callee = vm.root(callee);
        let caller = vm.eval_source_raw("(lambda () #f)").unwrap();
        let caller = vm.root(caller);
        // The closures name their codes by id; rewrite the codes in place.
        let callee_id = unsafe { field(callee.get().as_ptr(), 0) }.as_int() as usize;
        let caller_id = unsafe { field(caller.get().as_ptr(), 0) }.as_int() as usize;
        vm.codes[callee_id].as_mut().unwrap().frame_size = size;
        {
            let code = vm.codes[caller_id].as_mut().unwrap();
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
    });
    assert_eq!((vm.heap.stats.minor, vm.heap.stats.full), before);
    vm.collect();
    vm.with_heap(|heap| {
        assert_eq!(heap.as_str(heap.register(1)), Some("λ: scoped UTF-8"));
        let (a, b) = heap.register(2).as_pair().unwrap();
        assert_eq!((heap.as_str(a), heap.as_str(b)), (Some("λ: scoped UTF-8"), Some("λ: scoped UTF-8")));
    });
}

#[test]
fn reserved_allocation_does_not_collect_when_it_cannot_fit() {
    let mut vm = vm();
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
        });
        assert_eq!((vm.heap.stats.minor, vm.heap.stats.full, vm.heap.committed()), before);
    }
    vm.heap.set_nursery_window(vm.heap.nursery_capacity());
    // Stress must still force the slow boundary, even with nursery space.
    vm.with_heap(|mut heap| {
        let nil = heap.immediate(Value::NIL).unwrap();
        assert!(heap.try_cons(nil, nil).is_none());
    });
}

#[test]
fn owned_data_can_leave_heap_access() {
    assert_eq!(std::mem::size_of::<HeapValue<'_>>(), std::mem::size_of::<Value>());
    let mut vm = vm();
    vm.heap.stress = false;
    let text = vm.with_heap(|mut heap| {
        let text = heap.try_string("owned copy").unwrap();
        heap.store(1, text);
        heap.as_str(text).unwrap().to_owned()
    });
    vm.regs[1] = Value::VOID;
    vm.collect();
    drop(vm);
    assert_eq!(text, "owned copy");
}

#[test]
fn slow_pair_and_box_operands_survive_collection() {
    let mut vm = vm();
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
