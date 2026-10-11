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
fn heap_values_survive_reserved_allocation() {
    let mut vm = vm();
    vm.heap.stress = false;
    vm.regs[1] = vm.make_string("λ: scoped UTF-8");
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
        vm.regs[1] = vm.alloc_pair(Value::fixnum(1).unwrap(), Value::fixnum(2).unwrap());
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
