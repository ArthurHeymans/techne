//! Rust embedding API.
//!
//! * `Value`: a Scheme value held by Rust. Immediates (numbers that fit,
//!   characters, booleans, symbols, ...) are held as they are; a heap
//!   object is held through a root of its world, kept alive and up to date
//!   across collections while the `Value` lives. Every `Vm` method a value
//!   goes into checks it is of that world and the world is alive: a value
//!   of another world, or of one dropped, is an error, never a reference
//!   into a heap that is not this one's.
//! * `FromValue` / `IntoValue`: conversions used by `Vm::register_fn`, which
//!   exposes ordinary Rust closures as Scheme procedures. Arguments convert
//!   from an `Arg`, a view of the argument that nothing can collect or
//!   change while it is looked at.
//! * `Foreign<T>`: a Rust value owned by a Scheme object, dropped when the GC
//!   frees the object. Use `Foreign<RefCell<T>>` for mutable state.
//!
//! ```
//! use techne_vm::{api::Foreign, vm::Vm};
//! use std::cell::RefCell;
//!
//! let mut vm = Vm::new();
//! vm.register_fn("add", |a: i64, b: i64| a + b);
//! vm.register_fn("make-counter", || Foreign::new(RefCell::new(0i64)));
//! vm.register_fn("bump!", |c: Foreign<RefCell<i64>>| { *c.borrow_mut() += 1; *c.borrow() });
//! let v = vm.eval_source("(define c (make-counter)) (bump! c) (add (bump! c) 40)").unwrap();
//! assert_eq!(v.as_int(), Some(42));
//! ```

use std::{
    any::Any,
    cell::Cell,
    fmt::Display,
    marker::PhantomData,
    ops::Deref,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{
    builtins::{list_values, repr, type_error},
    heap::{Kind, is_kind, str_bytes},
    num,
    value::Value as RawValue,
    vm::{BoxedNative, Error, Native, NativeDoc, NativeImpl, Vm},
};

/// A world's identity, never reused. `DEAD` is none: the world of a root
/// whose world was dropped.
pub(crate) type WorldId = u64;
pub(crate) const DEAD: WorldId = 0;

pub(crate) fn new_world_id() -> WorldId {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Whether `v` means something only in its world: a heap object, or a
/// native (an index into the world's natives).
fn of_world(v: RawValue) -> bool {
    v.is_ptr() || v.is_native()
}

/// A rooted slot of a world: the world traces it and updates it as
/// objects move, while anything holds it.
pub(crate) struct RootCell {
    value: Cell<RawValue>,
    world: Cell<WorldId>,
}

impl RootCell {
    pub(crate) fn get(&self) -> RawValue {
        self.value.get()
    }
    pub(crate) fn set(&self, v: RawValue) {
        self.value.set(v)
    }
    /// Its world is gone: it holds nothing any more.
    pub(crate) fn kill(&self) {
        self.value.set(RawValue::VOID);
        self.world.set(DEAD);
    }
}

/// A GC root for a raw value, inside the VM. Cloning shares the slot.
#[derive(Clone)]
pub(crate) struct Root(Rc<RootCell>);

impl Root {
    pub(crate) fn new(v: RawValue, world: WorldId) -> Root {
        Root(Rc::new(RootCell { value: Cell::new(v), world: Cell::new(world) }))
    }
    pub(crate) fn cell(&self) -> &Rc<RootCell> {
        &self.0
    }
    /// The current value (valid until the next allocation).
    pub(crate) fn get(&self) -> RawValue {
        self.0.get()
    }
    pub(crate) fn set(&self, v: RawValue) {
        self.0.set(v)
    }
    pub(crate) fn world(&self) -> WorldId {
        self.0.world.get()
    }
}

impl std::fmt::Debug for Root {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let v = self.get();
        if v.is_ptr() { write!(f, "Root(#<object>)") } else { write!(f, "Root({})", repr(v)) }
    }
}

/// A Rust value owned by a Scheme object.
pub struct Foreign<T: 'static>(pub Rc<T>);

impl<T: 'static> Foreign<T> {
    pub fn new(value: T) -> Self {
        Foreign(Rc::new(value))
    }
}

impl<T: 'static> Clone for Foreign<T> {
    fn clone(&self) -> Self {
        Foreign(self.0.clone())
    }
}

impl<T: 'static> Deref for Foreign<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

/// A Scheme value held by Rust: an immediate as it is; a heap object, or
/// a native procedure (an index into its world's table), through a root of
/// its world (see the module). Not `Send`: it belongs to its world's
/// thread.
#[derive(Clone)]
pub struct Value(Repr);

#[derive(Clone)]
enum Repr {
    Imm(RawValue),
    Heap(Root),
    /// A symbol (or, `true`, a keyword), by name: a symbol's id is freed
    /// with the last world using it, so the id is the one of the world it
    /// goes into.
    Sym(Rc<str>, bool),
}

impl Value {
    pub const NIL: Value = Value(Repr::Imm(RawValue::NIL));
    pub const FALSE: Value = Value(Repr::Imm(RawValue::FALSE));
    pub const TRUE: Value = Value(Repr::Imm(RawValue::TRUE));
    pub const VOID: Value = Value(Repr::Imm(RawValue::VOID));
    pub const EOF: Value = Value(Repr::Imm(RawValue::EOF));

    pub fn bool(b: bool) -> Value {
        Value(Repr::Imm(RawValue::bool(b)))
    }
    /// A fixnum: `i` if it fits 48 bits.
    pub fn fixnum(i: i64) -> Option<Value> {
        RawValue::fixnum(i).map(|v| Value(Repr::Imm(v)))
    }
    pub fn float(f: f64) -> Value {
        Value(Repr::Imm(RawValue::float(f)))
    }
    pub fn char(c: char) -> Value {
        Value(Repr::Imm(RawValue::char(c)))
    }
    /// The symbol named `name`, in whatever world it goes into.
    pub fn symbol(name: &str) -> Value {
        Value(Repr::Sym(name.into(), false))
    }

    /// The value `root` holds, as the host holds it.
    pub(crate) fn from_root(root: Root) -> Value {
        match root.get() {
            v if of_world(v) || root.world() == DEAD => Value(Repr::Heap(root)),
            v => Value::held(v),
        }
    }

    /// Immediate `v` as the host holds it: a symbol by name.
    fn held(v: RawValue) -> Value {
        if v.is_symbol() {
            Value(Repr::Sym(crate::reader::symbol_name(v.as_symbol()), false))
        } else if v.is_keyword() {
            Value(Repr::Sym(crate::reader::symbol_name(v.as_keyword()), true))
        } else {
            Value(Repr::Imm(v))
        }
    }

    fn imm(&self) -> Option<RawValue> {
        match self.0 {
            Repr::Imm(v) => Some(v),
            Repr::Heap(_) | Repr::Sym(..) => None,
        }
    }
    pub fn is_false(&self) -> bool {
        self.imm().is_some_and(RawValue::is_false)
    }
    pub fn is_truthy(&self) -> bool {
        !self.is_false()
    }
    pub fn is_nil(&self) -> bool {
        self.imm() == Some(RawValue::NIL)
    }
    pub fn is_void(&self) -> bool {
        self.imm() == Some(RawValue::VOID)
    }
    pub fn is_eof(&self) -> bool {
        self.imm() == Some(RawValue::EOF)
    }
    /// A fixnum's value (a big integer is a heap object: `Vm::get`).
    pub fn as_int(&self) -> Option<i64> {
        self.imm().filter(|v| v.is_int()).map(RawValue::as_int)
    }
    pub fn as_float(&self) -> Option<f64> {
        self.imm().filter(|v| v.is_float()).map(RawValue::as_float)
    }
    pub fn as_char(&self) -> Option<char> {
        self.imm().filter(|v| v.is_char()).map(RawValue::as_char)
    }
    pub fn is_symbol(&self) -> bool {
        matches!(self.0, Repr::Sym(_, false))
    }
    /// A symbol's name.
    pub fn symbol_name(&self) -> Option<Rc<str>> {
        match &self.0 {
            Repr::Sym(name, false) => Some(name.clone()),
            _ => None,
        }
    }
    /// A heap object (string, pair, vector, procedure, ...) or a native
    /// procedure: a value of one world, rather than an immediate.
    pub fn is_object(&self) -> bool {
        matches!(self.0, Repr::Heap(_))
    }
}

/// A fixnum: every `i32` fits.
impl From<i32> for Value {
    fn from(i: i32) -> Value {
        Value(Repr::Imm(RawValue::int_unchecked(i.into())))
    }
}

/// Values are the same object (`eq?`), or the same immediate.
impl PartialEq for Value {
    fn eq(&self, other: &Value) -> bool {
        match (&self.0, &other.0) {
            (Repr::Imm(a), Repr::Imm(b)) => a == b,
            (Repr::Heap(a), Repr::Heap(b)) => a.world() == b.world() && a.get() == b.get(),
            (Repr::Sym(a, ak), Repr::Sym(b, bk)) => a == b && ak == bk,
            _ => false,
        }
    }
}

impl std::fmt::Debug for Value {
    /// An immediate as Scheme writes it; a heap object without looking at
    /// it (`Vm::repr` prints it).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Repr::Imm(v) => write!(f, "{}", repr(*v)),
            Repr::Heap(r) if r.world() == DEAD => write!(f, "#<object of a dropped world>"),
            Repr::Heap(_) => write!(f, "#<object>"),
            Repr::Sym(name, false) => write!(f, "{}", crate::reader::symbol_repr(name)),
            Repr::Sym(name, true) => write!(f, "#:{name}"),
        }
    }
}

impl Vm {
    /// `v` held by Rust: a heap object rooted.
    pub(crate) fn export(&mut self, v: RawValue) -> Value {
        if of_world(v) { Value(Repr::Heap(self.root(v))) } else { Value::held(v) }
    }

    /// The raw value `v` holds, if it is of this world.
    pub(crate) fn import_value(&self, v: &Value) -> Result<RawValue, Error> {
        match &v.0 {
            Repr::Imm(r) => Ok(*r),
            Repr::Heap(root) => self.import_root(root),
            Repr::Sym(name, keyword) => {
                // Interned as this world's, charged to it.
                let _charged = self.charging();
                let id = crate::reader::intern(name);
                Ok(if *keyword { RawValue::keyword(id) } else { RawValue::symbol(id) })
            }
        }
    }

    /// The raw value `root` holds, if it is of this world.
    pub(crate) fn import_root(&self, root: &Root) -> Result<RawValue, Error> {
        match root.world() {
            w if w == self.world_id() => Ok(root.get()),
            DEAD => Err(Error::new("a value of a dropped world")),
            _ => Err(Error::new("a value of another world")),
        }
    }

    /// The raw values `values` hold, each of this world.
    pub(crate) fn import_all(&self, values: &[Value]) -> Result<Vec<RawValue>, Error> {
        values.iter().map(|v| self.import_value(v)).collect()
    }

    /// Decode with `f` what nothing collects or changes while it looks.
    /// The values it keeps are rooted as it makes them (`Arg::value`); an
    /// error it returns (a conversion's own, perhaps of another world)
    /// keeps of another world only its message.
    pub(crate) fn decode<T>(&self, f: impl FnOnce(&Decode<'_>) -> Result<T, Error>) -> Result<T, Error> {
        let _entered = self.charging();
        f(&Decode { vm: self }).map_err(|e| e.adopt(self.world_id()))
    }

    /// `v` as Scheme writes it, if it is of this world.
    pub fn repr(&self, v: &Value) -> String {
        self.host_ref(|vm| vm.import_value(v).map(repr)).and_then(|r| r).unwrap_or_else(|e| format!("#<{}>", e.msg))
    }

    /// `v` written briefly: at most a line, long contents cut short.
    pub fn brief(&self, v: &Value) -> String {
        self.host_ref(|vm| vm.import_value(v).map(crate::builtins::brief)).and_then(|r| r).unwrap_or_else(|e| format!("#<{}>", e.msg))
    }
}

/// What decoding natives' arguments looks at: the VM, shared, so that
/// nothing runs, collects or changes a value meanwhile.
pub(crate) struct Decode<'a> {
    vm: &'a Vm,
}

/// An argument of a native, or a part of one, to convert to Rust
/// (`FromValue`). What it gives cannot outlive the conversion except as
/// owned Rust data or a `Value`.
#[derive(Clone, Copy)]
pub struct Arg<'a> {
    v: RawValue,
    cx: &'a Decode<'a>,
}

impl<'a> Arg<'a> {
    pub(crate) fn new(cx: &'a Decode<'a>, v: RawValue) -> Arg<'a> {
        Arg { v, cx }
    }
    pub fn is_false(&self) -> bool {
        self.v.is_false()
    }
    pub fn is_truthy(&self) -> bool {
        self.v.is_truthy()
    }
    pub fn is_symbol(&self) -> bool {
        self.v.is_symbol()
    }
    /// A fixnum's value.
    pub fn as_int(&self) -> Option<i64> {
        self.v.is_int().then(|| self.v.as_int())
    }
    /// An exact integer that fits `i64`.
    pub fn integer(&self) -> Result<i64, Error> {
        // Decoded in its world, whichever the host has entered.
        let _entered = self.cx.vm.cage_entry();
        if self.v.is_int() { Ok(self.v.as_int()) } else { num::integer(self.v, "integer argument") }
    }
    /// A real number.
    pub fn real(&self) -> Result<f64, Error> {
        // Decoded in its world, whichever the host has entered.
        let _entered = self.cx.vm.cage_entry();
        Ok(num::real(self.v, "number argument")?.f())
    }
    pub fn as_char(&self) -> Option<char> {
        self.v.is_char().then(|| self.v.as_char())
    }
    /// A string's text.
    pub fn as_str(&self) -> Option<&'a str> {
        // Decoded in its world, whichever the host has entered.
        let _entered = self.cx.vm.cage_entry();
        // SAFETY: a string of this world (an argument the VM holds, or a
        // part of one), which nothing changes while the VM is shared.
        is_kind(self.v, Kind::String).then(|| std::str::from_utf8(unsafe { str_bytes(self.v.as_ptr()) }).ok()).flatten()
    }
    /// A bytevector's bytes, or a string's.
    pub fn as_bytes(&self) -> Option<&'a [u8]> {
        // Decoded in its world, whichever the host has entered.
        let _entered = self.cx.vm.cage_entry();
        // SAFETY: as for `as_str`.
        (is_kind(self.v, Kind::Bytevector) || is_kind(self.v, Kind::String)).then(|| unsafe { str_bytes(self.v.as_ptr()) })
    }
    /// A symbol's name.
    pub fn symbol_name(&self) -> Option<Rc<str>> {
        self.v.is_symbol().then(|| crate::reader::symbol_name(self.v.as_symbol()))
    }
    /// The elements of a proper list or of a vector.
    pub fn items(&self) -> Option<Vec<Arg<'a>>> {
        // Decoded in its world, whichever the host has entered.
        let _entered = self.cx.vm.cage_entry();
        let items = if is_kind(self.v, Kind::Vector) {
            // SAFETY: a vector of this world.
            (0..unsafe { crate::heap::len_of(self.v.as_ptr()) }).map(|i| unsafe { crate::heap::field(self.v.as_ptr(), i) }).collect()
        } else {
            list_values(self.v)?
        };
        Some(items.into_iter().map(|v| Arg::new(self.cx, v)).collect())
    }
    /// The kind of heap object it is; none for an immediate.
    pub fn kind(&self) -> Option<Kind> {
        // Decoded in its world, whichever the host has entered.
        let _entered = self.cx.vm.cage_entry();
        // SAFETY: a heap object of this world.
        self.v.is_ptr().then(|| Kind::of(unsafe { crate::heap::kind_of(self.v.as_ptr()) })).flatten()
    }
    /// The parts of a pair (car, cdr), vector, box, record (its type, then
    /// its fields), record type (name, field names), ratio, complex number,
    /// or closure (what it captured); none for other values.
    pub fn fields(&self) -> Vec<Arg<'a>> {
        // Decoded in its world, whichever the host has entered.
        let _entered = self.cx.vm.cage_entry();
        let skip = match self.kind() {
            Some(Kind::Closure) => 1,
            Some(Kind::Pair | Kind::Vector | Kind::Box | Kind::Record | Kind::Rtd | Kind::Ratio | Kind::Complex) => 0,
            _ => return Vec::new(),
        };
        let obj = self.v.as_ptr();
        // SAFETY: a traced object of this world, its fields after its header.
        (skip..unsafe { crate::heap::len_of(obj) }).map(|i| Arg::new(self.cx, unsafe { crate::heap::field(obj, i) })).collect()
    }
    /// A dict's keys and values, alternating, or a set's elements.
    pub fn dict_entries(&self) -> Vec<Arg<'a>> {
        // Decoded in its world, whichever the host has entered.
        let _entered = self.cx.vm.cage_entry();
        match self.kind() {
            Some(Kind::Dict | Kind::Set) => crate::dict::entries(self.v).into_iter().map(|v| Arg::new(self.cx, v)).collect(),
            _ => Vec::new(),
        }
    }
    /// The Rust value of a foreign object of type `T`.
    pub fn foreign<T: 'static>(&self) -> Option<Rc<T>> {
        // Decoded in its world, whichever the host has entered.
        let _entered = self.cx.vm.cage_entry();
        self.cx.vm.foreign_raw(self.v).and_then(|(rc, _)| rc.clone().downcast::<T>().ok())
    }
    /// The argument, kept: a `Value`, rooted.
    pub fn value(&self) -> Value {
        // Decoded in its world, whichever the host has entered.
        let _entered = self.cx.vm.cage_entry();
        if !of_world(self.v) {
            return Value::held(self.v);
        }
        let vm = self.cx.vm;
        let _charged = vm.charging();
        let root = Root::new(self.v, vm.world_id());
        vm.roots.borrow_mut().push(Rc::downgrade(root.cell()));
        Value(Repr::Heap(root))
    }
    /// The argument written briefly, for messages.
    pub fn brief(&self) -> String {
        // Decoded in its world, whichever the host has entered.
        let _entered = self.cx.vm.cage_entry();
        crate::builtins::brief(self.v)
    }
    /// The error for an argument that is not `expected`.
    pub fn type_error(&self, expected: &str) -> Error {
        // Decoded in its world, whichever the host has entered.
        let _entered = self.cx.vm.cage_entry();
        type_error("argument", expected, self.v)
    }
    /// The error of `who` for an argument that is not `expected`.
    pub fn type_error_for(&self, who: &str, expected: &str) -> Error {
        // Decoded in its world, whichever the host has entered.
        let _entered = self.cx.vm.cage_entry();
        type_error(who, expected, self.v)
    }
}

/// The arguments of an async native (`Vm::register_async`) while it makes
/// its future: in the VM's registers, where collections keep them current,
/// so reading one needs no root. They cannot go into the future (keep a
/// `Value`, `Args::value`, for that).
#[derive(Clone, Copy)]
pub struct Args<'a> {
    base: usize,
    n: usize,
    world: WorldId,
    task: Option<usize>,
    _call: PhantomData<&'a ()>,
}

impl Args<'_> {
    pub(crate) fn new(vm: &Vm, base: usize, n: usize) -> Self {
        Args { base, n, world: vm.world_id(), task: vm.current_task, _call: PhantomData }
    }
    pub fn len(&self) -> usize {
        self.n
    }
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }
    /// The arguments from the `from`th on.
    pub fn rest(&self, from: usize) -> Self {
        let from = from.min(self.n);
        Args { base: self.base + from, n: self.n - from, ..*self }
    }
    /// Argument `i`, from its register: one the collector traces, of this
    /// world and of the call that made these arguments.
    fn raw(&self, vm: &Vm, i: usize) -> Result<RawValue, Error> {
        if i >= self.n {
            return Err(Error::new(format!("no argument {i} of {}", self.n)));
        }
        if vm.world_id() != self.world || vm.current_task != self.task || self.base + i >= vm.stack_top {
            return Err(Error::new("arguments of another call"));
        }
        Ok(vm.regs[self.base + i])
    }
    /// Argument `i`, converted to Rust.
    pub fn get<T: FromValue>(&self, vm: &mut Vm, i: usize) -> Result<T, Error> {
        let v = self.raw(vm, i)?;
        vm.decode(|cx| T::from_arg(Arg::new(cx, v)))
    }
    /// Look at argument `i` in place with `f` (`Vm::inspect`).
    pub fn inspect<R>(&self, vm: &mut Vm, i: usize, f: impl FnOnce(Arg<'_>) -> R) -> Result<R, Error> {
        let v = self.raw(vm, i)?;
        vm.decode(|cx| Ok(f(Arg::new(cx, v))))
    }
    /// Argument `i`, kept: a `Value`, which the future may hold.
    pub fn value(&self, vm: &mut Vm, i: usize) -> Result<Value, Error> {
        let v = self.raw(vm, i)?;
        Ok(vm.export(v))
    }
}

/// A native's result, raw: of the world it was made in, and consumed
/// before that world's VM is used again (it borrows it). Only the VM's
/// own conversions make one.
#[doc(hidden)]
pub struct Raw<'v> {
    v: RawValue,
    world: WorldId,
    _vm: PhantomData<&'v mut Vm>,
}

impl Raw<'_> {
    pub(crate) fn new<'v>(vm: &Vm, v: RawValue) -> Raw<'v> {
        Raw { v, world: vm.world_id(), _vm: PhantomData }
    }
}

/// The raw value `r` holds, checked to be of `world` (read before `r` was
/// made: it borrows the VM).
pub(crate) fn accept(world: WorldId, r: Raw<'_>) -> Result<RawValue, Error> {
    if of_world(r.v) && r.world != world {
        return Err(Error::new("a conversion gave a value of another world"));
    }
    Ok(r.v)
}

pub trait FromValue: Sized {
    /// Convert `arg`. Nothing collects or runs while it does: what it
    /// keeps of the argument is owned Rust data, or `Arg::value`.
    fn from_arg(arg: Arg<'_>) -> Result<Self, Error>;
}

/// Conversion to Scheme. Implement `into_value`; the VM's own conversions
/// also give the raw value directly (`into_raw`), without a root.
pub trait IntoValue: Sized {
    fn into_value(self, vm: &mut Vm) -> Result<Value, Error>;

    #[doc(hidden)]
    fn into_raw(self, vm: &mut Vm) -> Result<Raw<'_>, Error> {
        let v = self.into_value(vm)?;
        let raw = vm.import_value(&v)?;
        Ok(Raw::new(vm, raw))
    }
}

/// `IntoValue` for a type with a raw conversion: `into_value` through it.
macro_rules! into_value_via_raw {
    () => {
        fn into_value(self, vm: &mut Vm) -> Result<Value, Error> {
            // Made in the world's cage, whichever world the host is in.
            let _entered = vm.charging();
            let world = vm.world_id();
            let raw = self.into_raw(vm)?;
            let v = accept(world, raw)?;
            Ok(vm.export(v))
        }
    };
}

impl FromValue for Value {
    fn from_arg(arg: Arg<'_>) -> Result<Self, Error> {
        Ok(arg.value())
    }
}
impl FromValue for i64 {
    fn from_arg(arg: Arg<'_>) -> Result<Self, Error> {
        arg.integer()
    }
}
impl FromValue for usize {
    fn from_arg(arg: Arg<'_>) -> Result<Self, Error> {
        usize::try_from(arg.integer()?).map_err(|_| arg.type_error("non-negative integer"))
    }
}
impl FromValue for f64 {
    fn from_arg(arg: Arg<'_>) -> Result<Self, Error> {
        arg.real()
    }
}
impl FromValue for bool {
    /// Scheme truthiness: everything but `#f` is true.
    fn from_arg(arg: Arg<'_>) -> Result<Self, Error> {
        Ok(arg.is_truthy())
    }
}
impl FromValue for char {
    fn from_arg(arg: Arg<'_>) -> Result<Self, Error> {
        arg.as_char().ok_or_else(|| arg.type_error("char"))
    }
}
impl FromValue for String {
    /// A string's text, or a symbol's name.
    fn from_arg(arg: Arg<'_>) -> Result<Self, Error> {
        if let Some(s) = arg.as_str() {
            Ok(s.to_owned())
        } else if let Some(name) = arg.symbol_name() {
            Ok(name.to_string())
        } else {
            Err(arg.type_error("string"))
        }
    }
}
/// Bytes as a bytevector. As an argument, a string gives its UTF-8 bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bytes(pub Vec<u8>);

impl FromValue for Bytes {
    fn from_arg(arg: Arg<'_>) -> Result<Self, Error> {
        arg.as_bytes().map(|b| Bytes(b.to_vec())).ok_or_else(|| arg.type_error("bytevector or string"))
    }
}
impl IntoValue for Bytes {
    into_value_via_raw!();
    fn into_raw(self, vm: &mut Vm) -> Result<Raw<'_>, Error> {
        let v = vm.make_bytevector_raw(&self.0);
        Ok(Raw::new(vm, v))
    }
}

impl<T: FromValue> FromValue for Vec<T> {
    /// From a list or a vector.
    fn from_arg(arg: Arg<'_>) -> Result<Self, Error> {
        arg.items().ok_or_else(|| arg.type_error("list or vector"))?.into_iter().map(T::from_arg).collect()
    }
}
impl<T: FromValue> FromValue for Option<T> {
    /// `#f` is `None`.
    fn from_arg(arg: Arg<'_>) -> Result<Self, Error> {
        if arg.is_false() { Ok(None) } else { T::from_arg(arg).map(Some) }
    }
}
impl<T: 'static> FromValue for Foreign<T> {
    fn from_arg(arg: Arg<'_>) -> Result<Self, Error> {
        arg.foreign::<T>().map(Foreign).ok_or_else(|| arg.type_error(std::any::type_name::<T>()))
    }
}

impl IntoValue for Value {
    fn into_value(self, _: &mut Vm) -> Result<Value, Error> {
        Ok(self)
    }
}
impl IntoValue for &Value {
    fn into_value(self, _: &mut Vm) -> Result<Value, Error> {
        Ok(self.clone())
    }
}

/// Immediates and the values the VM makes from Rust ones, each through
/// `$make` (the VM, the Rust value) to a raw value.
macro_rules! into_value_raw {
    ($t:ty, |$vm:ident, $x:ident| $make:expr) => {
        impl IntoValue for $t {
            into_value_via_raw!();
            fn into_raw(self, $vm: &mut Vm) -> Result<Raw<'_>, Error> {
                let $x = self;
                let v = $make;
                Ok(Raw::new($vm, v))
            }
        }
    };
}

into_value_raw!((), |vm, _x| RawValue::VOID);
into_value_raw!(bool, |vm, x| RawValue::bool(x));
into_value_raw!(i64, |vm, x| vm.make_int_raw(x));
into_value_raw!(i32, |vm, x| vm.make_int_raw(i64::from(x)));
into_value_raw!(usize, |vm, x| match i64::try_from(x) {
    Ok(i) => vm.make_int_raw(i),
    Err(_) => num::make_integer(vm, &x.into()),
});
into_value_raw!(f64, |vm, x| RawValue::float(x));
into_value_raw!(char, |vm, x| RawValue::char(x));
into_value_raw!(String, |vm, x| vm.make_string_raw(&x));
into_value_raw!(&str, |vm, x| vm.make_string_raw(x));

impl<T: IntoValue> IntoValue for Vec<T> {
    into_value_via_raw!();
    /// As a list. Converted elements are rooted until the list is built.
    fn into_raw(self, vm: &mut Vm) -> Result<Raw<'_>, Error> {
        let mark = vm.scratch.len();
        let world = vm.world_id();
        let converted = self.into_iter().try_for_each(|x| {
            let raw = x.into_raw(vm)?;
            let v = accept(world, raw)?;
            vm.scratch.push(v);
            Ok(())
        });
        // Unroot the elements whether or not they all converted.
        let items: Vec<RawValue> = vm.scratch.drain(mark..).collect();
        converted?;
        let v = vm.make_list_raw(&items);
        Ok(Raw::new(vm, v))
    }
}
impl<T: IntoValue> IntoValue for Option<T> {
    into_value_via_raw!();
    /// `None` is `#f`.
    fn into_raw(self, vm: &mut Vm) -> Result<Raw<'_>, Error> {
        match self {
            Some(x) => x.into_raw(vm),
            None => Ok(Raw::new(vm, RawValue::FALSE)),
        }
    }
}
impl<T: 'static> IntoValue for Foreign<T> {
    into_value_via_raw!();
    fn into_raw(self, vm: &mut Vm) -> Result<Raw<'_>, Error> {
        let rc: Rc<dyn Any> = self.0;
        let v = vm.make_foreign_raw(rc, std::any::type_name::<T>());
        Ok(Raw::new(vm, v))
    }
}
impl<T: IntoValue, E: Display + 'static> IntoValue for Result<T, E> {
    into_value_via_raw!();
    /// `Err` raises a Scheme error with the error's message. A `vm::Error`
    /// passes through unchanged (keeping raised objects and escapes).
    fn into_raw(self, vm: &mut Vm) -> Result<Raw<'_>, Error> {
        match self {
            Ok(x) => x.into_raw(vm),
            Err(e) => Err(into_error(e)),
        }
    }
}

fn into_error<E: Display + 'static>(e: E) -> Error {
    let boxed: Box<dyn Any> = Box::new(e);
    match boxed.downcast::<Error>() {
        Ok(err) => *err,
        Err(other) => {
            let e = other.downcast::<E>().expect("same type");
            Error::new(e.to_string())
        }
    }
}

/// A typed closure takes exactly the parameters its signature names.
fn check_arity(name: &str, doc: &NativeDoc, arity: usize) {
    assert!(
        doc.params.len() == arity && doc.params.iter().all(|p| !p.starts_with(['[', '.'])),
        "{name}: the signature names {:?}, the closure takes {arity} arguments",
        doc.params
    );
}

/// How the VM calls typed closures. Crate-private, and so the public
/// traits bounded by them are sealed: only the VM calls a native, and
/// only the closures below are ones.
pub(crate) trait Typed<Args> {
    const ARITY: usize;
    fn into_native(self) -> BoxedNative;
    /// Call it with the arguments from register `args` on.
    fn invoke(&self, vm: &mut Vm, args: usize) -> Result<RawValue, Error>;
}

/// `Typed` for closures that also receive the VM.
pub(crate) trait TypedVm<Args> {
    const ARITY: usize;
    fn into_native(self) -> BoxedNative;
    fn invoke(&self, vm: &mut Vm, args: usize) -> Result<RawValue, Error>;
}

/// Rust closures callable from Scheme: `Fn(A, B, ...) -> R`.
#[allow(private_bounds)]
pub trait IntoNative<Args>: Typed<Args> {}
impl<Args, F: Typed<Args>> IntoNative<Args> for F {}

/// Rust closures that also receive the VM: `Fn(&mut Vm, A, B, ...) -> R`.
#[allow(private_bounds)]
pub trait IntoNativeVm<Args>: TypedVm<Args> {}
impl<Args, F: TypedVm<Args>> IntoNativeVm<Args> for F {}

macro_rules! impl_into_native {
    ($($a:ident),*) => {
        impl<F, Ret, $($a),*> Typed<($($a,)*)> for F
        where
            F: Fn($($a),*) -> Ret + 'static,
            Ret: IntoValue,
            $($a: FromValue,)*
        {
            const ARITY: usize = impl_into_native!(@count $($a)*);
            fn into_native(self) -> BoxedNative {
                Rc::new(move |vm: &mut Vm, args: usize, _n: usize| self.invoke(vm, args))
            }
            #[allow(non_snake_case, unused_mut, unused_variables, unused_assignments)]
            #[inline(always)]
            fn invoke(&self, vm: &mut Vm, args: usize) -> Result<RawValue, Error> {
                let ($($a,)*) = vm.decode(|cx| {
                    let mut i = 0;
                    Ok(($({
                        let a = <$a as FromValue>::from_arg(Arg::new(cx, cx.vm.regs[args + i]))?;
                        i += 1;
                        a
                    },)*))
                })?;
                let world = vm.world_id();
                let raw = (self)($($a),*).into_raw(vm).map_err(|e| e.adopt(world))?;
                accept(world, raw)
            }
        }
    };
    (@count) => { 0 };
    (@count $x:ident $($rest:ident)*) => { 1 + impl_into_native!(@count $($rest)*) };
}

macro_rules! impl_into_native_vm {
    ($($a:ident),*) => {
        impl<F, Ret, $($a),*> TypedVm<($($a,)*)> for F
        where
            F: Fn(&mut Vm, $($a),*) -> Ret + 'static,
            Ret: IntoValue,
            $($a: FromValue,)*
        {
            const ARITY: usize = impl_into_native!(@count $($a)*);
            fn into_native(self) -> BoxedNative {
                Rc::new(move |vm: &mut Vm, args: usize, _n: usize| self.invoke(vm, args))
            }
            #[allow(non_snake_case, unused_mut, unused_variables, unused_assignments)]
            #[inline(always)]
            fn invoke(&self, vm: &mut Vm, args: usize) -> Result<RawValue, Error> {
                let ($($a,)*) = vm.decode(|cx| {
                    let mut i = 0;
                    Ok(($({
                        let a = <$a as FromValue>::from_arg(Arg::new(cx, cx.vm.regs[args + i]))?;
                        i += 1;
                        a
                    },)*))
                })?;
                let world = vm.world_id();
                let raw = (self)(vm, $($a),*).into_raw(vm).map_err(|e| e.adopt(world))?;
                accept(world, raw)
            }
        }
    };
}

impl_into_native_vm!();
impl_into_native_vm!(A);
impl_into_native_vm!(A, B);
impl_into_native_vm!(A, B, C);
impl_into_native_vm!(A, B, C, D);

impl_into_native!();
impl_into_native!(A);
impl_into_native!(A, B);
impl_into_native!(A, B, C);
impl_into_native!(A, B, C, D);
impl_into_native!(A, B, C, D, E);
impl_into_native!(A, B, C, D, E, G);

/// Whether any value of `F` is any other: `F` holds nothing (a closure
/// capturing nothing, a function), and dropping one does nothing. Then a
/// function generic in `F` can call one without being given it, provided
/// the type is inhabited (this size/drop check alone does not prove that).
fn stateless<F>() -> bool {
    std::mem::size_of::<F>() == 0 && !std::mem::needs_drop::<F>()
}

/// A stateless `F` (`stateless`), to call.
///
/// # Safety
/// `F` must be inhabited, zero-sized and non-dropping. Registration proves
/// inhabitation by receiving an actual `F`; `stateless` checks the rest.
unsafe fn conjure<'a, F>() -> &'a F {
    debug_assert!(stateless::<F>());
    // SAFETY: the caller proves a valid zero-sized F exists. The dangling
    // pointer is non-null and aligned, including for over-aligned captures.
    unsafe { std::ptr::NonNull::<F>::dangling().as_ref() }
}

fn stateless_native<Args, F: IntoNative<Args>>(vm: &mut Vm, args: usize, _n: usize) -> Result<RawValue, Error> {
    // SAFETY: only installed by typed, given an actual stateless F.
    unsafe { conjure::<F>() }.invoke(vm, args)
}

extern "C" fn stateless_plain<Args, F: IntoNative<Args>>(vm: &mut Vm, args: usize, n: usize) -> crate::jit::NativeRet {
    crate::jit::NativeRet::from_call(vm, |vm| stateless_native::<Args, F>(vm, args, n))
}

fn stateless_native_vm<Args, F: IntoNativeVm<Args>>(vm: &mut Vm, args: usize, _n: usize) -> Result<RawValue, Error> {
    // SAFETY: only installed by typed_vm, given an actual stateless F.
    unsafe { conjure::<F>() }.invoke(vm, args)
}

extern "C" fn stateless_plain_vm<Args, F: IntoNativeVm<Args>>(vm: &mut Vm, args: usize, n: usize) -> crate::jit::NativeRet {
    crate::jit::NativeRet::from_call(vm, |vm| stateless_native_vm::<Args, F>(vm, args, n))
}

/// How the VM calls typed closure `f`: if it holds nothing, through
/// functions made for its type, which compiled code calls directly
/// (`NativeImpl::Plain`, as `natives!` defines natives); else boxed.
fn typed<Args, F: IntoNative<Args>>(f: F) -> NativeImpl {
    if stateless::<F>() {
        std::mem::forget(f);
        NativeImpl::Plain(stateless_native::<Args, F>, stateless_plain::<Args, F>)
    } else {
        NativeImpl::Boxed(f.into_native())
    }
}

/// `typed` for closures that need the VM.
fn typed_vm<Args, F: IntoNativeVm<Args>>(f: F) -> NativeImpl {
    if stateless::<F>() {
        std::mem::forget(f);
        NativeImpl::Plain(stateless_native_vm::<Args, F>, stateless_plain_vm::<Args, F>)
    } else {
        NativeImpl::Boxed(f.into_native())
    }
}

impl Vm {
    /// Expose a Rust closure as a global Scheme procedure. Compiled code
    /// calls one that holds nothing directly, as it calls `natives!`
    /// natives.
    pub fn register_fn<Args, F: IntoNative<Args>>(&mut self, name: &str, f: F) {
        let arity = F::ARITY;
        self.define_native(Native { name: name.into(), f: typed(f), min: arity, max: Some(arity), doc: None });
    }

    /// Like `register_fn`, for closures that need the VM (to call back into
    /// Scheme, allocate, or inspect values).
    pub fn register_fn_vm<Args, F: IntoNativeVm<Args>>(&mut self, name: &str, f: F) {
        let arity = F::ARITY;
        self.define_native(Native { name: name.into(), f: typed_vm(f), min: arity, max: Some(arity), doc: None });
    }

    /// Like `register_fn`, with the parameters and documentation that
    /// `procedures!` takes from the definition.
    pub fn register_documented<Args, F: IntoNative<Args>>(&mut self, name: &str, doc: &'static NativeDoc, f: F) {
        check_arity(name, doc, F::ARITY);
        self.define_native(Native { name: name.into(), f: typed(f), min: F::ARITY, max: Some(F::ARITY), doc: Some(doc) });
    }

    /// Like `register_fn_vm`, with the parameters and documentation that
    /// `procedures!` takes from the definition.
    pub fn register_documented_vm<Args, F: IntoNativeVm<Args>>(&mut self, name: &str, doc: &'static NativeDoc, f: F) {
        check_arity(name, doc, F::ARITY);
        self.define_native(Native { name: name.into(), f: typed_vm(f), min: F::ARITY, max: Some(F::ARITY), doc: Some(doc) });
    }

    /// Expose a variadic Rust function, given its arguments as values.
    pub fn register_variadic(&mut self, name: &str, f: impl Fn(&mut Vm, &[Value]) -> Result<Value, Error> + 'static) {
        let f: BoxedNative = Rc::new(move |vm: &mut Vm, args: usize, n: usize| {
            let vals: Vec<Value> = (args..args + n).map(|i| vm.regs[i]).collect::<Vec<_>>().into_iter().map(|v| vm.export(v)).collect();
            let world = vm.world_id();
            let out = f(vm, &vals).map_err(|e| e.adopt(world))?;
            vm.import_value(&out)
        });
        self.define_native(Native { name: name.into(), f: NativeImpl::Boxed(f), min: 0, max: None, doc: None });
    }

    /// Name a foreign Rust type for Scheme: `type-of` returns the name and
    /// `define-method` can dispatch on it (`(define-method (show (b buffer)) ...)`).
    pub fn name_foreign_type<T: 'static>(&mut self, name: &str) {
        self.foreign_type_names.insert(std::any::type_name::<T>(), crate::reader::intern(name));
    }

    /// Call a global procedure by name.
    pub fn call_global(&mut self, name: &str, args: &[Value]) -> Result<Value, Error> {
        let f = self.get_global(name).ok_or_else(|| Error::new(format!("unbound procedure {name}")))?;
        self.call(&f, args)
    }

    /// `get` for a raw value of this world, inside the VM.
    pub(crate) fn get_raw<T: FromValue>(&mut self, v: RawValue) -> Result<T, Error> {
        self.decode(|cx| T::from_arg(Arg::new(cx, v)))
    }

    /// `to_value`, raw, inside the VM.
    /// The error a conversion gives keeps of another world only its
    /// message (`Error::adopt`).
    pub(crate) fn convert_raw<T: IntoValue>(&mut self, x: T) -> Result<RawValue, Error> {
        let world = self.world_id();
        let raw = x.into_raw(self).map_err(|e| e.adopt(world))?;
        accept(world, raw)
    }

    /// Convert a Scheme value to Rust.
    pub fn get<T: FromValue>(&mut self, v: &Value) -> Result<T, Error> {
        self.host_try(|vm| {
            let raw = vm.import_value(v)?;
            vm.decode(|cx| T::from_arg(Arg::new(cx, raw)))
        })
    }

    /// Look at `v` in place with `f` (a string's text, say, without copying
    /// it): nothing collects or changes the world while `f` looks. A panic
    /// in `f` retires the world, as any at the host's boundary does.
    pub fn inspect<R>(&mut self, v: &Value, f: impl FnOnce(Arg<'_>) -> R) -> Result<R, Error> {
        self.host_try(|vm| {
            let raw = vm.import_value(v)?;
            vm.decode(|cx| Ok(f(Arg::new(cx, raw))))
        })
    }

    /// Convert a Rust value to Scheme.
    pub fn to_value<T: IntoValue>(&mut self, x: T) -> Result<Value, Error> {
        self.host_try(|vm| x.into_value(vm))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exercise both generated entries without the interpreter dispatch
    /// loop, so Miri can check the reference fabrication in isolation.
    fn both(vm: &mut Vm, native: NativeImpl, args: &[RawValue]) -> (RawValue, RawValue) {
        vm.regs.try_resize(args.len(), RawValue::VOID, &mut vm.moves).unwrap();
        vm.regs[..args.len()].copy_from_slice(args);
        let NativeImpl::Plain(rust, c) = native else { panic!("expected plain native") };
        let a = rust(vm, 0, args.len()).unwrap();
        let b = c(vm, 0, args.len()).into_result().unwrap();
        (a, b)
    }

    #[test]
    fn stateless_entries_respect_alignment() {
        #[repr(align(4096))]
        struct Aligned;
        let aligned = Aligned;
        let f = move |n: i64| {
            assert_eq!((&aligned as *const _ as usize) % 4096, 0);
            n + 1
        };
        assert_eq!(std::mem::size_of_val(&f), 0);
        assert_eq!(std::mem::align_of_val(&f), 4096);
        let mut vm = Vm::bare();
        let (a, b) = both(&mut vm, typed(f), &[RawValue::int_unchecked(41)]);
        assert_eq!((a.as_int(), b.as_int()), (42, 42));
        let unit = ();
        let (a, b) = both(
            &mut vm,
            typed(move || {
                let _ = &unit;
                7i64
            }),
            &[],
        );
        assert_eq!((a.as_int(), b.as_int()), (7, 7));
    }

    #[test]
    fn stateless_vm_entries_and_conversion_failure() {
        let mut vm = Vm::bare();
        let native = typed_vm(|vm: &mut Vm, n: i64| -> Result<Value, Error> {
            vm.full_collect_raw();
            vm.to_value(n + 1)
        });
        let NativeImpl::Plain(rust, c) = native else { panic!("expected plain native") };
        let (a, b) = both(&mut vm, NativeImpl::Plain(rust, c), &[RawValue::int_unchecked(41)]);
        assert_eq!((a.as_int(), b.as_int()), (42, 42));
        vm.regs[0] = RawValue::FALSE;
        let expected = rust(&mut vm, 0, 1).unwrap_err();
        let error = c(&mut vm, 0, 1).into_result().unwrap_err();
        assert_eq!(error.msg, expected.msg);
    }

    #[test]
    fn stateless_drop_capture_is_retained() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static DROPS: AtomicUsize = AtomicUsize::new(0);
        struct Noisy;
        impl Drop for Noisy {
            fn drop(&mut self) {
                DROPS.fetch_add(1, Ordering::SeqCst);
            }
        }
        let noisy = Noisy;
        let native = typed(move || {
            let _ = &noisy;
            7i64
        });
        assert!(matches!(native, NativeImpl::Boxed(_)));
        assert_eq!(DROPS.load(Ordering::SeqCst), 0);
        drop(native);
        assert_eq!(DROPS.load(Ordering::SeqCst), 1);
    }

    /// Typed closures holding nothing are called directly, as `natives!`
    /// natives are (compiled code calls their `PlainFn`); the others are
    /// boxed.
    #[test]
    fn stateless_closures_are_plain() {
        fn add(a: i64, b: i64) -> i64 {
            a + b
        }
        let n = Rc::new(Cell::new(0i64));
        let plain = |f: NativeImpl| matches!(f, NativeImpl::Plain(..));
        assert!(plain(typed(|a: i64| a + 1)));
        assert!(plain(typed(add)));
        assert!(plain(typed_vm(|vm: &mut Vm, s: String| vm.to_value(s))));
        assert!(!plain(typed(move || n.get())));
        // Holding nothing but dropping something: not conjured.
        struct Noisy;
        impl Drop for Noisy {
            fn drop(&mut self) {}
        }
        let noisy = Noisy;
        assert!(!plain(typed(move || {
            let _ = &noisy;
            1i64
        })));
    }
}
