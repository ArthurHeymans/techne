//! Ports (R7RS 6.13): textual string, file and standard ports, and binary
//! bytevector and file ports.
//!
//! A textual input port is text read so far and, for files and stdin, where
//! more comes from, a line at a time. `read` takes a datum from the text and
//! asks for more lines while the datum is incomplete, so a datum may span
//! lines of a file or of stdin. A binary input port is likewise bytes read
//! so far and where more come from, a block at a time. A read that fails is
//! an error (a `file-error?`), never the end of the input.

use std::{
    cell::RefCell,
    fs::File,
    io::{BufRead, BufReader, BufWriter, Read, Write},
};

use crate::{
    api::Foreign,
    builtins::{Sharing, index_arg, print_with, range_args, type_error},
    bytes::{byte_arg, bytes_arg},
    heap::{self, Kind, bytes_mut, is_kind, str_bytes},
    reader,
    value::Value,
    vm::{Capability, Error, ErrorKind, Native, NativeFn, NativeImpl, Vm},
};

type R = Result<Value, Error>;

pub enum Port {
    StringOut(String),
    /// Output handed to a Rust function as it is written.
    Sink(Box<dyn FnMut(&str)>),
    FileOut(BufWriter<File>),
    /// The VM's standard output.
    Stdout,
    Stderr,
    In {
        text: String,
        pos: usize,
        /// Where more text comes from: a file or stdin.
        source: Option<Box<dyn BufRead>>,
    },
    /// Binary input: bytes read so far and where more come from.
    BytesIn {
        bytes: Vec<u8>,
        pos: usize,
        source: Option<Box<dyn Read>>,
    },
    BytesOut(Vec<u8>),
    BinaryFileOut(BufWriter<File>),
    Closed {
        input: bool,
        binary: bool,
    },
}

/// How much a binary input port reads from its source at a time.
const BLOCK: usize = 1 << 16;

fn io_error(e: std::io::Error) -> Error {
    Error::new(e.to_string()).with_kind(ErrorKind::File)
}

impl Port {
    fn is_input(&self) -> bool {
        matches!(self, Port::In { .. } | Port::BytesIn { .. } | Port::Closed { input: true, .. })
    }

    fn is_binary(&self) -> bool {
        matches!(self, Port::BytesIn { .. } | Port::BytesOut(_) | Port::BinaryFileOut(_) | Port::Closed { binary: true, .. })
    }

    /// The unread bytes of a binary input port, reading another block from
    /// its source when none are left; empty at its end.
    fn available_bytes(&mut self) -> Result<&[u8], Error> {
        let Port::BytesIn { bytes, pos, source } = self else { return Err(Error::new("not an open binary input port")) };
        if *pos == bytes.len()
            && let Some(source) = source
        {
            bytes.clear();
            *pos = 0;
            bytes.resize(BLOCK, 0);
            let n = loop {
                match source.read(bytes) {
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    r => break r,
                }
            };
            bytes.truncate(*n.as_ref().unwrap_or(&0));
            n.map_err(io_error)?;
        }
        Ok(&bytes[*pos..])
    }

    /// Reads another line from the source into the text; false at its end.
    fn refill(&mut self) -> Result<bool, Error> {
        let Port::In { text, pos, source: Some(source) } = self else { return Ok(false) };
        if *pos == text.len() {
            text.clear();
            *pos = 0;
        }
        let n = source.read_line(text).map_err(|e| Error::new(format!("read: {e}")).with_kind(ErrorKind::File))?;
        Ok(n > 0)
    }

    /// The unread text, with at least one character unless at the end.
    fn available(&mut self) -> Result<&str, Error> {
        loop {
            match self {
                Port::In { text, pos, .. } if *pos < text.len() => break,
                Port::In { .. } => {
                    if !self.refill()? {
                        break;
                    }
                }
                _ => return Err(Error::new("not an open input port")),
            }
        }
        let Port::In { text, pos, .. } = self else { unreachable!() };
        Ok(&text[*pos..])
    }

    fn advance(&mut self, n: usize) {
        if let Port::In { pos, .. } | Port::BytesIn { pos, .. } = self {
            *pos += n;
        }
    }

    fn write_bytes(&mut self, b: &[u8]) -> Result<(), Error> {
        match self {
            Port::BytesOut(out) => out.extend_from_slice(b),
            Port::BinaryFileOut(w) => w.write_all(b).map_err(io_error)?,
            _ => return Err(Error::new("not an open binary output port")),
        }
        Ok(())
    }

    fn write(&mut self, vm_out: &mut dyn Write, s: &str) -> Result<(), Error> {
        let io = |e: std::io::Error| Error::new(e.to_string()).with_kind(ErrorKind::File);
        match self {
            Port::StringOut(out) => out.push_str(s),
            Port::Sink(f) => f(s),
            Port::FileOut(w) => w.write_all(s.as_bytes()).map_err(io)?,
            Port::Stdout => vm_out.write_all(s.as_bytes()).map_err(io)?,
            Port::Stderr => {
                let _ = vm_out.flush();
                std::io::stderr().write_all(s.as_bytes()).map_err(io)?
            }
            _ => return Err(Error::new("not an open output port")),
        }
        Ok(())
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

/// Task-local keys of the `current-output-port` and `current-input-port`
/// parameters.
pub const OUTPUT_PORT_KEY: i64 = -1;
pub const INPUT_PORT_KEY: i64 = -2;

/// The port a parameterized standard port is set to, if any.
fn parameterized(vm: &Vm, key: i64) -> Option<Value> {
    vm.locals.get(&key).map(|r| r.get()).filter(|v| v.is_truthy())
}

/// Write text to `port` (a port value) or to the current output.
pub fn write_out(vm: &mut Vm, port: Option<Value>, text: &str) -> Result<(), Error> {
    match port.or_else(|| parameterized(vm, OUTPUT_PORT_KEY)) {
        None => {
            let _ = vm.out.write_all(text.as_bytes());
            Ok(())
        }
        Some(p) => {
            let port = port_arg(vm, p)?;
            port.borrow_mut().write(&mut vm.out, text)
        }
    }
}

/// Text written by `display`/`write` with an optional port argument.
pub fn display_to(vm: &mut Vm, v: Value, port: Option<Value>, write: bool, newline: bool) -> Result<(), Error> {
    write_shared(vm, v, port, write, newline, Sharing::Cycles)
}

fn write_shared(vm: &mut Vm, v: Value, port: Option<Value>, write: bool, newline: bool, sharing: Sharing) -> Result<(), Error> {
    let mut s = String::new();
    print_with(&mut s, v, write, sharing);
    if newline {
        s.push('\n');
    }
    write_out(vm, port, &s)
}

/// The input port argument `i`, else the current input port.
fn input_port(vm: &mut Vm, args: usize, n: usize, i: usize) -> Result<PortRef, Error> {
    let v = if n > i {
        vm.regs[args + i]
    } else {
        match parameterized(vm, INPUT_PORT_KEY) {
            Some(p) => p,
            None => vm.global_value("%stdin")?,
        }
    };
    port_arg(vm, v)
}

fn arg(vm: &Vm, args: usize, i: usize) -> Value {
    vm.regs[args + i]
}

fn string(v: Value, who: &str) -> Result<String, Error> {
    if is_kind(v, Kind::String) {
        Ok(String::from_utf8_lossy(unsafe { str_bytes(v.as_ptr()) }).into_owned())
    } else {
        Err(type_error(who, "string", v))
    }
}

fn read_line(vm: &mut Vm, args: usize, n: usize) -> R {
    let p = input_port(vm, args, n, 0)?;
    let mut port = p.borrow_mut();
    let mut line = String::new();
    loop {
        let rest = port.available()?;
        if rest.is_empty() {
            break;
        }
        match rest.find('\n') {
            Some(i) => {
                line.push_str(&rest[..i]);
                port.advance(i + 1);
                if line.ends_with('\r') {
                    line.pop();
                }
                return Ok(vm.make_string(line.as_bytes()));
            }
            None => {
                let k = rest.len();
                line.push_str(rest);
                port.advance(k);
            }
        }
    }
    Ok(if line.is_empty() { Value::EOF } else { vm.make_string(line.as_bytes()) })
}

fn read_char(vm: &mut Vm, args: usize, n: usize, consume: bool) -> R {
    let p = input_port(vm, args, n, 0)?;
    let mut port = p.borrow_mut();
    let c = port.available()?.chars().next();
    if consume && let Some(c) = c {
        port.advance(c.len_utf8());
    }
    Ok(c.map_or(Value::EOF, Value::char))
}

fn read_string(vm: &mut Vm, args: usize, n: usize) -> R {
    let k = crate::num::integer(arg(vm, args, 0), "read-string")?.max(0) as usize;
    let p = input_port(vm, args, n, 1)?;
    let mut port = p.borrow_mut();
    let mut out = String::new();
    let mut count = 0;
    while count < k {
        let rest = port.available()?;
        let Some(c) = rest.chars().next() else { break };
        out.push(c);
        port.advance(c.len_utf8());
        count += 1;
    }
    Ok(if count == 0 && k > 0 { Value::EOF } else { vm.make_string(out.as_bytes()) })
}

/// Everything left in the port.
fn read_all(vm: &mut Vm, args: usize, n: usize) -> R {
    let p = input_port(vm, args, n, 0)?;
    let mut port = p.borrow_mut();
    while port.refill()? {}
    let rest = port.available()?.to_owned();
    port.advance(rest.len());
    Ok(vm.make_string(rest.as_bytes()))
}

fn read_datum(vm: &mut Vm, args: usize, n: usize) -> R {
    let p = input_port(vm, args, n, 0)?;
    let datum = {
        let mut port = p.borrow_mut();
        loop {
            let rest = port.available()?;
            match reader::read_next(rest) {
                Ok(None) if port.refill()? => {}
                Ok(None) => {
                    let k = port.available()?.len();
                    port.advance(k);
                    break None;
                }
                Ok(Some((datum, used))) => {
                    port.advance(used);
                    break Some(datum);
                }
                Err(e) if e.starts_with(reader::INCOMPLETE) && port.refill()? => {}
                Err(e) => return Err(Error::new(format!("read: {e}")).with_kind(ErrorKind::Read)),
            }
        }
    };
    Ok(match datum {
        None => Value::EOF,
        Some(d) => vm.constant(&d),
    })
}

fn char_ready(vm: &mut Vm, args: usize, n: usize) -> R {
    let p = input_port(vm, args, n, 0)?;
    let port = p.borrow();
    Ok(Value::bool(match &*port {
        Port::In { text, pos, source } => *pos < text.len() || source.is_none(),
        _ => return Err(Error::new("char-ready?: not an open input port")),
    }))
}

fn get_output_string(vm: &mut Vm, args: usize, _: usize) -> R {
    let p = port_arg(vm, arg(vm, args, 0))?;
    let text = match &*p.borrow() {
        Port::StringOut(s) => s.clone(),
        _ => return Err(Error::new("get-output-string: not a string output port")),
    };
    Ok(vm.make_string(text.as_bytes()))
}

fn get_output_bytevector(vm: &mut Vm, args: usize, _: usize) -> R {
    let p = port_arg(vm, arg(vm, args, 0))?;
    let bytes = match &*p.borrow() {
        Port::BytesOut(b) => b.clone(),
        _ => return Err(Error::new("get-output-bytevector: not a bytevector output port")),
    };
    Ok(vm.make_bytevector(&bytes))
}

/// The binary input port argument `i`, else the current input port.
fn binary_input(vm: &mut Vm, args: usize, n: usize, i: usize) -> Result<PortRef, Error> {
    input_port(vm, args, n, i)
}

/// The output port argument `i`, else the current output port.
fn output_port(vm: &mut Vm, args: usize, n: usize, i: usize) -> Result<PortRef, Error> {
    let v = if n > i {
        arg(vm, args, i)
    } else {
        match parameterized(vm, OUTPUT_PORT_KEY) {
            Some(p) => p,
            None => return Err(Error::new("the standard output is not a binary port")),
        }
    };
    port_arg(vm, v)
}

fn read_u8(vm: &mut Vm, args: usize, n: usize, consume: bool) -> R {
    let p = binary_input(vm, args, n, 0)?;
    let mut port = p.borrow_mut();
    let b = port.available_bytes()?.first().copied();
    if consume && b.is_some() {
        port.advance(1);
    }
    Ok(b.map_or(Value::EOF, |b| Value::int_unchecked(b as i64)))
}

/// Reads up to `k` bytes, fewer only at the end of the input.
fn read_up_to(port: &mut Port, k: usize) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    while out.len() < k {
        let rest = port.available_bytes()?;
        if rest.is_empty() {
            break;
        }
        let take = rest.len().min(k - out.len());
        out.extend_from_slice(&rest[..take]);
        port.advance(take);
    }
    Ok(out)
}

fn read_bytevector(vm: &mut Vm, args: usize, n: usize) -> R {
    let k = index_arg(arg(vm, args, 0), "read-bytevector")?;
    let p = binary_input(vm, args, n, 1)?;
    let bytes = read_up_to(&mut p.borrow_mut(), k)?;
    Ok(if bytes.is_empty() && k > 0 { Value::EOF } else { vm.make_bytevector(&bytes) })
}

/// `(read-bytevector! bv [port start end])`: the number of bytes read.
fn read_bytevector_into(vm: &mut Vm, args: usize, n: usize) -> R {
    let v = arg(vm, args, 0);
    let len = bytes_arg(v, "read-bytevector!")?.len();
    if unsafe { *v.as_ptr() } & heap::IMMUTABLE != 0 {
        return Err(Error::new("read-bytevector!: bytevector literals cannot be changed"));
    }
    let (a, b) = range_args(vm, args, n, 2, len, "read-bytevector!")?;
    let p = binary_input(vm, args, n, 1)?;
    let bytes = read_up_to(&mut p.borrow_mut(), b - a)?;
    if bytes.is_empty() && b > a {
        return Ok(Value::EOF);
    }
    // Reading allocates nothing, so `v` has not moved.
    unsafe { bytes_mut(v.as_ptr())[a..a + bytes.len()].copy_from_slice(&bytes) };
    Ok(Value::int_unchecked(bytes.len() as i64))
}

fn u8_ready(vm: &mut Vm, args: usize, n: usize) -> R {
    let p = binary_input(vm, args, n, 0)?;
    let port = p.borrow();
    Ok(Value::bool(match &*port {
        Port::BytesIn { bytes, pos, source } => *pos < bytes.len() || source.is_none(),
        _ => return Err(Error::new("u8-ready?: not an open binary input port")),
    }))
}

fn write_u8(vm: &mut Vm, args: usize, n: usize) -> R {
    let b = byte_arg(arg(vm, args, 0), "write-u8")?;
    output_port(vm, args, n, 1)?.borrow_mut().write_bytes(&[b])?;
    Ok(Value::VOID)
}

/// `(write-bytevector bv [port start end])`.
fn write_bytevector(vm: &mut Vm, args: usize, n: usize) -> R {
    let bytes = bytes_arg(arg(vm, args, 0), "write-bytevector")?;
    let (a, b) = range_args(vm, args, n, 2, bytes.len(), "write-bytevector")?;
    let bytes = bytes[a..b].to_vec();
    output_port(vm, args, n, 1)?.borrow_mut().write_bytes(&bytes)?;
    Ok(Value::VOID)
}

fn close_port(vm: &mut Vm, args: usize, _: usize) -> R {
    let p = port_arg(vm, arg(vm, args, 0))?;
    let mut port = p.borrow_mut();
    if let Port::FileOut(w) | Port::BinaryFileOut(w) = &mut *port {
        w.flush().map_err(io_error)?;
    }
    if !matches!(*port, Port::Stdout | Port::Stderr) {
        let (input, binary) = (port.is_input(), port.is_binary());
        *port = Port::Closed { input, binary };
    }
    Ok(Value::VOID)
}

fn port_test(vm: &mut Vm, args: usize, test: fn(&Port) -> bool) -> R {
    let v = arg(vm, args, 0);
    Ok(Value::bool(vm.get::<PortRef>(v).is_ok_and(|p| test(&p.borrow()))))
}

fn write_string(vm: &mut Vm, args: usize, n: usize) -> R {
    let s = string(arg(vm, args, 0), "write-string")?;
    let len = s.chars().count();
    let bound = |vm: &Vm, i: usize, default: usize| -> Result<usize, Error> {
        if n > i { Ok(crate::num::integer(arg(vm, args, i), "write-string")?.clamp(0, len as i64) as usize) } else { Ok(default) }
    };
    let (start, end) = (bound(vm, 2, 0)?, bound(vm, 3, len)?);
    let part: String = s.chars().skip(start).take(end.saturating_sub(start)).collect();
    let p = (n > 1).then(|| arg(vm, args, 1));
    write_out(vm, p, &part)?;
    Ok(Value::VOID)
}

/// Flushes the port argument, or the current output port.
fn flush(vm: &mut Vm, args: usize, n: usize) -> R {
    vm.flush();
    let port = if n > 0 { Some(arg(vm, args, 0)) } else { parameterized(vm, OUTPUT_PORT_KEY) };
    if let Some(p) = port {
        let p = port_arg(vm, p)?;
        if let Port::FileOut(w) | Port::BinaryFileOut(w) = &mut *p.borrow_mut() {
            w.flush().map_err(io_error)?;
        }
    }
    Ok(Value::VOID)
}

fn file_error(path: &str, e: std::io::Error) -> Error {
    Error::new(format!("{path}: {e}")).with_kind(ErrorKind::File)
}

fn open_input_file(vm: &mut Vm, args: usize, _: usize) -> R {
    let path = string(arg(vm, args, 0), "open-input-file")?;
    let f = File::open(&path).map_err(|e| file_error(&path, e))?;
    make_port(vm, Port::In { text: String::new(), pos: 0, source: Some(Box::new(BufReader::new(f))) })
}

fn open_output_file(vm: &mut Vm, args: usize, _: usize) -> R {
    let path = string(arg(vm, args, 0), "open-output-file")?;
    let f = File::create(&path).map_err(|e| file_error(&path, e))?;
    make_port(vm, Port::FileOut(BufWriter::new(f)))
}

fn open_binary_input_file(vm: &mut Vm, args: usize, _: usize) -> R {
    let path = string(arg(vm, args, 0), "open-binary-input-file")?;
    let f = File::open(&path).map_err(|e| file_error(&path, e))?;
    make_port(vm, Port::BytesIn { bytes: Vec::new(), pos: 0, source: Some(Box::new(f)) })
}

fn open_binary_output_file(vm: &mut Vm, args: usize, _: usize) -> R {
    let path = string(arg(vm, args, 0), "open-binary-output-file")?;
    let f = File::create(&path).map_err(|e| file_error(&path, e))?;
    make_port(vm, Port::BinaryFileOut(BufWriter::new(f)))
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
        "%output-port-key" 0 0 => |_: &mut Vm, _, _| Ok(Value::int_unchecked(OUTPUT_PORT_KEY));
        "%input-port-key" 0 0 => |_: &mut Vm, _, _| Ok(Value::int_unchecked(INPUT_PORT_KEY));
        "%stdout" 0 0 => |vm: &mut Vm, _, _| make_port(vm, Port::Stdout);
        "%stderr" 0 0 => |vm: &mut Vm, _, _| make_port(vm, Port::Stderr);
        "open-output-string" 0 0 => |vm: &mut Vm, _, _| make_port(vm, Port::StringOut(String::new()));
        "open-input-string" 1 1 => |vm: &mut Vm, a, _| {
            let text = string(arg(vm, a, 0), "open-input-string")?;
            make_port(vm, Port::In { text, pos: 0, source: None }) };
        "get-output-string" 1 1 => get_output_string;
        "close-port" 1 1 => close_port;
        "close-input-port" 1 1 => close_port;
        "close-output-port" 1 1 => close_port;
        "port?" 1 1 => |vm: &mut Vm, a, _| port_test(vm, a, |_| true);
        "input-port?" 1 1 => |vm: &mut Vm, a, _| port_test(vm, a, Port::is_input);
        "output-port?" 1 1 => |vm: &mut Vm, a, _| port_test(vm, a, |p| !p.is_input());
        "textual-port?" 1 1 => |vm: &mut Vm, a, _| port_test(vm, a, |p| !p.is_binary());
        "binary-port?" 1 1 => |vm: &mut Vm, a, _| port_test(vm, a, Port::is_binary);
        "input-port-open?" 1 1 => |vm: &mut Vm, a, _| port_test(vm, a, |p| matches!(p, Port::In { .. } | Port::BytesIn { .. }));
        "open-input-bytevector" 1 1 => |vm: &mut Vm, a, _| {
            let bytes = bytes_arg(arg(vm, a, 0), "open-input-bytevector")?.to_vec();
            make_port(vm, Port::BytesIn { bytes, pos: 0, source: None }) };
        "open-output-bytevector" 0 0 => |vm: &mut Vm, _, _| make_port(vm, Port::BytesOut(Vec::new()));
        "get-output-bytevector" 1 1 => get_output_bytevector;
        "read-u8" 0 1 => |vm: &mut Vm, a, n| read_u8(vm, a, n, true);
        "peek-u8" 0 1 => |vm: &mut Vm, a, n| read_u8(vm, a, n, false);
        "u8-ready?" 0 1 => u8_ready;
        "read-bytevector" 1 2 => read_bytevector;
        "read-bytevector!" 1 4 => read_bytevector_into;
        "write-u8" 1 2 => write_u8;
        "write-bytevector" 1 4 => write_bytevector;
        "output-port-open?" 1 1 => |vm: &mut Vm, a, _| port_test(vm, a, |p| !p.is_input() && !matches!(p, Port::Closed { .. }));
        "read-line" 0 1 => read_line;
        "read-char" 0 1 => |vm: &mut Vm, a, n| read_char(vm, a, n, true);
        "peek-char" 0 1 => |vm: &mut Vm, a, n| read_char(vm, a, n, false);
        "char-ready?" 0 1 => char_ready;
        "read-string" 1 2 => read_string;
        "read" 0 1 => read_datum;
        "read-string-all" 0 1 => read_all;
        "write-string" 1 4 => write_string;
        "write-char" 1 2 => |vm: &mut Vm, a, n| {
            let c = arg(vm, a, 0);
            if !c.is_char() { return Err(type_error("write-char", "char", c)) }
            let p = (n > 1).then(|| arg(vm, a, 1));
            write_out(vm, p, c.as_char().encode_utf8(&mut [0; 4]))?; Ok(Value::VOID) };
        "write-shared" 1 2 => |vm: &mut Vm, a, n| { let p = (n > 1).then(|| arg(vm, a, 1)); write_shared(vm, arg(vm, a, 0), p, true, false, Sharing::All)?; Ok(Value::VOID) };
        "write-simple" 1 2 => |vm: &mut Vm, a, n| { let p = (n > 1).then(|| arg(vm, a, 1)); write_shared(vm, arg(vm, a, 0), p, true, false, Sharing::None)?; Ok(Value::VOID) };
        "eof-object" 0 0 => |_: &mut Vm, _, _| Ok(Value::EOF);
        "eof-object?" 1 1 => |vm: &mut Vm, a, _| Ok(Value::bool(arg(vm, a, 0) == Value::EOF));
        "flush-output" 0 1 => flush;
        "flush-output-port" 0 1 => flush;
        // Stdin as a port: a line at a time, when a reader asks. It takes the
        // stdin lock only while reading, so several VMs can each have one.
        "%make-stdin" 0 0 => |vm: &mut Vm, _, _| make_port(vm, Port::In { text: String::new(), pos: 0, source: Some(Box::new(BufReader::new(std::io::stdin()))) });
    }
    vm.requiring(Capability::Files, |vm| {
        natives! { vm;
            "open-input-file" 1 1 => open_input_file;
            "open-output-file" 1 1 => open_output_file;
            "open-binary-input-file" 1 1 => open_binary_input_file;
            "open-binary-output-file" 1 1 => open_binary_output_file;
        }
    });
}
