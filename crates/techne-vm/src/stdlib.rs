//! Natives beyond the core: records, conditions, control, ports, strings,
//! hash-table views and system access.

use crate::{
    builtins::{Bulk, error_object_parts, list_values, repr, type_error},
    heap::{Kind, field, header, is_kind, len_of, set_field, str_bytes},
    reader::{self, symbol_name},
    value::Value,
    vm::{Capability, Error, Native, NativeFn, NativeImpl, SpecialObj, Vm},
};

type R = Result<Value, Error>;

fn arg(vm: &Vm, args: usize, i: usize) -> Value {
    vm.regs[args + i]
}

fn args_vec(vm: &Vm, args: usize, n: usize) -> Vec<Value> {
    vm.regs[args..args + n].to_vec()
}

fn string(vm: &Vm, v: Value, who: &str) -> Result<String, Error> {
    let _ = vm;
    if is_kind(v, Kind::String) {
        Ok(String::from_utf8_lossy(unsafe { str_bytes(v.as_ptr()) }).into_owned())
    } else {
        Err(type_error(who, "string", v))
    }
}

// ----- records -----

/// `(%make-rtd name fields [applicable-field])`. Calling a record whose type
/// has an applicable field calls the procedure stored in that field.
fn make_rtd(vm: &mut Vm, args: usize, n: usize) -> R {
    let id = vm.fresh_id();
    let p = vm.alloc(5);
    unsafe {
        *p = header(Kind::Rtd, 4, 0);
        set_field(p, 0, arg(vm, args, 0));
        set_field(p, 1, arg(vm, args, 1));
        set_field(p, 2, Value::int_unchecked(id));
        set_field(p, 3, if n > 2 { arg(vm, args, 2) } else { Value::FALSE });
    }
    Ok(Value::ptr(p))
}

fn record(vm: &mut Vm, args: usize, n: usize) -> R {
    let rtd = arg(vm, args, 0);
    let fields = vm.regs[args + 1..args + n].to_vec();
    Ok(vm.make_record(rtd, &fields))
}

fn record_check(v: Value, rtd: Value) -> bool {
    is_kind(v, Kind::Record) && unsafe { field(v.as_ptr(), 0) } == rtd
}

fn record_type_error(v: Value, rtd: Value) -> Error {
    let name = symbol_name(unsafe { field(rtd.as_ptr(), 0) }.as_symbol());
    type_error("record accessor", &name, v)
}

fn record_p(vm: &mut Vm, args: usize, _: usize) -> R {
    Ok(Value::bool(record_check(arg(vm, args, 0), arg(vm, args, 1))))
}

fn record_ref(vm: &mut Vm, args: usize, _: usize) -> R {
    let (v, rtd, i) = (arg(vm, args, 0), arg(vm, args, 1), arg(vm, args, 2));
    if !record_check(v, rtd) {
        return Err(record_type_error(v, rtd));
    }
    Ok(unsafe { field(v.as_ptr(), 1 + i.as_int() as usize) })
}

fn record_set(vm: &mut Vm, args: usize, _: usize) -> R {
    let (v, rtd, i, x) = (arg(vm, args, 0), arg(vm, args, 1), arg(vm, args, 2), arg(vm, args, 3));
    if !record_check(v, rtd) {
        return Err(record_type_error(v, rtd));
    }
    unsafe { set_field(v.as_ptr(), 1 + i.as_int() as usize, x) };
    vm.write_barrier(v.as_ptr(), x);
    Ok(Value::VOID)
}

// ----- conditions and control -----

fn raise(vm: &mut Vm, args: usize, _: usize) -> R {
    let v = arg(vm, args, 0);
    Err(vm.raise_error(v))
}

fn error_object_message(vm: &mut Vm, args: usize, _: usize) -> R {
    let v = arg(vm, args, 0);
    error_object_parts(vm, v).map(|(m, _)| m).ok_or_else(|| type_error("error-object-message", "error object", v))
}

fn error_object_irritants(vm: &mut Vm, args: usize, _: usize) -> R {
    let v = arg(vm, args, 0);
    error_object_parts(vm, v).map(|(_, i)| i).ok_or_else(|| type_error("error-object-irritants", "error object", v))
}

fn values(vm: &mut Vm, args: usize, n: usize) -> R {
    if n == 1 {
        return Ok(arg(vm, args, 0));
    }
    let vals = args_vec(vm, args, n);
    let rtd = vm.special(SpecialObj::ValuesRtd);
    Ok(vm.make_record(rtd, &vals))
}

fn apply(vm: &mut Vm, args: usize, n: usize) -> R {
    let f = arg(vm, args, 0);
    let mut vals = vm.regs[args + 1..args + n - 1].to_vec();
    let last = arg(vm, args, n - 1);
    vals.extend(list_values(last).ok_or_else(|| type_error("apply", "list", last))?);
    vm.call(f, &vals)
}

/// `(eval datum [module])`: in the named module, or the current one.
fn eval(vm: &mut Vm, args: usize, n: usize) -> R {
    let datum = value_to_sexp(arg(vm, args, 0))?;
    let module = if n > 1 { module_arg(vm, arg(vm, args, 1), "eval")? } else { vm.current_module() };
    vm.eval_sexp_in(module, &datum)
}

/// A module named by a string (`root`, `user` or a file path; see `Vm::find_module`).
fn module_arg(vm: &mut Vm, v: Value, who: &str) -> Result<u32, Error> {
    let name = string(vm, v, who)?;
    vm.find_module(&name)
}

/// Scheme data back to syntax (for `eval`).
pub fn value_to_sexp(v: Value) -> Result<reader::Sexp, Error> {
    use reader::Sexp;
    if v.is_int() {
        return Ok(Sexp::Int(v.as_int()));
    }
    if v.is_float() {
        return Ok(Sexp::Float(v.as_float()));
    }
    if is_kind(v, Kind::BigInt) {
        return Ok(match crate::num::heap_int(v) {
            crate::num::N::I(i) => Sexp::Int(i),
            crate::num::N::B(b) => Sexp::BigInt(std::rc::Rc::new(b)),
            crate::num::N::R(_) | crate::num::N::F(_) | crate::num::N::C(_) => unreachable!(),
        });
    }
    if is_kind(v, Kind::Ratio) || is_kind(v, Kind::Complex) {
        return Ok(reader::number(crate::num::num(v, "eval")?));
    }
    if v.is_char() {
        return Ok(Sexp::Char(v.as_char()));
    }
    if v.is_symbol() {
        return Ok(Sexp::Sym(v.as_symbol()));
    }
    if v == Value::TRUE || v == Value::FALSE {
        return Ok(Sexp::Bool(v.is_truthy()));
    }
    if v == Value::NIL {
        return Ok(Sexp::list_of(vec![]));
    }
    if is_kind(v, Kind::String) {
        return Ok(Sexp::Str(String::from_utf8_lossy(unsafe { str_bytes(v.as_ptr()) }).as_ref().into()));
    }
    if is_kind(v, Kind::Bytevector) {
        return Ok(Sexp::Bytes(unsafe { str_bytes(v.as_ptr()) }.into()));
    }
    if is_kind(v, Kind::Pair) {
        let mut items = Vec::new();
        let mut l = v;
        while is_kind(l, Kind::Pair) {
            items.push(value_to_sexp(unsafe { field(l.as_ptr(), 0) })?);
            l = unsafe { field(l.as_ptr(), 1) };
        }
        let tail = if l == Value::NIL { None } else { Some(Box::new(value_to_sexp(l)?)) };
        return Ok(Sexp::List(items, tail, reader::NO_POS));
    }
    if is_kind(v, Kind::Vector) {
        let items = (0..unsafe { len_of(v.as_ptr()) }).map(|i| value_to_sexp(unsafe { field(v.as_ptr(), i) })).collect::<Result<_, _>>()?;
        return Ok(Sexp::Vector(items));
    }
    Err(type_error("eval", "datum", v))
}

// ----- types and dispatch -----

const BUILTIN_TYPES: &[&str] = &[
    "t",
    "number",
    "integer",
    "ratio",
    "float",
    "complex",
    "string",
    "symbol",
    "keyword",
    "char",
    "list",
    "pair",
    "null",
    "vector",
    "bytevector",
    "procedure",
    "boolean",
    "hash-table",
    "record",
    "void",
    "eof",
    "foreign",
];

/// Dispatch key of a value: the record type id for records, otherwise a type symbol.
fn type_key(vm: &Vm, v: Value) -> Value {
    let name = if v.is_int() || is_kind(v, Kind::BigInt) {
        "integer"
    } else if is_kind(v, Kind::Ratio) {
        "ratio"
    } else if is_kind(v, Kind::Complex) {
        "complex"
    } else if v.is_float() {
        "float"
    } else if v.is_symbol() {
        "symbol"
    } else if v.is_keyword() {
        "keyword"
    } else if v.is_char() {
        "char"
    } else if v == Value::NIL {
        "null"
    } else if v == Value::TRUE || v == Value::FALSE {
        "boolean"
    } else if v == Value::EOF {
        "eof"
    } else if v.is_native() || is_kind(v, Kind::Closure) {
        "procedure"
    } else if is_kind(v, Kind::Record) {
        return unsafe { field(field(v.as_ptr(), 0).as_ptr(), 2) };
    } else if is_kind(v, Kind::Pair) {
        "pair"
    } else if is_kind(v, Kind::String) {
        "string"
    } else if is_kind(v, Kind::Vector) {
        "vector"
    } else if is_kind(v, Kind::Bytevector) {
        "bytevector"
    } else if is_kind(v, Kind::Table) {
        "hash-table"
    } else if let Some((_, type_name)) = vm.foreign(v) {
        return Value::symbol(vm.foreign_type_names.get(type_name).copied().unwrap_or_else(|| reader::intern("foreign")));
    } else {
        "void"
    };
    Value::symbol(reader::intern(name))
}

/// The next more general dispatch key, or `#f` after `t`.
fn type_parent(vm: &Vm, key: Value) -> Value {
    if key.is_int() {
        return Value::symbol(reader::intern("record"));
    }
    if !key.is_symbol() {
        return Value::FALSE;
    }
    let name = symbol_name(key.as_symbol());
    let parent = match &*name {
        "t" => return Value::FALSE,
        "integer" | "ratio" | "float" | "complex" => "number",
        "pair" | "null" => "list",
        _ if vm.foreign_type_names.values().any(|s| *s == key.as_symbol()) => "foreign",
        _ => "t",
    };
    Value::symbol(reader::intern(parent))
}

/// `(%type-designator 'name thunk)`: a builtin or foreign type name, else the
/// record type the thunk evaluates to.
fn type_designator(vm: &mut Vm, args: usize, _: usize) -> R {
    let name = arg(vm, args, 0);
    let sym = name.as_symbol();
    if BUILTIN_TYPES.contains(&&*symbol_name(sym)) || vm.foreign_type_names.values().any(|s| *s == sym) {
        return Ok(name);
    }
    let rtd = vm.call(arg(vm, args, 1), &[])?;
    if is_kind(rtd, Kind::Rtd) {
        Ok(unsafe { field(rtd.as_ptr(), 2) })
    } else {
        Err(Error::new(format!("unknown type {}", symbol_name(sym))))
    }
}

/// Readable type name: a type symbol, or the record type's name.
fn type_of(vm: &mut Vm, args: usize, _: usize) -> R {
    let v = arg(vm, args, 0);
    if is_kind(v, Kind::Record) {
        return Ok(unsafe { field(field(v.as_ptr(), 0).as_ptr(), 0) });
    }
    Ok(type_key(vm, v))
}

// ----- keyword and optional arguments -----

/// `(%parse-args rest nopt keywords name has-rest)`: a vector with the optional
/// positional values, then the keyword values (`#<unset>` when absent), then
/// (with `has-rest`) a list of the remaining positional arguments.
fn parse_args(vm: &mut Vm, args: usize, _: usize) -> R {
    let rest = list_values(arg(vm, args, 0)).ok_or_else(|| Error::new("internal: bad argument list"))?;
    let nopt = arg(vm, args, 1).as_int() as usize;
    let keywords = list_values(arg(vm, args, 2)).unwrap_or_default();
    let name = repr(arg(vm, args, 3));
    let has_rest = arg(vm, args, 4).is_truthy();
    let mut out = vec![Value::UNSET; nopt + keywords.len()];
    let mut extra = Vec::new();
    let mut positional = 0;
    let mut i = 0;
    while i < rest.len() {
        let x = rest[i];
        if x.is_keyword() {
            let k =
                keywords.iter().position(|k| *k == x).ok_or_else(|| Error::new(format!("{name}: unknown keyword argument {}", repr(x))))?;
            let v = rest.get(i + 1).ok_or_else(|| Error::new(format!("{name}: missing value for keyword {}", repr(x))))?;
            out[nopt + k] = *v;
            i += 2;
            continue;
        }
        if positional < nopt {
            out[positional] = x;
            positional += 1;
        } else if has_rest {
            extra.push(x);
        } else {
            return Err(Error::new(format!("{name}: too many arguments")));
        }
        i += 1;
    }
    // Root the collected values: building the rest list may move them.
    let mark = vm.scratch.len();
    vm.scratch.extend_from_slice(&out);
    if has_rest {
        let list = vm.make_list(&extra);
        vm.scratch.push(list);
    }
    let items = vm.scratch[mark..].to_vec();
    let v = vm.make_vector(&items);
    vm.scratch.truncate(mark);
    Ok(v)
}

/// The `kind` of an error object (`file`, `read`), if it has one.
fn error_kind(vm: &Vm, v: Value) -> Option<&'static str> {
    error_object_parts(vm, v)?;
    let kind = unsafe { field(v.as_ptr(), 3) };
    kind.is_symbol().then(|| match &*symbol_name(kind.as_symbol()) {
        "file" => "file",
        "read" => "read",
        _ => "other",
    })
}

// ----- ports -----

pub use crate::ports::{OUTPUT_PORT_KEY, display_to, make_output_port, write_out};

// ----- strings -----

fn str_fn(vm: &mut Vm, args: usize, who: &str, f: impl Fn(&str) -> String) -> R {
    let s = string(vm, arg(vm, args, 0), who)?;
    let out = f(&s);
    Ok(vm.make_string(out.as_bytes()))
}

fn string_split(vm: &mut Vm, args: usize, n: usize) -> R {
    let s = string(vm, arg(vm, args, 0), "string-split")?;
    let parts: Vec<String> = if n > 1 {
        let sep = arg(vm, args, 1);
        let sep = if sep.is_char() { sep.as_char().to_string() } else { string(vm, sep, "string-split")? };
        s.split(sep.as_str()).map(str::to_owned).collect()
    } else {
        s.split_whitespace().map(str::to_owned).collect()
    };
    vm.to_value(parts)
}

fn string_join(vm: &mut Vm, args: usize, n: usize) -> R {
    let parts: Vec<String> = vm.get(arg(vm, args, 0))?;
    let sep = if n > 1 { string(vm, arg(vm, args, 1), "string-join")? } else { " ".into() };
    Ok(vm.make_string(parts.join(&sep).as_bytes()))
}

fn string_contains(vm: &mut Vm, args: usize, _: usize) -> R {
    let s = string(vm, arg(vm, args, 0), "string-contains")?;
    let sub = string(vm, arg(vm, args, 1), "string-contains")?;
    Ok(match s.find(&sub) {
        Some(i) => Value::int_unchecked(s[..i].chars().count() as i64),
        None => Value::FALSE,
    })
}

fn string_index(vm: &mut Vm, args: usize, _: usize) -> R {
    let s = string(vm, arg(vm, args, 0), "string-index")?;
    let c = arg(vm, args, 1);
    if !c.is_char() {
        return Err(type_error("string-index", "char", c));
    }
    Ok(s.chars().position(|x| x == c.as_char()).map_or(Value::FALSE, |i| Value::int_unchecked(i as i64)))
}

fn string_replace(vm: &mut Vm, args: usize, _: usize) -> R {
    let s = string(vm, arg(vm, args, 0), "string-replace")?;
    let from = string(vm, arg(vm, args, 1), "string-replace")?;
    let to = string(vm, arg(vm, args, 2), "string-replace")?;
    Ok(vm.make_string(s.replace(&from, &to).as_bytes()))
}

fn string_from_chars(vm: &mut Vm, args: usize, n: usize) -> R {
    let s = (0..n)
        .map(|i| {
            let c = arg(vm, args, i);
            if c.is_char() { Ok(c.as_char()) } else { Err(type_error("string", "char", c)) }
        })
        .collect::<Result<String, _>>()?;
    Ok(vm.make_string(s.as_bytes()))
}

fn string_cmp(vm: &mut Vm, args: usize, n: usize, who: &str, ci: bool, ok: fn(std::cmp::Ordering) -> bool) -> R {
    let get = |vm: &Vm, i| string(vm, arg(vm, args, i), who).map(|s| if ci { crate::builtins::fold_string(&s) } else { s });
    for i in 1..n {
        if !ok(get(vm, i - 1)?.cmp(&get(vm, i)?)) {
            return Ok(Value::FALSE);
        }
    }
    Ok(Value::TRUE)
}

// ----- vectors and hash tables -----

fn vector_copy(vm: &mut Vm, args: usize, n: usize) -> R {
    let v = arg(vm, args, 0);
    if !is_kind(v, Kind::Vector) {
        return Err(type_error("vector-copy", "vector", v));
    }
    let len = unsafe { len_of(v.as_ptr()) };
    let start: usize = if n > 1 { vm.get(arg(vm, args, 1))? } else { 0 };
    let end: usize = if n > 2 { vm.get(arg(vm, args, 2))? } else { len };
    if start > end || end > len {
        return Err(Error::new(format!("vector-copy: bad range {start}..{end}")));
    }
    let p = Bulk::new(vm, 1 + end - start).take(vm, 1 + end - start);
    let v = arg(vm, args, 0).as_ptr();
    unsafe {
        *p = header(Kind::Vector, end - start, 0);
        for i in start..end {
            set_field(p, i - start, field(v, i));
        }
    }
    Ok(Value::ptr(p))
}

fn table_entries(vm: &Vm, t: Value) -> Result<Vec<(Value, Value)>, Error> {
    let _ = vm;
    if !is_kind(t, Kind::Table) {
        return Err(type_error("hash table operation", "hash table", t));
    }
    let slots = unsafe { field(t.as_ptr(), 1).as_ptr() };
    Ok((0..unsafe { len_of(slots) } / 2)
        .filter_map(|i| {
            let k = unsafe { field(slots, 2 * i) };
            (k != Value::EMPTY && k != Value::UNDEFINED).then(|| (k, unsafe { field(slots, 2 * i + 1) }))
        })
        .collect())
}

fn hash_keys(vm: &mut Vm, args: usize, _: usize) -> R {
    let keys: Vec<Value> = table_entries(vm, arg(vm, args, 0))?.into_iter().map(|(k, _)| k).collect();
    Ok(vm.make_list(&keys))
}

fn hash_values(vm: &mut Vm, args: usize, _: usize) -> R {
    let vals: Vec<Value> = table_entries(vm, arg(vm, args, 0))?.into_iter().map(|(_, v)| v).collect();
    Ok(vm.make_list(&vals))
}

fn hash_to_alist(vm: &mut Vm, args: usize, _: usize) -> R {
    let entries = table_entries(vm, arg(vm, args, 0))?;
    let mut b = Bulk::new(vm, 6 * entries.len());
    let t = arg(vm, args, 0);
    let entries = table_entries(vm, t)?; // re-read after allocating
    let mut acc = Value::NIL;
    for (k, v) in entries.into_iter().rev() {
        let p = b.take(vm, 3);
        unsafe {
            *p = header(Kind::Pair, 2, 0);
            set_field(p, 0, k);
            set_field(p, 1, v);
        }
        let q = b.take(vm, 3);
        unsafe {
            *q = header(Kind::Pair, 2, 0);
            set_field(q, 0, Value::ptr(p));
            set_field(q, 1, acc);
        }
        acc = Value::ptr(q);
    }
    Ok(acc)
}

// ----- system -----

/// When `current-jiffy` counts from: the first use.
fn jiffy_epoch() -> std::time::Instant {
    thread_local!(static EPOCH: std::time::Instant = std::time::Instant::now());
    EPOCH.with(|e| *e)
}

fn current_ms(_: &mut Vm, _: usize, _: usize) -> R {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    Ok(Value::int_unchecked(ms as i64))
}

fn file_to_string(vm: &mut Vm, args: usize, _: usize) -> R {
    let path = string(vm, arg(vm, args, 0), "file->string")?;
    let text = std::fs::read_to_string(&path).map_err(|e| Error::new(format!("{path}: {e}")))?;
    Ok(vm.make_string(text.as_bytes()))
}

macro_rules! natives {
    ($vm:expr; $($name:literal $min:literal $max:tt => $f:expr;)*) => {
        $( {
            let f: NativeFn = $f;
            $vm.define_native(Native { name: $name.into(), f: NativeImpl::Plain(f), min: $min, max: natives!(@max $max) });
        } )*
    };
    (@max _) => { None };
    (@max $m:literal) => { Some($m) };
}

pub fn install(vm: &mut Vm) {
    natives! { vm;
        "%make-rtd" 2 3 => make_rtd;
        "%parse-args" 5 5 => parse_args;
        "%describe" 1 1 => |vm: &mut Vm, a, _| {
            let v = arg(vm, a, 0);
            let s = if v.is_symbol() {
                vm.describe_binding(vm.current_module(), v.as_symbol())
            } else {
                let name = vm.procedure_name(v).unwrap_or_else(|| "value".into());
                vm.describe_value(&name, v)
            };
            Ok(vm.make_string(s.as_bytes())) };
        "documentation" 1 1 => |vm: &mut Vm, a, _| {
            let v = arg(vm, a, 0);
            Ok(match vm.documentation(v) { Some(d) => vm.make_string(d.as_bytes()), None => Value::FALSE }) };
        "%type-key" 1 1 => |vm: &mut Vm, a, _| Ok(type_key(vm, arg(vm, a, 0)));
        "%type-parent" 1 1 => |vm: &mut Vm, a, _| Ok(type_parent(vm, arg(vm, a, 0)));
        "%type-designator" 2 2 => type_designator;
        "type-of" 1 1 => type_of;
        "%unset?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(arg(vm, a, 0) == Value::UNSET));
        "%missing-keyword" 2 2 => |vm: &mut Vm, a, _| Err(Error::new(format!("{}: missing required keyword argument {}", repr(arg(vm, a, 0)), repr(arg(vm, a, 1)))));
        "%match-error" 1 1 => |vm: &mut Vm, a, _| { let v = arg(vm, a, 0); let obj = vm.make_error_object("match: no clause matches", &[v]); Err(vm.raise_error(obj)) };
        "keyword?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(arg(vm, a, 0).is_keyword()));
        "keyword->string" 1 1 => |vm: &mut Vm, a, _| { let k = arg(vm, a, 0); if !k.is_keyword() { return Err(type_error("keyword->string", "keyword", k)) } let s = symbol_name(k.as_keyword()); Ok(vm.make_string(s.as_bytes())) };
        "string->keyword" 1 1 => |vm: &mut Vm, a, _| { let s = string(vm, arg(vm, a, 0), "string->keyword")?; Ok(Value::keyword(reader::intern(&s))) };
        "%record" 1 _ => record;
        "%record?" 2 2 => record_p;
        "%record-ref" 3 3 => record_ref;
        "%record-set!" 4 4 => record_set;
        "record?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(is_kind(arg(vm, a, 0), Kind::Record)));
        // A record's fields as a list of (name . value), for inspectors.
        "record-fields" 1 1 => |vm: &mut Vm, a, _| {
            let r = arg(vm, a, 0);
            if !is_kind(r, Kind::Record) {
                return Err(type_error("record-fields", "record", r));
            }
            let names = list_values(unsafe { field(field(r.as_ptr(), 0).as_ptr(), 1) }).unwrap_or_default();
            let mark = vm.scratch.len();
            // Field names are symbols; the record is read again after each
            // allocation, which may move it.
            for (i, name) in names.into_iter().enumerate() {
                let value = unsafe { field(arg(vm, a, 0).as_ptr(), 1 + i) };
                let pair = vm.alloc_pair(name, value);
                vm.scratch.push(pair);
            }
            let items: Vec<Value> = vm.scratch[mark..].to_vec();
            let list = vm.make_list(&items);
            vm.scratch.truncate(mark);
            Ok(list) };

        "raise" 1 1 => raise;
        "raise-continuable" 1 1 => |vm: &mut Vm, a, _| { let v = arg(vm, a, 0); vm.raise_continuable(v) };
        "error-object?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(error_object_parts(vm, arg(vm, a, 0)).is_some()));
        "file-error?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(error_kind(vm, arg(vm, a, 0)) == Some("file")));
        "read-error?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(error_kind(vm, arg(vm, a, 0)) == Some("read")));
        "error-object-message" 1 1 => error_object_message;
        "error-object-irritants" 1 1 => error_object_irritants;
        "condition/report-string" 1 1 => |vm: &mut Vm, a, _| {
            let s = crate::builtins::condition_message(vm, arg(vm, a, 0)); Ok(vm.make_string(s.as_bytes())) };
        "%push-handler" 1 1 => |vm: &mut Vm, a, _| { let h = vm.root(arg(vm, a, 0)); vm.push_proc_handler(h); Ok(Value::VOID) };
        "%push-wind" 1 1 => |vm: &mut Vm, a, _| { let after = vm.root(arg(vm, a, 0)); vm.push_wind(after); Ok(Value::VOID) };
        "%pop-handler" 0 0 => |vm: &mut Vm, _, _| { vm.pop_handler(); Ok(Value::VOID) };
        "%fresh-key" 0 0 => |vm: &mut Vm, _, _| Ok(Value::int_unchecked(vm.fresh_id()));
        "%task-local-ref" 2 2 => |vm: &mut Vm, a, _| {
            let key = arg(vm, a, 0).as_int();
            Ok(vm.locals.get(&key).map_or(arg(vm, a, 1), |r| r.get())) };
        "%task-local-set!" 2 2 => |vm: &mut Vm, a, _| {
            let key = arg(vm, a, 0).as_int(); let v = vm.root(arg(vm, a, 1)); vm.locals.insert(key, v); Ok(Value::VOID) };
        "values" 0 _ => values;
        "%values?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(record_check(arg(vm, a, 0), vm.special(SpecialObj::ValuesRtd))));
        "%values->list" 1 1 => |vm: &mut Vm, a, _| {
            let v = arg(vm, a, 0);
            let items: Vec<Value> = (0..unsafe { len_of(v.as_ptr()) } - 1).map(|i| unsafe { field(v.as_ptr(), 1 + i) }).collect();
            Ok(vm.make_list(&items)) };
        "apply" 2 _ => apply;
        "eval" 1 2 => eval;
        "in-module" 1 1 => |vm: &mut Vm, a, _| { let m = module_arg(vm, arg(vm, a, 0), "in-module")?; vm.set_current_module(m); Ok(Value::VOID) };
        "current-module" 0 0 => |vm: &mut Vm, _, _| { let name = vm.module_name(vm.current_module()); Ok(vm.make_string(name.as_bytes())) };


        "string-split" 1 2 => string_split;
        "string-join" 1 2 => string_join;
        "string-contains" 2 2 => string_contains;
        "string-index" 2 2 => string_index;
        "string-replace" 3 3 => string_replace;
        "string-upcase" 1 1 => |vm: &mut Vm, a, _| str_fn(vm, a, "string-upcase", str::to_uppercase);
        "string-downcase" 1 1 => |vm: &mut Vm, a, _| str_fn(vm, a, "string-downcase", str::to_lowercase);
        "string-trim" 1 1 => |vm: &mut Vm, a, _| str_fn(vm, a, "string-trim", |s| s.trim().to_owned());
        "string-trim-left" 1 1 => |vm: &mut Vm, a, _| str_fn(vm, a, "string-trim-left", |s| s.trim_start().to_owned());
        "string-trim-right" 1 1 => |vm: &mut Vm, a, _| str_fn(vm, a, "string-trim-right", |s| s.trim_end().to_owned());
        "string-suffix?" 2 2 => |vm: &mut Vm, a, _| { let x = string(vm, arg(vm, a, 0), "string-suffix?")?; let s = string(vm, arg(vm, a, 1), "string-suffix?")?; Ok(Value::bool(s.ends_with(&x))) };
        "string" 0 _ => string_from_chars;
        "string>?" 1 _ => |vm: &mut Vm, a, n| string_cmp(vm, a, n, "string>?", false, |o| o.is_gt());
        "string<=?" 1 _ => |vm: &mut Vm, a, n| string_cmp(vm, a, n, "string<=?", false, |o| o.is_le());
        "string>=?" 1 _ => |vm: &mut Vm, a, n| string_cmp(vm, a, n, "string>=?", false, |o| o.is_ge());
        "string-ci=?" 1 _ => |vm: &mut Vm, a, n| string_cmp(vm, a, n, "string-ci=?", true, |o| o.is_eq());
        "string-ci<?" 1 _ => |vm: &mut Vm, a, n| string_cmp(vm, a, n, "string-ci<?", true, |o| o.is_lt());
        "string-ci>?" 1 _ => |vm: &mut Vm, a, n| string_cmp(vm, a, n, "string-ci>?", true, |o| o.is_gt());
        "string-ci<=?" 1 _ => |vm: &mut Vm, a, n| string_cmp(vm, a, n, "string-ci<=?", true, |o| o.is_le());
        "string-ci>=?" 1 _ => |vm: &mut Vm, a, n| string_cmp(vm, a, n, "string-ci>=?", true, |o| o.is_ge());
        "char-upcase" 1 1 => |vm: &mut Vm, a, _| { let c: char = vm.get(arg(vm, a, 0))?; Ok(Value::char(c.to_uppercase().next().unwrap_or(c))) };
        "char-downcase" 1 1 => |vm: &mut Vm, a, _| { let c: char = vm.get(arg(vm, a, 0))?; Ok(Value::char(c.to_lowercase().next().unwrap_or(c))) };
        "char-upper-case?" 1 1 => |vm: &mut Vm, a, _| { let c: char = vm.get(arg(vm, a, 0))?; Ok(Value::bool(c.is_uppercase())) };
        "char-lower-case?" 1 1 => |vm: &mut Vm, a, _| { let c: char = vm.get(arg(vm, a, 0))?; Ok(Value::bool(c.is_lowercase())) };

        "vector-copy" 1 3 => vector_copy;
        "hash-table-keys" 1 1 => hash_keys;
        "hash-table-values" 1 1 => hash_values;
        "hash-table->alist" 1 1 => hash_to_alist;
        "hash-table?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(is_kind(arg(vm, a, 0), Kind::Table)));

        "current-milliseconds" 0 0 => current_ms;
        "current-second" 0 0 => |_: &mut Vm, _, _| {
            Ok(Value::float(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64()))) };
        "current-jiffy" 0 0 => |vm: &mut Vm, _, _| Ok(vm.make_int(jiffy_epoch().elapsed().as_micros() as i64));
        "jiffies-per-second" 0 0 => |_: &mut Vm, _, _| Ok(Value::int_unchecked(1_000_000));
        "features" 0 0 => |vm: &mut Vm, _, _| {
            let syms: Vec<Value> = crate::library::FEATURES.iter().map(|n| Value::symbol(reader::intern(n))).collect();
            Ok(vm.make_list(&syms)) };
        "%package-stage" 2 2 => |vm: &mut Vm, a, _| {
            let path = string(vm, arg(vm, a, 0), "load-package")?;
            let generation = crate::num::integer(arg(vm, a, 1), "load-package")? as u32;
            let m = vm.stage_package(std::path::Path::new(&path), generation)?;
            let name = vm.module_name(m);
            Ok(vm.make_string(name.as_bytes())) };
        "%package-publish" 0 0 => |vm: &mut Vm, _, _| { vm.publish_staged(); Ok(Value::VOID) };
        "%package-discard" 0 0 => |vm: &mut Vm, _, _| { vm.discard_staged(); Ok(Value::VOID) };
        // Source text of a file, evaluated in a module: definitions remember
        // the file (and the line and column the text is padded to).
        "eval-source" 3 3 => |vm: &mut Vm, a, _| {
            let source = string(vm, arg(vm, a, 0), "eval-source")?;
            let module = module_arg(vm, arg(vm, a, 1), "eval-source")?;
            let file = string(vm, arg(vm, a, 2), "eval-source")?;
            vm.eval_in(module, &file, &source) };
        // The names a module sees, for completion: a list of (name kind),
        // the name a string, the kind `syntax`, `macro`, `procedure` or
        // `variable`. A module not loaded yet is not loaded (completing
        // must not run a file): the user module's names are offered.
        "module-completions" 1 1 => |vm: &mut Vm, a, _| {
            let name = string(vm, arg(vm, a, 0), "module-completions")?;
            let module = vm.loaded_module(&name).unwrap_or(crate::vm::USER_MODULE);
            let items: Vec<_> = vm
                .completions(module)
                .into_iter()
                .map(|(name, kind)| {
                    let name = vm.make_string(name.as_bytes());
                    let item = vm.make_list(&[name, Value::symbol(reader::intern(kind.name()))]);
                    vm.root(item)
                })
                .collect();
            let values: Vec<Value> = items.iter().map(|r| r.get()).collect();
            Ok(vm.make_list(&values)) };
        "procedure-location" 1 1 => |vm: &mut Vm, a, _| {
            let v = arg(vm, a, 0);
            match vm.procedure_info(v).filter(|i| i.file.is_some() && i.line > 0) {
                Some(i) => {
                    let (file, line, column) = (i.file.unwrap_or_default().to_string(), i.line as i64, i.column as i64);
                    let file = vm.make_string(file.as_bytes());
                    let file = vm.root(file);
                    let items = [file.get(), Value::int_unchecked(line), Value::int_unchecked(column)];
                    Ok(vm.make_list(&items))
                }
                None => Ok(Value::FALSE),
            } };
        // Name the current module as a library, e.g. `(techne editor)`, for
        // a host to give its own interface a library name.
        "%name-library" 1 1 => |vm: &mut Vm, a, _| {
            let name = crate::builtins::repr(arg(vm, a, 0));
            let m = vm.current_module();
            vm.modules[m as usize].name = name.into();
            Ok(Value::VOID) };
        "%environment" 1 1 => |vm: &mut Vm, a, _| {
            let sets = list_values(arg(vm, a, 0)).ok_or_else(|| Error::new("environment: expected import sets"))?;
            let sets = sets.into_iter().map(value_to_sexp).collect::<Result<Vec<_>, _>>()?;
            let m = vm.environment(&sets)?;
            let name = vm.module_name(m);
            Ok(vm.make_string(name.as_bytes())) };
        "exact?" 1 1 => |vm: &mut Vm, a, _| { let v = arg(vm, a, 0); crate::num::num(v, "exact?")?; Ok(Value::bool(crate::num::is_exact(v))) };
        "exact-integer?" 1 1 => |vm: &mut Vm, a, _| { let v = arg(vm, a, 0); Ok(Value::bool(v.is_int() || is_kind(v, Kind::BigInt))) };
        "gensym" 0 1 => |vm: &mut Vm, _, _| { let id = vm.fresh_id(); Ok(Value::symbol(reader::intern(&format!(" g{id}")))) };
        "repr" 1 1 => |vm: &mut Vm, a, _| { let s = repr(arg(vm, a, 0)); Ok(vm.make_string(s.as_bytes())) };
    }
    vm.requiring(Capability::Files, |vm| {
        natives! { vm;
            "file->string" 1 1 => file_to_string;
            "delete-file" 1 1 => |vm: &mut Vm, a, _| {
                let path = string(vm, arg(vm, a, 0), "delete-file")?;
                std::fs::remove_file(&path).map_err(|e| Error::new(format!("{path}: {e}")).with_kind(crate::vm::ErrorKind::File))?;
                Ok(Value::VOID) };
            "file-exists?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(std::path::Path::new(&string(vm, arg(vm, a, 0), "file-exists?")?).exists()));
        }
    });
    vm.requiring(Capability::Environment, |vm| {
        natives! { vm;
            "command-line" 0 0 => |vm: &mut Vm, _, _| { let args: Vec<String> = std::env::args().skip(1).collect(); vm.to_value(args) };
            "get-environment-variable" 1 1 => |vm: &mut Vm, a, _| { let k = string(vm, arg(vm, a, 0), "get-environment-variable")?; vm.to_value(std::env::var(k).ok()) };
            "%environment-variables" 0 0 => |vm: &mut Vm, _, _| {
                let flat: Vec<String> = std::env::vars().flat_map(|(k, v)| [k, v]).collect();
                vm.to_value(flat) };
            "getenv" 1 1 => |vm: &mut Vm, a, _| { let k = string(vm, arg(vm, a, 0), "getenv")?; vm.to_value(std::env::var(k).ok()) };
        }
    });
}
