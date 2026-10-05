//! Register-machine instructions and compiled code objects.
//!
//! Registers are `u16` offsets from the frame base `bp`. The callee's closure
//! lives at `bp - 1`; a call places the callee at `base` and its arguments at
//! `base + 1..`, so the callee frame starts at `bp + base + 1` without copying.
//! Jump targets are absolute instruction indices.

use std::rc::Rc;

use crate::value::Value;

pub type Reg = u16;

#[derive(Clone, Copy, Debug)]
pub enum Op {
    LoadK { dst: Reg, k: u32 },
    LoadI { dst: Reg, i: i32 },
    Mov { dst: Reg, src: Reg },
    GetG { dst: Reg, g: u32 },
    SetG { g: u32, src: Reg },
    /// Read a captured value from the current closure.
    GetC { dst: Reg, i: u16 },
    /// Read through a captured box.
    GetCB { dst: Reg, i: u16 },
    SetCB { i: u16, src: Reg },
    /// Replace the register's value with a fresh box containing it.
    MkBox { r: Reg },
    Unbox { dst: Reg, r: Reg },
    SetBox { r: Reg, src: Reg },
    Closure { dst: Reg, code: u32 },
    /// Install a `guard` handler: on a raise, unwind to this frame, store the
    /// condition in `dst` and continue at `t`.
    PushHandler { dst: Reg, t: u32 },
    PopHandler,
    /// `call/cc` (escape-only): put a fresh continuation in `k` and install an
    /// escape point; invoking the continuation stores its value in `dst` and
    /// continues at `t`.
    PushEscape { k: Reg, dst: Reg, t: u32 },

    Jmp { t: u32 },
    /// Loop back-edge: a jump that also counts towards task preemption.
    Loop { t: u32 },
    /// Jump if false / true.
    Jf { c: Reg, t: u32 },
    Jt { c: Reg, t: u32 },
    /// Fused compare-and-branch: jump when the comparison is false.
    JNLt { a: Reg, b: Reg, t: u32 },
    JNLe { a: Reg, b: Reg, t: u32 },
    JNNumEq { a: Reg, b: Reg, t: u32 },
    JNEq { a: Reg, b: Reg, t: u32 },
    JNNull { a: Reg, t: u32 },
    /// Compare with a small immediate and jump when false.
    JNLtI { a: Reg, i: i16, t: u32 },
    JNGtI { a: Reg, i: i16, t: u32 },
    JNEqI { a: Reg, i: i16, t: u32 },
    JNPair { a: Reg, t: u32 },

    Add { dst: Reg, a: Reg, b: Reg },
    AddI { dst: Reg, a: Reg, i: i16 },
    Sub { dst: Reg, a: Reg, b: Reg },
    Mul { dst: Reg, a: Reg, b: Reg },
    Quo { dst: Reg, a: Reg, b: Reg },
    Rem { dst: Reg, a: Reg, b: Reg },
    Mod { dst: Reg, a: Reg, b: Reg },
    Lt { dst: Reg, a: Reg, b: Reg },
    Le { dst: Reg, a: Reg, b: Reg },
    NumEq { dst: Reg, a: Reg, b: Reg },
    Car { dst: Reg, a: Reg },
    Cdr { dst: Reg, a: Reg },
    Cons { dst: Reg, a: Reg, b: Reg },
    NullP { dst: Reg, a: Reg },
    PairP { dst: Reg, a: Reg },
    Not { dst: Reg, a: Reg },
    EqP { dst: Reg, a: Reg, b: Reg },
    VRef { dst: Reg, v: Reg, i: Reg },
    VSet { v: Reg, i: Reg, x: Reg },

    Call { base: Reg, n: u16 },
    /// Call the procedure in global `g`; arguments at `base + 1..`.
    CallG { base: Reg, n: u16, g: u32 },
    TailCallG { base: Reg, n: u16, g: u32 },
    TailCall { base: Reg, n: u16 },
    Ret { r: Reg },
    /// A loop head of JIT-compiled code: run native code from here (the
    /// original instruction is in `Code::jit`).
    EnterJit,
}

/// Where a closure's captured value comes from in the creating frame.
#[derive(Clone, Copy, Debug)]
pub enum CapSrc {
    Reg(Reg),
    Cap(u16),
}

#[derive(Debug)]
pub struct Code {
    pub name: Rc<str>,
    pub ops: Vec<Op>,
    pub consts: Vec<Value>,
    pub nparams: u16,
    pub rest: bool,
    pub frame_size: u16,
    pub captures: Vec<CapSrc>,
    /// Source file index (`Vm::files`).
    pub file: u32,
    /// Source position of each instruction (`reader::NO_POS` if unknown).
    pub spans: Vec<u32>,
    /// Position of the defining lambda, parameter names and docstring.
    pub pos: u32,
    pub params: Vec<Rc<str>>,
    pub doc: Option<Rc<str>>,
    pub jit: crate::jit::JitSlot,
}

#[cfg(test)]
#[test]
fn op_size() {
    assert!(std::mem::size_of::<Op>() <= 12, "keep instructions small");
}
