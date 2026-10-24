//! Register-machine instructions and compiled code objects.
//!
//! Registers are `u16` offsets from the frame base `bp`. The callee's closure
//! lives at `bp - 1`; a call places the callee at `base` and its arguments at
//! `base + 1..`, so the callee frame starts at `bp + base + 1` without copying.
//! Jump targets are absolute instruction indices.

use std::{ptr::NonNull, rc::Rc};

use crate::value::Value;

pub type Reg = u16;

#[derive(Clone, Copy, Debug)]
pub enum Op {
    LoadK {
        dst: Reg,
        k: u32,
    },
    LoadI {
        dst: Reg,
        i: i32,
    },
    Mov {
        dst: Reg,
        src: Reg,
    },
    GetG {
        dst: Reg,
        g: u32,
    },
    SetG {
        g: u32,
        src: Reg,
    },
    /// Read a captured value from the current closure.
    GetC {
        dst: Reg,
        i: u16,
    },
    /// Read through a captured box.
    GetCB {
        dst: Reg,
        i: u16,
    },
    SetCB {
        i: u16,
        src: Reg,
    },
    /// Replace the register's value with a fresh box containing it.
    MkBox {
        r: Reg,
    },
    Unbox {
        dst: Reg,
        r: Reg,
    },
    SetBox {
        r: Reg,
        src: Reg,
    },
    Closure {
        dst: Reg,
        code: u32,
    },
    /// Install a `guard` handler: on a raise, unwind to this frame, store the
    /// condition in `dst` and continue at `t`.
    PushHandler {
        dst: Reg,
        t: u32,
    },
    PopHandler,
    /// `call/cc` (escape-only): put a fresh continuation in `k` and install an
    /// escape point; invoking the continuation stores its value in `dst` and
    /// continues at `t`.
    PushEscape {
        k: Reg,
        dst: Reg,
        t: u32,
    },

    Jmp {
        t: u32,
    },
    /// Loop back-edge: a jump that also counts towards task preemption.
    Loop {
        t: u32,
    },
    /// Jump if false / true.
    Jf {
        c: Reg,
        t: u32,
    },
    Jt {
        c: Reg,
        t: u32,
    },
    /// Fused compare-and-branch: jump when the comparison is false.
    JNLt {
        a: Reg,
        b: Reg,
        t: u32,
    },
    JNLe {
        a: Reg,
        b: Reg,
        t: u32,
    },
    JNNumEq {
        a: Reg,
        b: Reg,
        t: u32,
    },
    JNEq {
        a: Reg,
        b: Reg,
        t: u32,
    },
    JNNull {
        a: Reg,
        t: u32,
    },
    /// Compare with a small immediate and jump when false.
    JNLtI {
        a: Reg,
        i: i16,
        t: u32,
    },
    JNGtI {
        a: Reg,
        i: i16,
        t: u32,
    },
    JNEqI {
        a: Reg,
        i: i16,
        t: u32,
    },
    JNPair {
        a: Reg,
        t: u32,
    },

    Add {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    AddI {
        dst: Reg,
        a: Reg,
        i: i16,
    },
    Sub {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Mul {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Quo {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Rem {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Mod {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Lt {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Le {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    NumEq {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Car {
        dst: Reg,
        a: Reg,
    },
    Cdr {
        dst: Reg,
        a: Reg,
    },
    Cons {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    NullP {
        dst: Reg,
        a: Reg,
    },
    PairP {
        dst: Reg,
        a: Reg,
    },
    Not {
        dst: Reg,
        a: Reg,
    },
    EqP {
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    VRef {
        dst: Reg,
        v: Reg,
        i: Reg,
    },
    VSet {
        v: Reg,
        i: Reg,
        x: Reg,
    },

    Call {
        base: Reg,
        n: u16,
    },
    /// Call the procedure in global `g`; arguments at `base + 1..`.
    CallG {
        base: Reg,
        n: u16,
        g: u32,
    },
    TailCallG {
        base: Reg,
        n: u16,
        g: u32,
    },
    TailCall {
        base: Reg,
        n: u16,
    },
    Ret {
        r: Reg,
    },
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

/// A lexical source call removed by the tree optimizer. Names/positions,
/// not world values or code addresses: safe to share in compilation caches.
#[derive(Clone, Debug)]
pub struct InlineScope {
    pub name: Rc<str>,
    pub parent: Option<u32>,
    pub call_pos: u32,
    pub tail: bool,
}

/// A bytecode interval executing within a logical inline frame. Sparse and
/// sorted; ordinary code pays no per-instruction metadata cost.
#[derive(Clone, Copy, Debug)]
pub struct InlineSite {
    pub start: u32,
    pub end: u32,
    pub scope: u32,
    /// A logical tail call emitted as a physical non-tail call because its
    /// caller is inlined. Omit that virtual frame only after entering a callee.
    pub tail_call: bool,
}

/// Fully immutable from installation, including across snapshot worlds.
#[derive(Debug)]
pub struct InlineMap {
    pub scopes: Box<[InlineScope]>,
    pub sites: Box<[InlineSite]>,
}

#[derive(Debug)]
#[repr(C)]
pub struct Code {
    /// Its handle on the heap (`Kind::Code`), which keeps what the code
    /// refers to alive and whose death frees the code. First, since the
    /// collector reaches it through a closure's code address.
    pub handle: std::cell::Cell<Value>,
    pub name: Rc<str>,
    /// Can be shared between codes (`Shared`), as a template's are with the
    /// worlds started from it (`Code::instance_copy`): compiled code
    /// patching it makes it its own first, and an interpreter running the
    /// shared buffer keeps reading that one, which stays valid and means
    /// the same.
    pub ops: Shared<Op>,
    pub consts: Vec<Value>,
    pub nparams: u16,
    pub rest: bool,
    pub frame_size: u16,
    pub captures: Shared<CapSrc>,
    /// Source file index (`Vm::files`).
    pub file: u32,
    /// Source position of each instruction (`reader::NO_POS` if unknown).
    pub spans: Shared<u32>,
    pub inlines: Option<Rc<InlineMap>>,
    /// Position of the defining lambda, parameter names and docstring.
    pub pos: u32,
    pub params: Shared<Rc<str>>,
    pub doc: Option<Rc<str>>,
    pub jit: crate::jit::JitSlot,
    /// The package generation of the module it was compiled in (0: none),
    /// for retiring a generation's code.
    pub generation: u32,
    /// For a procedure defined at the root module's top level: the position
    /// of its `define` form in `file`, from which calls may inline it.
    pub definition: Option<u32>,
    /// That definition as an inline template, once asked for.
    pub inline: std::cell::OnceCell<Option<Rc<crate::compiler::Inline>>>,
    /// The modules whose globals it uses (set by `Vm::add_code`): a retired
    /// module lives while a live code uses it.
    pub uses: Box<[u32]>,
    /// What it takes, charged to its world's budget while it lives (set
    /// by `Vm::add_code`, `Budget::charge`).
    pub(crate) charged: Option<crate::alloc::Reservation>,
}

impl Code {
    /// The bytes it takes of its own: the code and its buffers, but those
    /// it shares with a template's, and compiled code (`jit::Jit::held`).
    /// Its strings (its name, docstring and parameters' names) and where
    /// it runs others inline are its own if it is `fresh` (just compiled),
    /// a template's if it was copied from one.
    pub(crate) fn own_bytes(&self, fresh: bool) -> usize {
        let strings = || {
            let inlines = self.inlines.as_ref().map_or(0, |m| {
                m.scopes.iter().map(|s| size_of::<InlineScope>() + s.name.len()).sum::<usize>() + m.sites.len() * size_of::<InlineSite>()
            });
            self.name.len() + self.doc.as_ref().map_or(0, |d| d.len()) + self.params.iter().map(|p| p.len()).sum::<usize>() + inlines
        };
        size_of::<Code>()
            + self.consts.capacity() * size_of::<Value>()
            + self.ops.own_bytes()
            + self.captures.own_bytes()
            + self.spans.own_bytes()
            + self.params.own_bytes()
            + self.uses.len() * size_of::<u32>()
            + if fresh { strings() } else { 0 }
    }

    /// Logical locations, innermost first. A failed call still belongs to
    /// its caller; a suspended physical caller has already entered its callee.
    pub(crate) fn trace_locations(&self, pc: usize, called: bool, mut emit: impl FnMut(&str, u32, u32)) {
        let pos = self.spans.get(pc).copied().unwrap_or(crate::reader::NO_POS);
        let site = self.inlines.as_ref().and_then(|map| {
            let index = map.sites.partition_point(|site| site.end as usize <= pc);
            map.sites.get(index).filter(|site| site.start as usize <= pc).map(|site| (map, site))
        });
        let Some((map, site)) = site else {
            emit(&self.name, self.file, pos);
            return;
        };
        let first = &map.scopes[site.scope as usize];
        if !(called && site.tail_call) {
            emit(&first.name, self.file, pos);
        }
        let mut current = Some(site.scope);
        let mut remaining = 32;
        while let Some(id) = current {
            let scope = &map.scopes[id as usize];
            if !scope.tail {
                let name = scope.parent.map_or(&*self.name, |p| &*map.scopes[p as usize].name);
                emit(name, self.file, scope.call_pos);
                remaining -= 1;
                if remaining == 0 {
                    break;
                }
            }
            current = scope.parent;
        }
    }

    /// Share its immutable parts from now on: it is in a template.
    pub(crate) fn share(&mut self) {
        // A patched buffer is the template's own: share the original.
        if let Some(ops) = self.jit.ops.get() {
            self.ops = ops.to_vec().into();
        }
        self.ops.share();
        self.captures.share();
        self.spans.share();
        self.params.share();
    }

    /// A copy for a world started from the template this code is in
    /// (`Vm::instantiate`, after `Code::share`): its immutable parts
    /// shared, no compiled state. Values are the template's until the
    /// caller replaces them.
    pub(crate) fn instance_copy(&self) -> Code {
        Code {
            handle: std::cell::Cell::new(self.handle.get()),
            name: self.name.clone(),
            ops: self.ops.clone(),
            consts: self.consts.clone(),
            nparams: self.nparams,
            rest: self.rest,
            frame_size: self.frame_size,
            captures: self.captures.clone(),
            file: self.file,
            spans: self.spans.clone(),
            inlines: self.inlines.clone(),
            pos: self.pos,
            params: self.params.clone(),
            doc: self.doc.clone(),
            jit: Default::default(),
            generation: self.generation,
            definition: self.definition,
            inline: self.inline.clone(),
            uses: self.uses.clone(),
            charged: None,
        }
    }
}

/// Parts of a code that do not change once it is installed (but for
/// compiled code's patches): owned while the code is its world's alone,
/// held through an `Rc` once codes share them (`Shared::share`). Reading
/// them goes through one slice pointer, whoever holds them: the
/// interpreter reloads a code's instructions on every call and return.
pub struct Shared<T> {
    /// The elements: a buffer of its own (`Owner::Own`), or the `Rc`'s.
    elems: NonNull<[T]>,
    owner: Owner<T>,
}

enum Owner<T> {
    /// `elems` is a boxed slice, leaked to this `Shared` alone, which
    /// reaches it through `elems` only and frees it when dropped.
    Own,
    Rc(Rc<[T]>),
}

impl<T> Shared<T> {
    fn rc(rc: Rc<[T]>) -> Self {
        Shared { elems: NonNull::from(&*rc), owner: Owner::Rc(rc) }
    }

    /// The bytes of its buffer, if it is its own.
    fn own_bytes(&self) -> usize {
        match self.owner {
            Owner::Own => self.len() * size_of::<T>(),
            Owner::Rc(_) => 0,
        }
    }
}

impl<T> Drop for Shared<T> {
    fn drop(&mut self) {
        if let Owner::Own = self.owner {
            // SAFETY: a buffer of its own, from `Box::leak`, reached by
            // nothing else.
            drop(unsafe { Box::from_raw(self.elems.as_ptr()) });
        }
    }
}

impl<T> std::ops::Deref for Shared<T> {
    type Target = [T];
    #[inline(always)]
    fn deref(&self) -> &[T] {
        // SAFETY: `elems` lives, and stays put, as long as `owner` does:
        // its own buffer until it is dropped, or the `Rc`'s.
        unsafe { self.elems.as_ref() }
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for Shared<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (**self).fmt(f)
    }
}

impl<T> From<Vec<T>> for Shared<T> {
    fn from(v: Vec<T>) -> Self {
        Shared { elems: NonNull::from(Box::leak(v.into_boxed_slice())), owner: Owner::Own }
    }
}

impl<T: Clone> Clone for Shared<T> {
    fn clone(&self) -> Self {
        match &self.owner {
            Owner::Own => self.to_vec().into(),
            Owner::Rc(rc) => Shared::rc(rc.clone()),
        }
    }
}

impl<T: Clone> Shared<T> {
    /// Its own elements, copied out first if they are shared.
    pub fn make_mut(&mut self) -> &mut [T] {
        if let Owner::Rc(rc) = &self.owner {
            *self = rc.to_vec().into();
        }
        // SAFETY: its own buffer now, reached through nothing but this
        // `Shared`, borrowed mutably for the result's lifetime.
        unsafe { self.elems.as_mut() }
    }

    /// Hold the elements through an `Rc` from now on: clones share them.
    pub fn share(&mut self) {
        if let Owner::Own = self.owner {
            // Copied: an allocation failing (a panic) leaves it as it was.
            *self = Shared::rc(Rc::from(&**self));
        }
    }
}

impl Op {
    /// The global the instruction reads, writes or calls.
    pub fn global(&self) -> Option<u32> {
        match *self {
            Op::GetG { g, .. } | Op::SetG { g, .. } | Op::CallG { g, .. } | Op::TailCallG { g, .. } => Some(g),
            _ => None,
        }
    }
}

#[cfg(test)]
#[test]
fn op_size() {
    assert!(std::mem::size_of::<Op>() <= 12, "keep instructions small");
}

#[cfg(test)]
mod tests {
    use super::Shared;

    #[test]
    fn shared_slices_copy_on_write() {
        let mut a: Shared<u32> = vec![1, 2, 3].into();
        a.make_mut()[0] = 10;
        a.share();
        let b = a.clone();
        assert_eq!((&*a, &*b), (&[10, 2, 3][..], &[10, 2, 3][..]));
        let mut c = b.clone();
        c.make_mut()[1] = 20;
        assert_eq!((&*a, &*b, &*c), (&[10, 2, 3][..], &[10, 2, 3][..], &[10, 20, 3][..]));
        drop(a);
        assert_eq!(&*b, &[10, 2, 3][..]);
        let d = c.clone();
        c.make_mut()[2] = 30;
        assert_eq!((&*c, &*d), (&[10, 20, 30][..], &[10, 20, 3][..]));
    }
}
