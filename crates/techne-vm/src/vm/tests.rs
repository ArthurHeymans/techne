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
