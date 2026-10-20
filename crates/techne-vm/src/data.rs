//! Data: values that are only data, held in Rust.
//!
//! Worlds share no values. What goes from one to another (a request into
//! the world that handles it, its response back) is copied out as [`Data`]
//! with [`Vm::to_data`] and into the other with [`Vm::from_data`]. Data
//! owns all it holds and is `Send`, so it can cross threads too.
//!
//! Data is what `write` writes and `read` reads back as it was: booleans,
//! integers of 64 bits, floats, characters, strings, bytevectors, symbols,
//! keywords, proper lists, vectors, and dicts whose keys are atoms (all
//! but lists, vectors and dicts), of at most [`MAX_DICT`] entries.
//! Anything else (procedures, records, foreign objects, sets, other
//! numbers, a dict keyed by a container) is refused, as are cycles and
//! nesting past `reader::MAX_DEPTH`.
//!
//! A copy shares nothing with the original: what was shared within it is
//! copied each time it occurs, so a few objects referring to each other
//! twice over can make an enormous copy. Copying out is therefore given a
//! limit of the bytes it may make; copying in admits what it will make
//! into the world first, as an allocation is.

use crate::{
    api::Value as Held,
    builtins::type_error,
    heap::{Kind, field, is_kind, len_of, str_bytes},
    num,
    reader::{MAX_DEPTH, intern, symbol_name},
    value::Value,
    vm::{Error, Vm},
};

/// Plain data, held in Rust: see the module's documentation.
#[derive(Clone, Debug, PartialEq)]
pub enum Data {
    Bool(bool),
    /// An exact integer.
    Int(i64),
    Float(f64),
    Char(char),
    String(String),
    Bytes(Vec<u8>),
    Symbol(String),
    /// `#:name`, by its name.
    Keyword(String),
    /// A proper list; the empty list is `List(vec![])`.
    List(Vec<Data>),
    Vector(Vec<Data>),
    /// A dict's keys and values, in no particular order.
    Dict(Vec<(Data, Data)>),
}

/// What a part of data is taken to hold, in a world or in Rust, beyond
/// its text: a pair, a vector's or dict's slot and its share of their
/// headers, the scratch it is made on, generously.
const PART_BYTES: usize = 64;

fn spend(left: &mut usize, bytes: usize) -> Result<(), Error> {
    *left = left
        .checked_sub(PART_BYTES.saturating_add(bytes))
        .ok_or_else(|| Error::new("to-data: past the size limit (shared parts are copied each time)"))?;
    Ok(())
}

/// The most entries a dict of data has. Making a dict compares keys whose
/// hashes are equal with each other, and distinct atoms can share a hash,
/// so the work grows as the square of the entries in the worst case.
pub const MAX_DICT: usize = 4096;

impl Data {
    /// At least how many bytes the data takes in a world, a new symbol
    /// counted as interning one does; `None` past `MAX_DEPTH`.
    fn size(&self, depth: usize) -> Option<usize> {
        if depth > MAX_DEPTH {
            return None;
        }
        let sum = |items: &mut dyn Iterator<Item = &Data>| {
            let mut n = 0usize;
            for x in items {
                n = n.saturating_add(x.size(depth + 1)?);
            }
            Some(n)
        };
        Some(PART_BYTES.saturating_add(match self {
            Data::String(s) => s.len(),
            Data::Symbol(s) | Data::Keyword(s) => crate::reader::SYMBOL_BYTES + s.len(),
            Data::Bytes(b) => b.len(),
            Data::List(items) | Data::Vector(items) => sum(&mut items.iter())?,
            Data::Dict(entries) => sum(&mut entries.iter().flat_map(|(k, v)| [k, v]))?,
            _ => 0,
        }))
    }

    /// Whether this can key a dict: an atom, no container.
    fn is_atom(&self) -> bool {
        !matches!(self, Data::List(_) | Data::Vector(_) | Data::Dict(_))
    }
}

/// A native's result made in the calling world, as [`Vm::from_data`]
/// makes it: an async native can return data another world handed it.
impl crate::api::IntoValue for Data {
    fn into_value(self, vm: &mut Vm) -> Result<Held, Error> {
        vm.from_data(&self)
    }
}

impl Vm {
    /// Copy the data `v` out of this world, making at most `limit` bytes
    /// (counted as [`Vm::from_data`] admits them). Anything but data is an
    /// error saying what it found, as is a copy past `limit`.
    pub fn to_data(&mut self, v: &Held, limit: usize) -> Result<Data, Error> {
        self.host_try(|vm| {
            let v = vm.import_value(v)?;
            let mut left = limit;
            to_data(v, 0, &mut left)
        })
    }

    /// Write bounded data for a text protocol, or return `None` for a
    /// value that needs a handle. Unlike [`Vm::to_data`], this admits
    /// improper lists and all numbers, but not dicts. These are the data
    /// values of the node protocol; its older text representation need
    /// not narrow numbers to integers of 64 bits.
    ///
    /// Parts and text share the byte budget of `to_data`; the written
    /// form must also fit `limit`. Cycles, nesting past `MAX_DEPTH` and
    /// values past either limit are errors, before they are printed.
    /// Unsupported containers are checked too, so the caller can safely
    /// use [`Vm::brief`] as a handle's preview after `Ok(None)`.
    pub fn to_written_data(&mut self, v: &Held, limit: usize) -> Result<Option<String>, Error> {
        self.host_try(|vm| {
            let v = vm.import_value(v)?;
            if !written_data(v, limit)? {
                return Ok(None);
            }
            let mut out = String::new();
            if !crate::builtins::print_within(&mut out, v, true, crate::builtins::Sharing::None, limit) {
                return Err(Error::new("to-data: written form past the size limit"));
            }
            Ok(Some(out))
        })
    }

    /// Make `data` in this world, as a value of its own.
    pub fn from_data(&mut self, data: &Data) -> Result<Held, Error> {
        let size = data.size(0).ok_or_else(|| Error::new(format!("from-data: nested more than {MAX_DEPTH} deep")))?;
        self.host_try(|vm| {
            vm.admit(size)?;
            let mark = vm.scratch.len();
            let r = crate::nested(|| build(vm, data, 0));
            let v = match r {
                Ok(()) => vm.scratch.pop().expect("the value built"),
                Err(e) => {
                    vm.scratch.truncate(mark);
                    return Err(e);
                }
            };
            Ok(vm.export(v))
        })
    }
}

/// Check the text protocol's data without first listing an unbounded
/// container. Shared parts spend the budget each time they are written.
fn written_data(v: Value, mut left: usize) -> Result<bool, Error> {
    enum Step {
        Enter(Value, usize),
        Fields(Value, usize, usize),
    }
    let mut data = true;
    let mut path = std::collections::HashSet::new();
    let mut todo = vec![Step::Enter(v, 0)];
    // SAFETY: the walk reads the heap and allocates nothing, so nothing
    // moves while it runs; `v` and all it reaches are of this world, the
    // caller holding `v` rooted.
    unsafe {
        while let Some(step) = todo.pop() {
            let (v, depth) = match step {
                Step::Fields(v, i, depth) => {
                    let n = len_of(v.as_ptr());
                    if i == n {
                        path.remove(&v.bits());
                    } else {
                        todo.push(Step::Fields(v, i + 1, depth));
                        let part = field(v.as_ptr(), i);
                        // A list's tail is written in the list: another pair of
                        // it at the list's depth, the empty list not at all; a
                        // dotted tail inside it, as an element is.
                        match (is_kind(v, Kind::Pair) && i == 1, part == Value::NIL, is_kind(part, Kind::Pair)) {
                            (true, true, _) => {}
                            (true, false, true) => todo.push(Step::Enter(part, depth)),
                            _ => todo.push(Step::Enter(part, depth + 1)),
                        }
                    }
                    continue;
                }
                Step::Enter(v, depth) => (v, depth),
            };
            let kind = v.is_ptr().then(|| Kind::of(crate::heap::kind_of(v.as_ptr()))).flatten();
            // As the reader counts: a datum inside `depth` containers, and what
            // a container holds, its closing parenthesis too, inside one more.
            let opens = v == Value::NIL || matches!(kind, Some(Kind::Pair | Kind::Vector | Kind::Bytevector));
            if depth + usize::from(opens) >= MAX_DEPTH {
                return Err(Error::new(format!("to-data: nested more than {MAX_DEPTH} deep")));
            }
            let bytes = match kind {
                Some(Kind::String | Kind::Bytevector) => str_bytes(v.as_ptr()).len(),
                Some(Kind::BigInt) => len_of(v.as_ptr()).saturating_mul(8),
                _ if v.is_symbol() => crate::reader::SYMBOL_BYTES + symbol_name(v.as_symbol()).len(),
                _ if v.is_keyword() => crate::reader::SYMBOL_BYTES + symbol_name(v.as_keyword()).len(),
                _ => 0,
            };
            spend(&mut left, bytes)?;
            // Walk what a handle's preview may print too. An unsupported
            // container is still not data, but cannot hide an unbounded
            // structure behind the preview's output limit.
            if matches!(kind, Some(Kind::Dict | Kind::Set)) && crate::dict::count(v) > MAX_DICT {
                return Err(Error::new(format!("to-data: more than {MAX_DICT} entries")));
            }
            match kind {
                Some(
                    Kind::Pair
                    | Kind::Vector
                    | Kind::Ratio
                    | Kind::Complex
                    | Kind::Box
                    | Kind::Record
                    | Kind::Rtd
                    | Kind::Dict
                    | Kind::Set
                    | Kind::Node,
                ) => {
                    data &= matches!(kind, Some(Kind::Pair | Kind::Vector | Kind::Ratio | Kind::Complex));
                    if !path.insert(v.bits()) {
                        return Err(Error::new("to-data: cyclic value"));
                    }
                    let n = len_of(v.as_ptr());
                    if n.saturating_mul(PART_BYTES) > left {
                        return Err(Error::new("to-data: past the size limit"));
                    }
                    todo.push(Step::Fields(v, 0, depth));
                }
                Some(Kind::String | Kind::Bytevector | Kind::BigInt) => {}
                None if v == Value::NIL
                    || v == Value::TRUE
                    || v == Value::FALSE
                    || v.is_int()
                    || v.is_float()
                    || v.is_char()
                    || v.is_symbol()
                    || v.is_keyword() => {}
                _ => data = false,
            }
        }
    }
    Ok(data)
}

/// Copy `v` out, inside `depth` containers, with `left` bytes of the
/// limit left.
fn to_data(v: Value, depth: usize, left: &mut usize) -> Result<Data, Error> {
    let container = v == Value::NIL || is_kind(v, Kind::Pair) || is_kind(v, Kind::Vector) || is_kind(v, Kind::Dict);
    if container && depth >= MAX_DEPTH {
        return Err(Error::new(format!("to-data: nested more than {MAX_DEPTH} deep")));
    }
    let text = if is_kind(v, Kind::String) || is_kind(v, Kind::Bytevector) {
        unsafe { str_bytes(v.as_ptr()) }.len()
    } else if v.is_symbol() {
        crate::reader::SYMBOL_BYTES + symbol_name(v.as_symbol()).len()
    } else if v.is_keyword() {
        crate::reader::SYMBOL_BYTES + symbol_name(v.as_keyword()).len()
    } else {
        0
    };
    spend(left, text)?;
    // A container's parts each take a part at least: past what is left,
    // it is refused before they are listed.
    let parts = if is_kind(v, Kind::Pair) {
        crate::builtins::list_len(v, "to-data").map_err(|_| type_error("to-data", "proper list", v))?
    } else if is_kind(v, Kind::Vector) {
        unsafe { len_of(v.as_ptr()) }
    } else if is_kind(v, Kind::Dict) {
        let n = crate::dict::count(v);
        if n > MAX_DICT {
            return Err(Error::new(format!("to-data: a dict of more than {MAX_DICT} entries")));
        }
        2 * n
    } else {
        0
    };
    if parts.saturating_mul(PART_BYTES) > *left {
        return Err(Error::new("to-data: past the size limit (shared parts are copied each time)"));
    }
    let nested = |items: Vec<Value>, left: &mut usize| -> Result<Vec<Data>, Error> {
        items.into_iter().map(|x| crate::nested(|| to_data(x, depth + 1, left))).collect()
    };
    Ok(if v == Value::TRUE || v == Value::FALSE {
        Data::Bool(v == Value::TRUE)
    } else if v.is_int() {
        Data::Int(v.as_int())
    } else if is_kind(v, Kind::BigInt) {
        match num::heap_int(v) {
            num::N::I(i) => Data::Int(i),
            _ => return Err(type_error("to-data", "integer of 64 bits", v)),
        }
    } else if v.is_float() {
        Data::Float(v.as_float())
    } else if v.is_char() {
        Data::Char(v.as_char())
    } else if v.is_symbol() {
        Data::Symbol(symbol_name(v.as_symbol()).to_string())
    } else if v.is_keyword() {
        Data::Keyword(symbol_name(v.as_keyword()).to_string())
    } else if is_kind(v, Kind::String) {
        Data::String(String::from_utf8(unsafe { str_bytes(v.as_ptr()) }.to_vec()).map_err(|_| Error::new("to-data: a string not UTF-8"))?)
    } else if is_kind(v, Kind::Bytevector) {
        Data::Bytes(unsafe { str_bytes(v.as_ptr()) }.to_vec())
    } else if v == Value::NIL || is_kind(v, Kind::Pair) {
        // Cycles are refused here, before anything grows.
        let items = crate::builtins::list_values(v).ok_or_else(|| type_error("to-data", "proper list", v))?;
        Data::List(nested(items, left)?)
    } else if is_kind(v, Kind::Vector) {
        let items = unsafe { (0..len_of(v.as_ptr())).map(|i| field(v.as_ptr(), i)).collect() };
        Data::Vector(nested(items, left)?)
    } else if is_kind(v, Kind::Dict) {
        let flat = nested(crate::dict::entries(v), left)?;
        let mut flat = flat.into_iter();
        let entries: Vec<(Data, Data)> = std::iter::from_fn(|| Some((flat.next()?, flat.next()?))).collect();
        if entries.iter().any(|(k, _)| !k.is_atom()) {
            return Err(Error::new("to-data: a dict keyed by a list, vector or dict"));
        }
        Data::Dict(entries)
    } else {
        return Err(type_error("to-data", "data", v));
    })
}

/// Make `data` and push it on `vm.scratch`, where what is made is rooted
/// until its container takes it.
fn build(vm: &mut Vm, data: &Data, depth: usize) -> Result<(), Error> {
    let nested = |vm: &mut Vm, items: &mut dyn Iterator<Item = &Data>| -> Result<usize, Error> {
        if depth >= MAX_DEPTH {
            return Err(Error::new(format!("from-data: nested more than {MAX_DEPTH} deep")));
        }
        let mark = vm.scratch.len();
        for x in items {
            crate::nested(|| build(vm, x, depth + 1))?;
        }
        Ok(mark)
    };
    let v = match data {
        Data::Bool(b) => Value::bool(*b),
        Data::Int(i) => vm.make_int_raw(*i),
        Data::Float(f) => Value::float(*f),
        Data::Char(c) => Value::char(*c),
        Data::String(s) => vm.make_string_raw(s),
        Data::Bytes(b) => vm.make_bytevector_raw(b),
        Data::Symbol(s) => Value::symbol(new_symbol(vm, s)?),
        Data::Keyword(s) => Value::keyword(new_symbol(vm, s)?),
        Data::List(items) | Data::Vector(items) => {
            let mark = nested(vm, &mut items.iter())?;
            let items = vm.scratch.split_off(mark);
            if matches!(data, Data::List(_)) { vm.make_list_raw(&items) } else { vm.make_vector_raw(&items) }
        }
        Data::Dict(entries) => {
            if entries.len() > MAX_DICT {
                return Err(Error::new(format!("from-data: a dict of more than {MAX_DICT} entries")));
            }
            if entries.iter().any(|(k, _)| !k.is_atom()) {
                return Err(Error::new("from-data: a dict keyed by a list, vector or dict"));
            }
            let mark = nested(vm, &mut entries.iter().flat_map(|(k, v)| [k, v]))?;
            crate::dict::from_scratch(vm, Kind::Dict, mark)
        }
    };
    vm.scratch.push(v);
    Ok(())
}

/// The symbol `name`, interned: a new one is admitted first with what its
/// table may grow by, which admitting the data did not see. What is made
/// so far is on `scratch`, so admitting may collect.
fn new_symbol(vm: &mut Vm, name: &str) -> Result<u32, Error> {
    if crate::reader::lookup(name).is_none() {
        vm.admit(name.len() + crate::reader::SYMBOL_BYTES + crate::reader::intern_growth())?;
    }
    Ok(intern(name))
}
