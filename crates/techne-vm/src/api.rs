//! Rust embedding API.
//!
//! * `Root`: a handle that keeps a Scheme value alive and up to date while Rust
//!   holds it (the GC moves objects; raw `Value`s are only valid until the next
//!   allocation).
//! * `FromValue` / `IntoValue`: conversions used by `Vm::register_fn`, which
//!   exposes ordinary Rust closures as Scheme procedures.
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
//! assert_eq!(v.as_int(), 42);
//! ```

use std::{any::Any, cell::Cell, fmt::Display, ops::Deref, rc::Rc};

use crate::{
    builtins::{list_values, repr, type_error},
    heap::{Kind, is_kind, str_bytes},
    num,
    value::Value,
    vm::{BoxedNative, Error, Native, NativeImpl, Vm},
};

/// A GC root for a Scheme value held by Rust. Cloning shares the slot.
#[derive(Clone)]
pub struct Root(Rc<Cell<Value>>);

impl Root {
    pub(crate) fn from_cell(cell: Rc<Cell<Value>>) -> Root {
        Root(cell)
    }
    /// The current value. Valid until the next allocation; call again after.
    pub fn get(&self) -> Value {
        self.0.get()
    }
    pub fn set(&self, v: Value) {
        self.0.set(v)
    }
}

impl std::fmt::Debug for Root {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Root({})", repr(self.get()))
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

pub trait FromValue: Sized {
    fn from_value(vm: &mut Vm, v: Value) -> Result<Self, Error>;
}

pub trait IntoValue {
    fn into_value(self, vm: &mut Vm) -> Result<Value, Error>;
}

impl FromValue for Value {
    fn from_value(_: &mut Vm, v: Value) -> Result<Self, Error> {
        Ok(v)
    }
}
impl FromValue for Root {
    fn from_value(vm: &mut Vm, v: Value) -> Result<Self, Error> {
        Ok(vm.root(v))
    }
}
impl FromValue for i64 {
    fn from_value(_: &mut Vm, v: Value) -> Result<Self, Error> {
        if v.is_int() { Ok(v.as_int()) } else { num::integer(v, "integer argument") }
    }
}
impl FromValue for usize {
    fn from_value(vm: &mut Vm, v: Value) -> Result<Self, Error> {
        let i = i64::from_value(vm, v)?;
        usize::try_from(i).map_err(|_| type_error("argument", "non-negative integer", v))
    }
}
impl FromValue for f64 {
    fn from_value(_: &mut Vm, v: Value) -> Result<Self, Error> {
        Ok(num::real(v, "number argument")?.f())
    }
}
impl FromValue for bool {
    /// Scheme truthiness: everything but `#f` is true.
    fn from_value(_: &mut Vm, v: Value) -> Result<Self, Error> {
        Ok(v.is_truthy())
    }
}
impl FromValue for char {
    fn from_value(_: &mut Vm, v: Value) -> Result<Self, Error> {
        if v.is_char() { Ok(v.as_char()) } else { Err(type_error("argument", "char", v)) }
    }
}
impl FromValue for String {
    fn from_value(_: &mut Vm, v: Value) -> Result<Self, Error> {
        if is_kind(v, Kind::String) {
            Ok(String::from_utf8_lossy(unsafe { str_bytes(v.as_ptr()) }).into_owned())
        } else if v.is_symbol() {
            Ok(crate::reader::symbol_name(v.as_symbol()).to_string())
        } else {
            Err(type_error("argument", "string", v))
        }
    }
}
/// Bytes as a bytevector. As an argument, a string gives its UTF-8 bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bytes(pub Vec<u8>);

impl FromValue for Bytes {
    fn from_value(_: &mut Vm, v: Value) -> Result<Self, Error> {
        if is_kind(v, Kind::Bytevector) || is_kind(v, Kind::String) {
            Ok(Bytes(unsafe { str_bytes(v.as_ptr()) }.to_vec()))
        } else {
            Err(type_error("argument", "bytevector or string", v))
        }
    }
}
impl IntoValue for Bytes {
    fn into_value(self, vm: &mut Vm) -> Result<Value, Error> {
        Ok(vm.make_bytevector(&self.0))
    }
}

impl<T: FromValue> FromValue for Vec<T> {
    /// From a list or a vector. Elements are rooted while converting.
    fn from_value(vm: &mut Vm, v: Value) -> Result<Self, Error> {
        let items = if is_kind(v, Kind::Vector) {
            (0..unsafe { crate::heap::len_of(v.as_ptr()) }).map(|i| unsafe { crate::heap::field(v.as_ptr(), i) }).collect()
        } else {
            list_values(v).ok_or_else(|| type_error("argument", "list or vector", v))?
        };
        let roots: Vec<Root> = items.into_iter().map(|x| vm.root(x)).collect();
        roots.iter().map(|r| T::from_value(vm, r.get())).collect()
    }
}
impl<T: FromValue> FromValue for Option<T> {
    /// `#f` is `None`.
    fn from_value(vm: &mut Vm, v: Value) -> Result<Self, Error> {
        if v.is_false() { Ok(None) } else { T::from_value(vm, v).map(Some) }
    }
}
impl<T: 'static> FromValue for Foreign<T> {
    fn from_value(vm: &mut Vm, v: Value) -> Result<Self, Error> {
        let expected = std::any::type_name::<T>();
        let (rc, _) = vm.foreign(v).ok_or_else(|| type_error("argument", expected, v))?;
        rc.clone().downcast::<T>().map(Foreign).map_err(|_| type_error("argument", expected, v))
    }
}

impl IntoValue for Value {
    fn into_value(self, _: &mut Vm) -> Result<Value, Error> {
        Ok(self)
    }
}
impl IntoValue for &Root {
    fn into_value(self, _: &mut Vm) -> Result<Value, Error> {
        Ok(self.get())
    }
}
impl IntoValue for Root {
    fn into_value(self, _: &mut Vm) -> Result<Value, Error> {
        Ok(self.get())
    }
}
impl IntoValue for () {
    fn into_value(self, _: &mut Vm) -> Result<Value, Error> {
        Ok(Value::VOID)
    }
}
impl IntoValue for bool {
    fn into_value(self, _: &mut Vm) -> Result<Value, Error> {
        Ok(Value::bool(self))
    }
}
impl IntoValue for i64 {
    fn into_value(self, vm: &mut Vm) -> Result<Value, Error> {
        Ok(vm.make_int(self))
    }
}
impl IntoValue for i32 {
    fn into_value(self, vm: &mut Vm) -> Result<Value, Error> {
        Ok(vm.make_int(self as i64))
    }
}
impl IntoValue for usize {
    fn into_value(self, vm: &mut Vm) -> Result<Value, Error> {
        Ok(vm.make_int(self as i64))
    }
}
impl IntoValue for f64 {
    fn into_value(self, _: &mut Vm) -> Result<Value, Error> {
        Ok(Value::float(self))
    }
}
impl IntoValue for char {
    fn into_value(self, _: &mut Vm) -> Result<Value, Error> {
        Ok(Value::char(self))
    }
}
impl IntoValue for String {
    fn into_value(self, vm: &mut Vm) -> Result<Value, Error> {
        Ok(vm.make_string(self.as_bytes()))
    }
}
impl IntoValue for &str {
    fn into_value(self, vm: &mut Vm) -> Result<Value, Error> {
        Ok(vm.make_string(self.as_bytes()))
    }
}
impl<T: IntoValue> IntoValue for Vec<T> {
    /// As a list. Converted elements are rooted until the list is built.
    fn into_value(self, vm: &mut Vm) -> Result<Value, Error> {
        let mark = vm.scratch.len();
        for x in self {
            let v = x.into_value(vm)?;
            vm.scratch.push(v);
        }
        let items: Vec<Value> = vm.scratch[mark..].to_vec();
        let list = vm.make_list(&items);
        vm.scratch.truncate(mark);
        Ok(list)
    }
}
impl<T: IntoValue> IntoValue for Option<T> {
    /// `None` is `#f`.
    fn into_value(self, vm: &mut Vm) -> Result<Value, Error> {
        match self {
            Some(x) => x.into_value(vm),
            None => Ok(Value::FALSE),
        }
    }
}
impl<T: 'static> IntoValue for Foreign<T> {
    fn into_value(self, vm: &mut Vm) -> Result<Value, Error> {
        let rc: Rc<dyn Any> = self.0;
        Ok(vm.make_foreign(rc, std::any::type_name::<T>()))
    }
}
impl<T: IntoValue, E: Display + 'static> IntoValue for Result<T, E> {
    /// `Err` raises a Scheme error with the error's message. A `vm::Error`
    /// passes through unchanged (keeping raised objects and escapes).
    fn into_value(self, vm: &mut Vm) -> Result<Value, Error> {
        match self {
            Ok(x) => x.into_value(vm),
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

/// Rust closures callable from Scheme; implemented for `Fn(A, B, ...) -> R`.
pub trait IntoNative<Args> {
    const ARITY: usize;
    fn into_native(self) -> BoxedNative;
}

macro_rules! impl_into_native {
    ($($a:ident),*) => {
        impl<F, Ret, $($a),*> IntoNative<($($a,)*)> for F
        where
            F: Fn($($a),*) -> Ret + 'static,
            Ret: IntoValue,
            $($a: FromValue,)*
        {
            const ARITY: usize = impl_into_native!(@count $($a)*);
            #[allow(non_snake_case, unused_mut, unused_variables, unused_assignments)]
            fn into_native(self) -> BoxedNative {
                Rc::new(move |vm: &mut Vm, args: usize, _n: usize| {
                    let mut i = 0;
                    $(
                        let v = vm.regs[args + i];
                        let $a = <$a as FromValue>::from_value(vm, v)?;
                        i += 1;
                    )*
                    (self)($($a),*).into_value(vm)
                })
            }
        }
    };
    (@count) => { 0 };
    (@count $x:ident $($rest:ident)*) => { 1 + impl_into_native!(@count $($rest)*) };
}

/// Rust closures that also receive the VM: `Fn(&mut Vm, A, B, ...) -> R`.
pub trait IntoNativeVm<Args> {
    const ARITY: usize;
    fn into_native(self) -> BoxedNative;
}

macro_rules! impl_into_native_vm {
    ($($a:ident),*) => {
        impl<F, Ret, $($a),*> IntoNativeVm<($($a,)*)> for F
        where
            F: Fn(&mut Vm, $($a),*) -> Ret + 'static,
            Ret: IntoValue,
            $($a: FromValue,)*
        {
            const ARITY: usize = impl_into_native!(@count $($a)*);
            #[allow(non_snake_case, unused_mut, unused_variables, unused_assignments)]
            fn into_native(self) -> BoxedNative {
                Rc::new(move |vm: &mut Vm, args: usize, _n: usize| {
                    let mut i = 0;
                    $(
                        let v = vm.regs[args + i];
                        let $a = <$a as FromValue>::from_value(vm, v)?;
                        i += 1;
                    )*
                    (self)(vm, $($a),*).into_value(vm)
                })
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

impl Vm {
    /// Expose a Rust closure as a global Scheme procedure.
    pub fn register_fn<Args, F: IntoNative<Args>>(&mut self, name: &str, f: F) {
        let arity = F::ARITY;
        self.define_native(Native { name: name.into(), f: NativeImpl::Boxed(f.into_native()), min: arity, max: Some(arity) });
    }

    /// Like `register_fn`, for closures that need the VM (to call back into
    /// Scheme, allocate, or inspect values).
    pub fn register_fn_vm<Args, F: IntoNativeVm<Args>>(&mut self, name: &str, f: F) {
        let arity = F::ARITY;
        self.define_native(Native { name: name.into(), f: NativeImpl::Boxed(f.into_native()), min: arity, max: Some(arity) });
    }

    /// Expose a variadic Rust function that receives the raw arguments.
    /// Values in `args` are only valid until the function allocates.
    pub fn register_variadic(&mut self, name: &str, f: impl Fn(&mut Vm, &[Value]) -> Result<Value, Error> + 'static) {
        let f: BoxedNative = Rc::new(move |vm: &mut Vm, args: usize, n: usize| {
            let vals = vm.regs[args..args + n].to_vec();
            f(vm, &vals)
        });
        self.define_native(Native { name: name.into(), f: NativeImpl::Boxed(f), min: 0, max: None });
    }

    /// Name a foreign Rust type for Scheme: `type-of` returns the name and
    /// `define-method` can dispatch on it (`(define-method (show (b buffer)) ...)`).
    pub fn name_foreign_type<T: 'static>(&mut self, name: &str) {
        self.foreign_type_names.insert(std::any::type_name::<T>(), crate::reader::intern(name));
    }

    /// Call a global procedure by name with converted arguments.
    pub fn call_global(&mut self, name: &str, args: &[Value]) -> Result<Value, Error> {
        let f = self.get_global(name).ok_or_else(|| Error::new(format!("unbound procedure {name}")))?;
        self.call(f, args)
    }

    /// Convert a Scheme value to Rust.
    pub fn get<T: FromValue>(&mut self, v: Value) -> Result<T, Error> {
        T::from_value(self, v)
    }

    /// Convert a Rust value to Scheme.
    pub fn to_value<T: IntoValue>(&mut self, x: T) -> Result<Value, Error> {
        x.into_value(self)
    }
}
