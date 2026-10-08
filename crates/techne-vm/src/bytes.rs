//! Bytevectors (R7RS 6.9): bytes in one heap object, a raw kind like
//! strings, so the collector never scans them. Literals (`#u8(...)`) are
//! immutable.

use crate::{
    builtins::{index_arg, int_arg, range_args, str_arg, type_error},
    heap::{self, Kind, bytes_mut, is_kind, str_bytes},
    value::Value,
    vm::{Error, Vm},
};

type R = Result<Value, Error>;

fn arg(vm: &Vm, args: usize, i: usize) -> Value {
    vm.regs[args + i]
}

/// The bytes of bytevector `v`, valid until the next allocation (which may
/// move or free it): for natives, not the embedding API (`api::Bytes`).
pub(crate) fn bytes_arg<'a>(v: Value, who: &str) -> Result<&'a [u8], Error> {
    if is_kind(v, Kind::Bytevector) { Ok(unsafe { str_bytes(v.as_ptr()) }) } else { Err(type_error(who, "bytevector", v)) }
}

/// The bytes of bytevector `v`, to change: literals are refused.
fn mutable_bytes<'a>(v: Value, who: &str) -> Result<&'a mut [u8], Error> {
    bytes_arg(v, who)?;
    let p = v.as_ptr();
    if unsafe { *p } & heap::IMMUTABLE != 0 {
        return Err(Error::new(format!("{who}: bytevector literals cannot be changed")));
    }
    Ok(unsafe { bytes_mut(p) })
}

pub fn byte_arg(v: Value, who: &str) -> Result<u8, Error> {
    match int_arg(v, who) {
        Ok(b @ 0..=255) if v.is_int() => Ok(b as u8),
        _ => Err(type_error(who, "byte (an exact integer from 0 to 255)", v)),
    }
}

/// The bytes of bytevector argument 0 in the optional range at 1.
fn byte_range<'a>(vm: &Vm, args: usize, n: usize, who: &str) -> Result<&'a [u8], Error> {
    let bytes = bytes_arg(arg(vm, args, 0), who)?;
    let (a, b) = range_args(vm, args, n, 1, bytes.len(), who)?;
    Ok(&bytes[a..b])
}

fn make_bytevector(vm: &mut Vm, args: usize, n: usize) -> R {
    let len = index_arg(arg(vm, args, 0), "make-bytevector")?;
    let fill = if n > 1 { byte_arg(arg(vm, args, 1), "make-bytevector")? } else { 0 };
    Ok(vm.make_bytevector(&vec![fill; len]))
}

fn bytevector(vm: &mut Vm, args: usize, n: usize) -> R {
    let bytes = (0..n).map(|i| byte_arg(arg(vm, args, i), "bytevector")).collect::<Result<Vec<_>, _>>()?;
    Ok(vm.make_bytevector(&bytes))
}

fn u8_ref(vm: &mut Vm, args: usize, _: usize) -> R {
    let (v, k) = (arg(vm, args, 0), arg(vm, args, 1));
    let bytes = bytes_arg(v, "bytevector-u8-ref")?;
    let i = index_arg(k, "bytevector-u8-ref")?;
    bytes.get(i).map(|&b| Value::int_unchecked(b as i64)).ok_or_else(|| crate::builtins::index_error("bytevector-u8-ref", v, k))
}

fn u8_set(vm: &mut Vm, args: usize, _: usize) -> R {
    let (v, k) = (arg(vm, args, 0), arg(vm, args, 1));
    let bytes = mutable_bytes(v, "bytevector-u8-set!")?;
    let i = index_arg(k, "bytevector-u8-set!")?;
    let b = byte_arg(arg(vm, args, 2), "bytevector-u8-set!")?;
    *bytes.get_mut(i).ok_or_else(|| crate::builtins::index_error("bytevector-u8-set!", v, k))? = b;
    Ok(Value::VOID)
}

/// `(bytevector-copy! to at from [start end])`, overlapping ranges included.
fn copy_into(vm: &mut Vm, args: usize, n: usize) -> R {
    let to = arg(vm, args, 0);
    mutable_bytes(to, "bytevector-copy!")?;
    let at = index_arg(arg(vm, args, 1), "bytevector-copy!")?;
    let from = bytes_arg(arg(vm, args, 2), "bytevector-copy!")?;
    let (a, b) = range_args(vm, args, n, 3, from.len(), "bytevector-copy!")?;
    let len = unsafe { heap::len_of(to.as_ptr()) };
    if at + (b - a) > len {
        return Err(Error::new("bytevector-copy!: not enough room in the target"));
    }
    // `from` and `to` may be the same object: copy through raw pointers.
    unsafe { std::ptr::copy(from.as_ptr().add(a), (to.as_ptr().add(1) as *mut u8).add(at), b - a) };
    Ok(Value::VOID)
}

fn append(vm: &mut Vm, args: usize, n: usize) -> R {
    let mut out = Vec::new();
    for i in 0..n {
        out.extend_from_slice(bytes_arg(arg(vm, args, i), "bytevector-append")?);
    }
    Ok(vm.make_bytevector(&out))
}

/// `utf8->string`: invalid UTF-8 is an error, naming where it starts.
fn utf8_to_string(vm: &mut Vm, args: usize, n: usize) -> R {
    let bytes = byte_range(vm, args, n, "utf8->string")?;
    match std::str::from_utf8(bytes) {
        Ok(s) => {
            let s = s.to_owned();
            Ok(vm.make_string(&s))
        }
        Err(e) => Err(Error::new(format!("utf8->string: invalid UTF-8 at byte {}", e.valid_up_to()))),
    }
}

/// `string->utf8`: the range is in characters.
fn string_to_utf8(vm: &mut Vm, args: usize, n: usize) -> R {
    let s = str_arg(arg(vm, args, 0), "string->utf8")?;
    let len = s.chars().count();
    let (a, b) = range_args(vm, args, n, 1, len, "string->utf8")?;
    let offset = |k: usize| s.char_indices().nth(k).map_or(s.len(), |(i, _)| i);
    let bytes = s.as_bytes()[offset(a)..offset(b)].to_vec();
    Ok(vm.make_bytevector(&bytes))
}

pub fn install(vm: &mut Vm) {
    crate::natives! { vm;
        /// Return #t if OBJ is a bytevector.
        "(bytevector? obj)" => |vm: &mut Vm, a, _| Ok(Value::bool(is_kind(arg(vm, a, 0), Kind::Bytevector)));
        /// Return a new bytevector of K bytes, each BYTE.
        "(make-bytevector k [byte])" => make_bytevector;
        /// Return a new bytevector of BYTES.
        "(bytevector . bytes)" => bytevector;
        /// Return the number of bytes of BYTEVECTOR.
        "(bytevector-length bytevector)" => |vm: &mut Vm, a, _| Ok(Value::int_unchecked(bytes_arg(arg(vm, a, 0), "bytevector-length")?.len() as i64));
        /// Return byte K of BYTEVECTOR, counting from 0.
        "(bytevector-u8-ref bytevector k)" => u8_ref;
        /// Store BYTE as byte K of BYTEVECTOR.
        "(bytevector-u8-set! bytevector k byte)" => u8_set;
        /// Return a new bytevector of the bytes of BYTEVECTOR from START to END.
        "(bytevector-copy bytevector [start] [end])" => |vm: &mut Vm, a, n| { let b = byte_range(vm, a, n, "bytevector-copy")?.to_vec(); Ok(vm.make_bytevector(&b)) };
        /// Copy the bytes of FROM from START to END into TO at AT.
        "(bytevector-copy! to at from [start] [end])" => copy_into;
        /// Return a new bytevector of the bytes of BYTEVECTORS in order.
        "(bytevector-append . bytevectors)" => append;
        /// Return the string BYTEVECTOR encodes in UTF-8, from START to END.
        "(utf8->string bytevector [start] [end])" => utf8_to_string;
        /// Return the UTF-8 encoding of STRING from START to END.
        "(string->utf8 string [start] [end])" => string_to_utf8;
    }
}
