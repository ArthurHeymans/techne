//! Natives beyond the core: records, conditions, control, ports, strings,
//! hash-table views and system access.

use std::{
    cell::RefCell,
    fs::File,
    io::{BufWriter, Write},
};

use crate::{
    api::Foreign,
    builtins::{Bulk, error_object_parts, list_values, print, repr, type_error},
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
            crate::num::N::F(_) => unreachable!(),
        });
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
    "float",
    "string",
    "symbol",
    "keyword",
    "char",
    "list",
    "pair",
    "null",
    "vector",
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
        "integer" | "float" => "number",
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

// ----- ports -----
//
// Ports are textual (bytevector ports come with language step 11). An input
// file is read whole into a string port; standard input is read a line at a
// time into one buffer that every standard-input port shares. Closing a port
// keeps it, closed, so that the predicates can still answer for it.

pub enum Port {
    StringOut(String),
    /// Output handed to a Rust function as it is written.
    Sink(Box<dyn FnMut(&str)>),
    StringIn {
        text: String,
        pos: usize,
    },
    Stdin,
    FileOut(BufWriter<File>),
    Stdout,
    Stderr,
    Closed {
        input: bool,
    },
}

impl Port {
    fn is_input(&self) -> bool {
        matches!(self, Port::StringIn { .. } | Port::Stdin | Port::Closed { input: true })
    }
    fn is_open(&self) -> bool {
        !matches!(self, Port::Closed { .. })
    }
}

type PortRef = Foreign<RefCell<Port>>;

fn port_arg(vm: &mut Vm, v: Value) -> Result<PortRef, Error> {
    vm.get::<PortRef>(v).map_err(|_| type_error("port operation", "port", v))
}

fn make_port(vm: &mut Vm, p: Port) -> R {
    vm.to_value(Foreign::new(RefCell::new(p)))
}

/// An output port that passes what is written to `f` (e.g. to stream a
/// REPL's output to its client).
pub fn make_output_port(vm: &mut Vm, f: impl FnMut(&str) + 'static) -> Result<Value, Error> {
    make_port(vm, Port::Sink(Box::new(f)))
}

/// Task-local keys of the `current-output-port`, `current-error-port` and
/// `current-input-port` parameters; unbound means the standard stream.
pub const OUTPUT_PORT_KEY: i64 = -1;
const ERROR_PORT_KEY: i64 = -2;
const INPUT_PORT_KEY: i64 = -3;

/// Write text to `port` (a port value) or to the current output.
pub fn write_out(vm: &mut Vm, port: Option<Value>, text: &str) -> Result<(), Error> {
    let target = port.or_else(|| vm.locals.get(&OUTPUT_PORT_KEY).map(|r| r.get()).filter(|v| v.is_truthy()));
    let Some(p) = target else {
        let _ = vm.out.write_all(text.as_bytes());
        return Ok(());
    };
    let port = port_arg(vm, p)?;
    match &mut *port.borrow_mut() {
        Port::StringOut(s) => s.push_str(text),
        Port::Sink(f) => f(text),
        Port::FileOut(w) => w.write_all(text.as_bytes()).map_err(|e| Error::new(e.to_string()))?,
        Port::Stdout => {
            let _ = vm.out.write_all(text.as_bytes());
        }
        Port::Stderr => eprint!("{text}"),
        Port::Closed { input: false } => return Err(Error::new("write: the port is closed")),
        _ => return Err(Error::new("not an output port")),
    }
    Ok(())
}

/// How much unread text an operation needs, so that standard input is read
/// only as far as necessary.
#[derive(Clone, Copy)]
enum Need {
    Chars(usize),
    Line,
    Datum,
    All,
}

thread_local! {
    static STDIN: RefCell<(String, usize)> = const { RefCell::new((String::new(), 0)) };
}

/// Read lines from standard input into `buf` until `need` is met or input
/// ends.
fn fill_stdin(buf: &mut (String, usize), need: Need) {
    loop {
        let rest = &buf.0[buf.1..];
        let met = match need {
            Need::Chars(k) => rest.chars().take(k).count() == k,
            Need::Line => rest.contains('\n'),
            Need::Datum => {
                let t = rest.trim_start();
                !t.is_empty() && reader::read_one(rest).is_ok()
            }
            Need::All => false,
        };
        if met {
            return;
        }
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => buf.0.push_str(&line),
        }
    }
}

/// Run `f` on the text and position of the input port at argument `i`, or
/// of the current input port.
fn with_input<T>(
    vm: &mut Vm,
    args: usize,
    n: usize,
    i: usize,
    need: Need,
    f: impl FnOnce(&mut String, &mut usize) -> T,
) -> Result<T, Error> {
    let port = if n > i { Some(arg(vm, args, i)) } else { vm.locals.get(&INPUT_PORT_KEY).map(|r| r.get()) };
    let Some(p) = port else {
        return Ok(STDIN.with(|s| {
            let mut s = s.borrow_mut();
            fill_stdin(&mut s, need);
            let (text, pos) = &mut *s;
            f(text, pos)
        }));
    };
    let p = port_arg(vm, p)?;
    let mut port = p.borrow_mut();
    match &mut *port {
        Port::StringIn { text, pos } => Ok(f(text, pos)),
        Port::Stdin => Ok(STDIN.with(|s| {
            let mut s = s.borrow_mut();
            fill_stdin(&mut s, need);
            let (text, pos) = &mut *s;
            f(text, pos)
        })),
        Port::Closed { input: true } => Err(Error::new("read: the port is closed")),
        _ => Err(Error::new("not an input port")),
    }
}

fn read_line(vm: &mut Vm, args: usize, n: usize) -> R {
    let line = with_input(vm, args, n, 0, Need::Line, |text, pos| {
        if *pos >= text.len() {
            return None;
        }
        let rest = &text[*pos..];
        let (line, used) = match rest.find('\n') {
            Some(i) => (rest[..i].strip_suffix('\r').unwrap_or(&rest[..i]), i + 1),
            None => (rest, rest.len()),
        };
        let line = line.to_owned();
        *pos += used;
        Some(line)
    })?;
    Ok(line.map_or(Value::EOF, |l| vm.make_string(l.as_bytes())))
}

fn read_char_impl(vm: &mut Vm, args: usize, n: usize, consume: bool) -> R {
    let c = with_input(vm, args, n, 0, Need::Chars(1), |text, pos| {
        let c = text[*pos..].chars().next();
        if consume && let Some(c) = c {
            *pos += c.len_utf8();
        }
        c
    })?;
    Ok(c.map_or(Value::EOF, Value::char))
}

/// `(read-string k [port])`: up to `k` characters, or the end-of-file
/// object when there are none.
fn read_string(vm: &mut Vm, args: usize, n: usize) -> R {
    let k = crate::num::integer(arg(vm, args, 0), "read-string")?.max(0) as usize;
    let s = with_input(vm, args, n, 1, Need::Chars(k), |text, pos| {
        let rest = &text[*pos..];
        let end = rest.char_indices().nth(k).map_or(rest.len(), |(i, _)| i);
        let s = rest[..end].to_owned();
        *pos += end;
        s
    })?;
    if s.is_empty() && k > 0 { Ok(Value::EOF) } else { Ok(vm.make_string(s.as_bytes())) }
}

fn read_all(vm: &mut Vm, args: usize, n: usize) -> R {
    let s = with_input(vm, args, n, 0, Need::All, |text, pos| {
        let s = text[*pos..].to_owned();
        *pos = text.len();
        s
    })?;
    Ok(vm.make_string(s.as_bytes()))
}

fn read_datum(vm: &mut Vm, args: usize, n: usize) -> R {
    let items = with_input(vm, args, n, 0, Need::Datum, |text, pos| -> Result<Option<Vec<reader::Sexp>>, String> {
        let next = |pos: &mut usize| -> Result<Option<reader::Sexp>, String> {
            let rest = &text[*pos..];
            if rest.trim_start().is_empty() {
                *pos = text.len();
                return Ok(None);
            }
            let (datum, used) = reader::read_one(rest)?;
            *pos += used;
            Ok(Some(datum))
        };
        Ok(match next(pos)? {
            // A label before the datum itself: #0=(a . #0#), unless the
            // datum came with it (#0=a).
            Some(d) if matches!(label(&d), Some(Label::Def(_, ref rest)) if rest.is_empty() || rest == "#") => {
                let datum = next(pos)?.ok_or("read: a datum label without a datum")?;
                Some(vec![d, datum])
            }
            d => d.map(|d| vec![d]),
        })
    })?;
    let items = match items {
        Ok(items) => items,
        Err(msg) => return Err(kind_error(vm, &msg, "read")),
    };
    let Some(items) = items else { return Ok(Value::EOF) };
    let labelled = items.iter().any(has_label);
    let marked = mark_seq(&items);
    let v = vm.constant(&marked[0]);
    if labelled { resolve_labels(vm, v) } else { Ok(v) }
}

/// An error that `file-error?` or `read-error?` recognises.
pub fn kind_error(vm: &mut Vm, message: &str, kind: &str) -> Error {
    let obj = vm.make_error_object_of_kind(message, &[], kind);
    vm.raise_error(obj)
}

// Datum labels (R7RS 2.4): `#n=` before a datum names it and `#n#` refers to
// it, so read can give back the cycles write prints. The reader returns the
// labels as symbols; they become marker lists, and after the value is built
// the markers are replaced by what they name.

const LABEL: &str = "\u{1f}datum-label";
const REF: &str = "\u{1f}datum-ref";

/// A datum label as the reader returns it: a symbol. A definition may carry
/// what follows `=` without a delimiter (`#0=a`, or `#` for a vector).
enum Label {
    Def(i64, String),
    Ref(i64),
}

fn label(s: &reader::Sexp) -> Option<Label> {
    let name = symbol_name(s.sym()?);
    let rest = name.strip_prefix('#')?;
    let digits = rest.find(|c: char| !c.is_ascii_digit()).filter(|&i| i > 0)?;
    let n = rest[..digits].parse().ok()?;
    match &rest[digits..] {
        "#" => Some(Label::Ref(n)),
        r => r.strip_prefix('=').map(|after| Label::Def(n, after.to_string())),
    }
}

fn has_label(s: &reader::Sexp) -> bool {
    match s {
        reader::Sexp::List(items, tail, _) => items.iter().any(has_label) || tail.as_deref().is_some_and(has_label),
        reader::Sexp::Vector(items) => items.iter().any(has_label),
        s => label(s).is_some(),
    }
}

/// A sequence of data with labels as markers: `#n= datum` becomes one
/// `(LABEL n datum)` item, `#n#` a `(REF n)` one.
fn mark_seq(items: &[reader::Sexp]) -> Vec<reader::Sexp> {
    use reader::Sexp;
    let marker = |name: &str, n: i64, rest: Vec<Sexp>| {
        let mut items = vec![Sexp::Sym(reader::intern(name)), Sexp::Int(n)];
        items.extend(rest);
        Sexp::List(items, None, reader::NO_POS)
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < items.len() {
        match label(&items[i]) {
            Some(Label::Def(n, rest)) if !rest.is_empty() && rest != "#" => {
                let datum = reader::read_one(&rest).map(|(d, _)| d).unwrap_or_else(|_| Sexp::Sym(reader::intern(&rest)));
                out.push(marker(LABEL, n, mark_seq(&[datum])));
                i += 1;
            }
            Some(Label::Def(n, rest)) if i + 1 < items.len() => {
                let datum = match (&items[i + 1], rest.as_str()) {
                    (Sexp::List(xs, None, _), "#") => Sexp::Vector(xs.clone()),
                    (d, _) => d.clone(),
                };
                out.push(marker(LABEL, n, mark_seq(&[datum])));
                i += 2;
            }
            Some(Label::Ref(n)) => {
                out.push(marker(REF, n, vec![]));
                i += 1;
            }
            _ => {
                out.push(match &items[i] {
                    Sexp::List(xs, tail, pos) => {
                        let tail = tail.as_deref().map(|t| Box::new(mark_seq(std::slice::from_ref(t)).remove(0)));
                        Sexp::List(mark_seq(xs), tail, *pos)
                    }
                    Sexp::Vector(xs) => Sexp::Vector(mark_seq(xs)),
                    other => other.clone(),
                });
                i += 1;
            }
        }
    }
    out
}

/// Replace the markers in a freshly built value by what they name.
fn resolve_labels(vm: &mut Vm, v: Value) -> R {
    let label = Value::symbol(reader::intern(LABEL));
    let reference = Value::symbol(reader::intern(REF));
    let mut labels: rustc_hash::FxHashMap<i64, Value> = Default::default();
    // What a marker stands for; a label is recorded first, so references
    // inside its datum find it.
    let resolve = |labels: &mut rustc_hash::FxHashMap<i64, Value>, x: Value| -> Result<(Value, bool), Error> {
        if !is_kind(x, Kind::Pair) {
            return Ok((x, true));
        }
        let head = unsafe { field(x.as_ptr(), 0) };
        let rest = unsafe { field(x.as_ptr(), 1) };
        if head != label && head != reference {
            return Ok((x, true));
        }
        let n = unsafe { field(rest.as_ptr(), 0) }.as_int();
        if head == label {
            let datum = unsafe { field(field(rest.as_ptr(), 1).as_ptr(), 0) };
            labels.insert(n, datum);
            Ok((datum, true))
        } else {
            let target = labels.get(&n).copied().ok_or_else(|| Error::new(format!("read: #{n}# before #{n}=")))?;
            Ok((target, false))
        }
    };
    let (root, _) = resolve(&mut labels, v)?;
    let mut work = vec![root];
    while let Some(o) = work.pop() {
        let (p, slots) = if is_kind(o, Kind::Pair) {
            (o.as_ptr(), 2)
        } else if is_kind(o, Kind::Vector) {
            (o.as_ptr(), unsafe { len_of(o.as_ptr()) })
        } else {
            continue;
        };
        for i in 0..slots {
            let (x, descend) = resolve(&mut labels, unsafe { field(p, i) })?;
            unsafe { set_field(p, i, x) };
            vm.write_barrier(p, x);
            if descend {
                work.push(x);
            }
        }
    }
    Ok(root)
}

fn get_output_string(vm: &mut Vm, args: usize, _: usize) -> R {
    let p = port_arg(vm, arg(vm, args, 0))?;
    let text = match &*p.borrow() {
        Port::StringOut(s) => s.clone(),
        _ => return Err(Error::new("get-output-string: not a string output port")),
    };
    Ok(vm.make_string(text.as_bytes()))
}

fn close_port(vm: &mut Vm, args: usize, _: usize) -> R {
    let p = port_arg(vm, arg(vm, args, 0))?;
    let mut port = p.borrow_mut();
    if let Port::FileOut(w) = &mut *port {
        w.flush().map_err(|e| Error::new(e.to_string()))?;
    }
    if port.is_open() {
        *port = Port::Closed { input: port.is_input() };
    }
    Ok(Value::VOID)
}

fn port_check(vm: &mut Vm, args: usize, test: fn(&Port) -> bool) -> R {
    let v = arg(vm, args, 0);
    Ok(Value::bool(vm.get::<PortRef>(v).is_ok_and(|p| test(&p.borrow()))))
}

/// `(write-string s [port [start [end]]])`, start and end in characters.
fn write_string(vm: &mut Vm, args: usize, n: usize) -> R {
    let s = string(vm, arg(vm, args, 0), "write-string")?;
    let index = |i: usize, default: usize| -> Result<usize, Error> {
        if n > i { Ok(crate::num::integer(arg(vm, args, i), "write-string")?.max(0) as usize) } else { Ok(default) }
    };
    let count = s.chars().count();
    let (start, end) = (index(2, 0)?, index(3, count)?.min(count));
    let text: String = s.chars().skip(start).take(end.saturating_sub(start)).collect();
    let port = (n > 1).then(|| arg(vm, args, 1));
    write_out(vm, port, &text)?;
    Ok(Value::VOID)
}

fn flush_output(vm: &mut Vm, args: usize, n: usize) -> R {
    if n > 0 {
        let p = port_arg(vm, arg(vm, args, 0))?;
        if let Port::FileOut(w) = &mut *p.borrow_mut() {
            w.flush().map_err(|e| Error::new(e.to_string()))?;
        }
    }
    vm.flush();
    Ok(Value::VOID)
}

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
    let get = |vm: &Vm, i| string(vm, arg(vm, args, i), who).map(|s| if ci { s.to_lowercase() } else { s });
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

fn vector_fill(vm: &mut Vm, args: usize, _: usize) -> R {
    let (v, x) = (arg(vm, args, 0), arg(vm, args, 1));
    if !is_kind(v, Kind::Vector) {
        return Err(type_error("vector-fill!", "vector", v));
    }
    for i in 0..unsafe { len_of(v.as_ptr()) } {
        unsafe { set_field(v.as_ptr(), i, x) };
    }
    vm.write_barrier(v.as_ptr(), x);
    Ok(Value::VOID)
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

fn current_ms(_: &mut Vm, _: usize, _: usize) -> R {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    Ok(Value::int_unchecked(ms as i64))
}

fn file_to_string(vm: &mut Vm, args: usize, _: usize) -> R {
    let path = string(vm, arg(vm, args, 0), "file->string")?;
    let text = std::fs::read_to_string(&path).map_err(|e| Error::new(format!("{path}: {e}")))?;
    Ok(vm.make_string(text.as_bytes()))
}

fn open_input_file(vm: &mut Vm, args: usize, _: usize) -> R {
    let path = string(vm, arg(vm, args, 0), "open-input-file")?;
    match std::fs::read(&path) {
        Ok(bytes) => make_port(vm, Port::StringIn { text: String::from_utf8_lossy(&bytes).into_owned(), pos: 0 }),
        Err(e) => Err(kind_error(vm, &format!("{path}: {e}"), "file")),
    }
}

fn open_output_file(vm: &mut Vm, args: usize, _: usize) -> R {
    let path = string(vm, arg(vm, args, 0), "open-output-file")?;
    match File::create(&path) {
        Ok(f) => make_port(vm, Port::FileOut(BufWriter::new(f))),
        Err(e) => Err(kind_error(vm, &format!("{path}: {e}"), "file")),
    }
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

        "raise" 1 1 => raise;
        "raise-continuable" 1 1 => |vm: &mut Vm, a, _| { let v = arg(vm, a, 0); vm.raise_continuable(v) };
        "error-object?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(error_object_parts(vm, arg(vm, a, 0)).is_some()));
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
        "%output-port-key" 0 0 => |_: &mut Vm, _, _| Ok(Value::int_unchecked(OUTPUT_PORT_KEY));
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

        "open-output-string" 0 0 => |vm: &mut Vm, _, _| make_port(vm, Port::StringOut(String::new()));
        "open-input-string" 1 1 => |vm: &mut Vm, a, _| { let s = string(vm, arg(vm, a, 0), "open-input-string")?; make_port(vm, Port::StringIn { text: s, pos: 0 }) };
        "get-output-string" 1 1 => get_output_string;
        "close-port" 1 1 => close_port;
        "close-input-port" 1 1 => close_port;
        "close-output-port" 1 1 => close_port;
        "read-line" 0 1 => read_line;
        "read-char" 0 1 => |vm: &mut Vm, a, n| read_char_impl(vm, a, n, true);
        "peek-char" 0 1 => |vm: &mut Vm, a, n| read_char_impl(vm, a, n, false);
        "read" 0 1 => read_datum;
        "read-string-all" 0 1 => read_all;
        "read-string" 1 2 => read_string;
        "char-ready?" 0 1 => |_: &mut Vm, _, _| Ok(Value::TRUE);
        "write-string" 1 4 => write_string;
        "port?" 1 1 => |vm: &mut Vm, a, _| port_check(vm, a, |_| true);
        "textual-port?" 1 1 => |vm: &mut Vm, a, _| port_check(vm, a, |_| true);
        "binary-port?" 1 1 => |vm: &mut Vm, a, _| port_check(vm, a, |_| false);
        "input-port?" 1 1 => |vm: &mut Vm, a, _| port_check(vm, a, Port::is_input);
        "output-port?" 1 1 => |vm: &mut Vm, a, _| port_check(vm, a, |p| !p.is_input());
        "input-port-open?" 1 1 => |vm: &mut Vm, a, _| port_check(vm, a, |p| p.is_input() && p.is_open());
        "output-port-open?" 1 1 => |vm: &mut Vm, a, _| port_check(vm, a, |p| !p.is_input() && p.is_open());
        "%standard-port" 1 1 => |vm: &mut Vm, a, _| {
            let which = symbol_name(arg(vm, a, 0).as_symbol());
            make_port(vm, match &*which { "input" => Port::Stdin, "error" => Port::Stderr, _ => Port::Stdout }) };
        "%error-port-key" 0 0 => |_: &mut Vm, _, _| Ok(Value::int_unchecked(ERROR_PORT_KEY));
        "%input-port-key" 0 0 => |_: &mut Vm, _, _| Ok(Value::int_unchecked(INPUT_PORT_KEY));
        "file-error?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(vm.error_object_kind(arg(vm, a, 0)).as_deref() == Some("file")));
        "read-error?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(vm.error_object_kind(arg(vm, a, 0)).as_deref() == Some("read")));
        "write-char" 1 2 => |vm: &mut Vm, a, n| { let c = arg(vm, a, 0); let s = repr_char(c)?; let p = (n > 1).then(|| arg(vm, a, 1)); write_out(vm, p, &s)?; Ok(Value::VOID) };
        "eof-object" 0 0 => |_: &mut Vm, _, _| Ok(Value::EOF);
        "eof-object?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(arg(vm, a, 0) == Value::EOF));
        "flush-output" 0 1 => flush_output;
        "flush-output-port" 0 1 => flush_output;
        "write-shared" 1 2 => |vm: &mut Vm, a, n| {
            let mut out = String::new();
            crate::builtins::print_shared(&mut out, arg(vm, a, 0));
            let p = (n > 1).then(|| arg(vm, a, 1));
            write_out(vm, p, &out)?;
            Ok(Value::VOID) };

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
        "vector-fill!" 2 2 => vector_fill;
        "hash-table-keys" 1 1 => hash_keys;
        "hash-table-values" 1 1 => hash_values;
        "hash-table->alist" 1 1 => hash_to_alist;
        "hash-table?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(is_kind(arg(vm, a, 0), Kind::Table)));

        "current-milliseconds" 0 0 => current_ms;
        "exact?" 1 1 => |vm: &mut Vm, a, _| { let v = arg(vm, a, 0); Ok(Value::bool(v.is_int() || is_kind(v, Kind::BigInt))) };
        "inexact?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(arg(vm, a, 0).is_float()));
        "exact-integer?" 1 1 => |vm: &mut Vm, a, _| { let v = arg(vm, a, 0); Ok(Value::bool(v.is_int() || is_kind(v, Kind::BigInt))) };
        "nan?" 1 1 => |vm: &mut Vm, a, _| { let v = arg(vm, a, 0); Ok(Value::bool(v.is_float() && v.as_float().is_nan())) };
        "gensym" 0 1 => |vm: &mut Vm, _, _| { let id = vm.fresh_id(); Ok(Value::symbol(reader::intern(&format!(" g{id}")))) };
        "repr" 1 1 => |vm: &mut Vm, a, _| { let s = repr(arg(vm, a, 0)); Ok(vm.make_string(s.as_bytes())) };
    }
    vm.requiring(Capability::Files, |vm| {
        natives! { vm;
            "open-input-file" 1 1 => open_input_file;
            "open-output-file" 1 1 => open_output_file;
            "file->string" 1 1 => file_to_string;
            "file-exists?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(std::path::Path::new(&string(vm, arg(vm, a, 0), "file-exists?")?).exists()));
            "delete-file" 1 1 => |vm: &mut Vm, a, _| {
                let path = string(vm, arg(vm, a, 0), "delete-file")?;
                match std::fs::remove_file(&path) {
                    Ok(()) => Ok(Value::VOID),
                    Err(e) => Err(kind_error(vm, &format!("{path}: {e}"), "file")),
                } };
        }
    });
    vm.requiring(Capability::Environment, |vm| {
        natives! { vm;
            "command-line" 0 0 => |vm: &mut Vm, _, _| { let args: Vec<String> = std::env::args().skip(1).collect(); vm.to_value(args) };
            "getenv" 1 1 => |vm: &mut Vm, a, _| { let k = string(vm, arg(vm, a, 0), "getenv")?; vm.to_value(std::env::var(k).ok()) };
            "get-environment-variable" 1 1 => |vm: &mut Vm, a, _| { let k = string(vm, arg(vm, a, 0), "get-environment-variable")?; vm.to_value(std::env::var(k).ok()) };
            "get-environment-variables" 0 0 => |vm: &mut Vm, _, _| {
                let vars: Vec<(String, String)> = std::env::vars().collect();
                let mut pairs = Vec::new();
                for (k, v) in vars {
                    let k = vm.make_string(k.as_bytes());
                    let k = vm.root(k);
                    let v = vm.make_string(v.as_bytes());
                    let pair = vm.alloc_pair(k.get(), v);
                    pairs.push(vm.root(pair));
                }
                let items: Vec<Value> = pairs.iter().map(|r| r.get()).collect();
                Ok(vm.make_list(&items)) };
        }
    });
}

fn repr_char(c: Value) -> Result<String, Error> {
    if c.is_char() { Ok(c.as_char().to_string()) } else { Err(type_error("write-char", "char", c)) }
}

/// Text written by `display`/`write` with an optional port argument.
pub fn display_to(vm: &mut Vm, v: Value, port: Option<Value>, write: bool, newline: bool) -> Result<(), Error> {
    let mut s = String::new();
    print(&mut s, v, write);
    if newline {
        s.push('\n');
    }
    write_out(vm, port, &s)
}
