//! JSON (RFC 8259) to data and back.
//!
//! An object reads as a dict with string keys (a key given twice keeps its
//! last value), an array as a vector, `null` as the symbol `null`, a
//! number as an exact integer when it has neither fraction nor exponent,
//! else a float. Reading is metered as `read` is and nests at most
//! `reader::MAX_DEPTH` deep. It makes values as it reads, and each key
//! once: keys are immutable strings, shared by the objects that have them.
//!
//! Writing takes the same data, lists as arrays too and symbols as dict
//! keys, and writes an object's members in the order of their keys, so
//! equal data is the same text.

use rustc_hash::FxHashMap;

use crate::{
    api::Root,
    builtins::{str_arg, type_error},
    bytes::bytes_arg,
    heap::{self, Kind, field, is_kind, len_of, str_bytes},
    num,
    reader::{self, MAX_DEPTH, intern, symbol_name},
    value::Value,
    vm::{
        Error, Vm,
        meter::{LONG, Meter},
    },
};

type R = Result<Value, Error>;

/// Keys kept to be shared, and the longest kept.
const SHARED_KEYS: usize = 1024;
const SHARED_KEY_LEN: usize = 64;

/// Reads JSON text into values as it goes. What it has made and still
/// needs is on `vm.scratch`, so that allocating, which may collect, moves
/// it safely: each value read is pushed there, an array or object pops
/// its items.
struct Parser<'a> {
    vm: &'a mut Vm,
    s: &'a [u8],
    pos: usize,
    depth: usize,
    meter: &'a Meter,
    /// The bytes of a string being read, escapes resolved.
    buf: Vec<u8>,
    /// Objects' keys read so far: the same key, one immutable string.
    keys: FxHashMap<Box<[u8]>, Root>,
    null: Value,
}

impl Parser<'_> {
    fn err<T>(&self, what: &str) -> Result<T, String> {
        Err(format!("{what} at byte {}", self.pos))
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.pos).copied()
    }

    fn space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, b: u8) -> Result<(), String> {
        self.space();
        if self.peek() == Some(b) {
            self.pos += 1;
            Ok(())
        } else {
            self.err(&format!("expected {}", b as char))
        }
    }

    /// After a long token, which may be many times what the last steps
    /// made: whether it is still within the room.
    fn long(&self, len: usize) -> Result<(), String> {
        self.meter.copied(len)
    }

    fn push(&mut self, v: Value) {
        self.vm.scratch.push(v);
    }

    /// Reads a value and pushes it.
    fn value(&mut self) -> Result<(), String> {
        self.meter.step()?;
        self.space();
        match self.peek() {
            Some(b'{') => self.nested(|p| p.object()),
            Some(b'[') => self.nested(|p| p.array()),
            Some(b'"') => {
                self.string()?;
                self.long(self.buf.len())?;
                let v = self.vm.make_string(std::str::from_utf8(&self.buf).expect("UTF-8"));
                self.push(v);
                Ok(())
            }
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => {
                for (word, v) in [("true", Value::TRUE), ("false", Value::FALSE), ("null", self.null)] {
                    if self.s[self.pos..].starts_with(word.as_bytes()) {
                        self.pos += word.len();
                        self.push(v);
                        return Ok(());
                    }
                }
                self.err("unexpected character")
            }
            None => self.err("unexpected end"),
        }
    }

    fn nested(&mut self, read: impl FnOnce(&mut Self) -> Result<(), String>) -> Result<(), String> {
        if self.depth >= MAX_DEPTH {
            return self.err(&format!("nested more than {MAX_DEPTH} deep"));
        }
        self.depth += 1;
        let r = crate::nested(|| read(self));
        self.depth -= 1;
        r
    }

    /// The items between `open` and `close`, separated by commas.
    fn items(&mut self, close: u8, mut item: impl FnMut(&mut Self) -> Result<(), String>) -> Result<(), String> {
        self.pos += 1;
        self.space();
        if self.peek() == Some(close) {
            self.pos += 1;
            return Ok(());
        }
        loop {
            item(self)?;
            self.space();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(c) if c == close => {
                    self.pos += 1;
                    return Ok(());
                }
                _ => return self.err(&format!("expected , or {}", close as char)),
            }
        }
    }

    fn object(&mut self) -> Result<(), String> {
        let mark = self.vm.scratch.len();
        self.items(b'}', |p| {
            p.space();
            if p.peek() != Some(b'"') {
                return p.err("expected a string key");
            }
            p.string()?;
            p.long(p.buf.len())?;
            let key = p.key();
            p.push(key);
            p.expect(b':')?;
            p.value()
        })?;
        let d = crate::dict::from_scratch(self.vm, Kind::Dict, mark);
        self.push(d);
        Ok(())
    }

    /// The key whose bytes are in `buf`: an immutable string, the one
    /// made before for the same bytes if any.
    fn key(&mut self) -> Value {
        if let Some(k) = self.keys.get(self.buf.as_slice()) {
            return k.get();
        }
        let k = self.vm.make_string(std::str::from_utf8(&self.buf).expect("UTF-8"));
        unsafe { *k.as_ptr() |= heap::IMMUTABLE };
        if self.buf.len() <= SHARED_KEY_LEN && self.keys.len() < SHARED_KEYS {
            let root = self.vm.root(k);
            self.keys.insert(self.buf.as_slice().into(), root);
        }
        k
    }

    fn array(&mut self) -> Result<(), String> {
        let mark = self.vm.scratch.len();
        self.items(b']', |p| p.value())?;
        let items = self.vm.scratch.split_off(mark);
        let v = self.vm.make_vector(&items);
        self.push(v);
        Ok(())
    }

    /// After `"`: the text up to the closing one into `buf`, escapes
    /// resolved.
    fn string(&mut self) -> Result<(), String> {
        self.pos += 1;
        self.buf.clear();
        loop {
            let plain = self.s[self.pos..].iter().position(|&b| b == b'"' || b == b'\\' || b < 0x20);
            let Some(n) = plain else { return self.err("unterminated string") };
            self.buf.extend_from_slice(&self.s[self.pos..self.pos + n]);
            self.pos += n;
            match self.s[self.pos] {
                b'"' => {
                    self.pos += 1;
                    return Ok(());
                }
                b'\\' => {
                    self.pos += 1;
                    let c = match self.peek() {
                        Some(b'"') => '"',
                        Some(b'\\') => '\\',
                        Some(b'/') => '/',
                        Some(b'b') => '\x08',
                        Some(b'f') => '\x0c',
                        Some(b'n') => '\n',
                        Some(b'r') => '\r',
                        Some(b't') => '\t',
                        Some(b'u') => {
                            self.pos += 1;
                            let hi = self.hex4()?;
                            let code = if (0xd800..0xdc00).contains(&hi) && self.s[self.pos..].starts_with(b"\\u") {
                                self.pos += 2;
                                let lo = self.hex4()?;
                                if !(0xdc00..0xe000).contains(&lo) {
                                    return self.err("unpaired surrogate");
                                }
                                0x10000 + ((hi - 0xd800) << 10) + (lo - 0xdc00)
                            } else {
                                hi
                            };
                            let Some(c) = char::from_u32(code) else { return self.err("unpaired surrogate") };
                            self.buf.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
                            continue;
                        }
                        _ => return self.err("bad escape"),
                    };
                    self.pos += 1;
                    self.buf.push(c as u8);
                }
                _ => return self.err("control character in a string"),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let digits = self.s.get(self.pos..self.pos + 4).filter(|d| d.iter().all(u8::is_ascii_hexdigit));
        let Some(d) = digits else { return self.err("bad \\u escape") };
        self.pos += 4;
        Ok(u32::from_str_radix(std::str::from_utf8(d).expect("hex digits"), 16).expect("hex"))
    }

    fn number(&mut self) -> Result<(), String> {
        let start = self.pos;
        let digits = |p: &mut Self| {
            let n = p.s[p.pos..].iter().take_while(|b| b.is_ascii_digit()).count();
            p.pos += n;
            n
        };
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => {
                digits(self);
            }
            _ => return self.err("bad number"),
        }
        let mut exact = true;
        if self.peek() == Some(b'.') {
            self.pos += 1;
            exact = false;
            if digits(self) == 0 {
                return self.err("bad number");
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            exact = false;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if digits(self) == 0 {
                return self.err("bad number");
            }
        }
        let text = std::str::from_utf8(&self.s[start..self.pos]).expect("ASCII");
        self.long(text.len())?;
        let v = if !exact {
            let f: f64 = text.parse().expect("a JSON number");
            if f.is_infinite() {
                return self.err("number too large for a float");
            }
            Value::float(f)
        } else if let Ok(i) = text.parse::<i64>() {
            self.vm.make_int(i)
        } else {
            match num::parse(text, 10) {
                num::Parsed::Number(n) => self.vm.constant(&reader::number(n)),
                _ => return self.err("bad number"),
            }
        };
        self.push(v);
        Ok(())
    }
}

fn string_to_json(vm: &mut Vm, args: usize, _: usize) -> R {
    let text = vm.regs[args];
    let bytes = if is_kind(text, Kind::String) { str_arg(text, "string->json")?.as_bytes() } else { bytes_arg(text, "string->json")? };
    if let Err(e) = std::str::from_utf8(bytes) {
        return Err(Error::new(format!("string->json: invalid UTF-8 at byte {}", e.valid_up_to())));
    }
    // Reading allocates, which moves what is in the nursery: the text too
    // unless it is old (as long text is).
    let owned;
    let s = if vm.heap.in_nursery(bytes.as_ptr() as *mut u64) {
        owned = bytes.to_vec();
        &owned[..]
    } else {
        bytes
    };
    let meter = Meter::new(vm);
    let mark = vm.scratch.len();
    let null = Value::symbol(intern("null"));
    let mut p = Parser { vm, s, pos: 0, depth: 0, meter: &meter, buf: Vec::new(), keys: FxHashMap::default(), null };
    let read = p.value().and_then(|()| {
        p.space();
        if p.pos < s.len() { p.err("text after the value") } else { Ok(()) }
    });
    drop(p);
    match read {
        Ok(()) => Ok(vm.scratch.pop().expect("the value read")),
        Err(e) => {
            vm.scratch.truncate(mark);
            Err(meter.error(vm).unwrap_or_else(|| Error::new(format!("string->json: {e}"))))
        }
    }
}

// ----- writing -----

fn write_string(out: &mut String, s: &str) {
    use std::fmt::Write as _;
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Writes `v`, inside `depth` arrays and objects. The text may be far
/// more than the data (shared parts are written each time), so it is
/// metered as it grows.
fn write(out: &mut String, v: Value, depth: usize, meter: &Meter) -> Result<(), Error> {
    let fail = |what: &str| type_error("json->string", what, v);
    let halted = |m: String| Error::new(format!("json->string: {m}"));
    meter.step().map_err(halted)?;
    let container = v == Value::NIL || is_kind(v, Kind::Pair) || is_kind(v, Kind::Vector) || is_kind(v, Kind::Dict);
    if container && depth >= MAX_DEPTH {
        return Err(Error::new(format!("json->string: nested more than {MAX_DEPTH} deep")));
    }
    if v == Value::TRUE || v == Value::FALSE {
        out.push_str(if v == Value::TRUE { "true" } else { "false" });
    } else if v.is_symbol() && *symbol_name(v.as_symbol()) == *"null" {
        out.push_str("null");
    } else if v.is_int() {
        out.push_str(&v.as_int().to_string());
    } else if is_kind(v, Kind::BigInt) {
        out.push_str(&num::to_string_radix(&num::heap_int(v), 10));
    } else if v.is_float() {
        if !v.as_float().is_finite() {
            return Err(fail("finite number"));
        }
        out.push_str(&reader::float_repr(v.as_float()));
    } else if is_kind(v, Kind::String) {
        let s = str_arg(v, "json->string")?;
        write_string(out, s);
        meter.copied(s.len()).map_err(halted)?;
    } else if is_kind(v, Kind::Vector) {
        let items: Vec<Value> = (0..unsafe { len_of(v.as_ptr()) }).map(|i| unsafe { field(v.as_ptr(), i) }).collect();
        write_array(out, items, depth, meter)?;
    } else if v == Value::NIL || is_kind(v, Kind::Pair) {
        let items = crate::builtins::list_values(v).ok_or_else(|| fail("proper list"))?;
        write_array(out, items, depth, meter)?;
    } else if is_kind(v, Kind::Dict) {
        let mut members = Vec::new();
        for e in crate::dict::entries(v).chunks(2) {
            let key = if is_kind(e[0], Kind::String) {
                unsafe { String::from_utf8_lossy(str_bytes(e[0].as_ptr())).into_owned() }
            } else if e[0].is_symbol() {
                symbol_name(e[0].as_symbol()).to_string()
            } else {
                return Err(type_error("json->string", "string or symbol key", e[0]));
            };
            members.push((key, e[1]));
        }
        members.sort_by(|a, b| a.0.cmp(&b.0));
        if let Some(w) = members.windows(2).find(|w| w[0].0 == w[1].0) {
            return Err(Error::new(format!("json->string: the key {:?} twice, as a string and a symbol", w[0].0)));
        }
        out.push('{');
        for (i, (k, x)) in members.into_iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            write_string(out, &k);
            out.push(':');
            crate::nested(|| write(out, x, depth + 1, meter))?;
        }
        out.push('}');
    } else {
        return Err(fail("JSON data"));
    }
    Ok(())
}

fn write_array(out: &mut String, items: Vec<Value>, depth: usize, meter: &Meter) -> Result<(), Error> {
    out.push('[');
    for (i, x) in items.into_iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        crate::nested(|| write(out, x, depth + 1, meter))?;
    }
    out.push(']');
    Ok(())
}

pub fn install(vm: &mut Vm) {
    crate::natives! { vm;
        /// Return the data TEXT holds as JSON.
        /// TEXT is a string, or a bytevector of UTF-8. Objects become dicts
        /// with string keys, arrays vectors, `null` the symbol `null`.
        "(string->json text)" => string_to_json;
        /// Return the JSON text of OBJ, without spaces.
        /// Dicts become objects, with their members in the order of their
        /// keys (strings or symbols); vectors and lists become arrays, the
        /// symbol `null` null.
        "(json->string obj)" => |vm: &mut Vm, a, _| {
            let mut out = String::new();
            let meter = Meter::new(vm);
            if let Err(e) = write(&mut out, vm.regs[a], 0, &meter) {
                return Err(meter.error(vm).unwrap_or(e));
            }
            vm.admit_items(out.len(), 1, 64)?;
            Ok(vm.make_string(&out)) };
    }
}
