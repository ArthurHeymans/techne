//! Encodings, digests and MACs for `(techne crypto)`: base64 (RFC 4648,
//! standard and URL-safe), hex, SHA-256 and SHA-512 (FIPS 180-4), HMAC
//! (RFC 2104), comparison in constant time, over RustCrypto's crates, and
//! random bytes from the operating system, which need entropy granted.
//!
//! The natives are internal (`%name`); the library names them. Data is a
//! bytevector, or a string taken as its UTF-8. Results are data: digests
//! and MACs are bytevectors, encodings strings. Each goes through its
//! input a chunk at a time and stops between chunks on an interrupt;
//! encodings admit what they make first.

use std::sync::atomic::Ordering;

use base64::{
    Engine, alphabet,
    engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
};
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256, Sha512};
use subtle::ConstantTimeEq;

use crate::{
    builtins::{index_arg, string_arg, type_error},
    bytes::bytes_arg,
    heap::{Kind, is_kind},
    value::Value,
    vm::{Capability, Error, Vm},
};

type R = Result<Value, Error>;

fn arg(vm: &Vm, args: usize, i: usize) -> Value {
    vm.regs[args + i]
}

/// The bytes of data `v`, a bytevector or a string (its UTF-8), valid until
/// the next allocation.
fn data_arg<'a>(v: Value, who: &str) -> Result<&'a [u8], Error> {
    if is_kind(v, Kind::String) {
        string_arg(v, who)
    } else if is_kind(v, Kind::Bytevector) {
        bytes_arg(v, who)
    } else {
        Err(type_error(who, "bytevector or string", v))
    }
}

/// Standard base64, padded.
const STANDARD: GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// URL-safe base64, written without padding and read so.
const URL: GeneralPurpose = GeneralPurpose::new(
    &alphabet::URL_SAFE,
    GeneralPurposeConfig::new().with_encode_padding(false).with_decode_padding_mode(DecodePaddingMode::RequireNone),
);

/// URL-safe base64 read with its padding, which must then be whole.
const URL_PADDED: GeneralPurpose =
    GeneralPurpose::new(&alphabet::URL_SAFE, GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::RequireCanonical));

/// How many bytes are worked through between checks for an interrupt.
const CHUNK: usize = 1 << 16;

#[derive(Clone, Copy)]
enum Algorithm {
    Sha256,
    Sha512,
}

impl Algorithm {
    /// Its block size: HMAC hashes longer keys first.
    fn block(self) -> usize {
        match self {
            Algorithm::Sha256 => 64,
            Algorithm::Sha512 => 128,
        }
    }
}

fn algorithm(v: Value, who: &str) -> Result<Algorithm, Error> {
    match v.is_symbol().then(|| crate::reader::symbol_name(v.as_symbol())).as_deref() {
        Some("sha-256") => Ok(Algorithm::Sha256),
        Some("sha-512") => Ok(Algorithm::Sha512),
        _ => Err(type_error(who, "sha-256 or sha-512", v)),
    }
}

/// Why a pass over data ended early.
enum Fail {
    Stopped,
    Bad(String),
}

/// Whether a stop is pending for the execution running: the attention flag
/// alone is not one, since the allocator raises it too, past the world's
/// memory limit, for the VM to look at (as `Meter` does).
fn stopping(vm: &Vm) -> impl Fn() -> bool + '_ {
    let exec = vm.running_execution();
    move || vm.interrupt.load(Ordering::Relaxed) && vm.stops.pending(exec).is_some()
}

/// Give `f` the chunks of `data`, `size` bytes each, with the offset of
/// each, checking `stop` before each.
fn chunked(data: &[u8], size: usize, stop: &dyn Fn() -> bool, mut f: impl FnMut(usize, &[u8]) -> Result<(), String>) -> Result<(), Fail> {
    data.chunks(size).enumerate().try_for_each(|(i, c)| if stop() { Err(Fail::Stopped) } else { f(i * size, c).map_err(Fail::Bad) })
}

/// Byte `b` of a text, for an error: as a character if it is a visible
/// ASCII one, else in hexadecimal.
fn shown(b: u8) -> String {
    if b.is_ascii_graphic() { format!("{:?}", (b as char).to_string()) } else { format!("byte {b:#04x}") }
}

/// The error a native returns for `fail`.
fn failure(vm: &mut Vm, who: &str, fail: Fail) -> Error {
    match fail {
        Fail::Stopped => vm.take_interrupt(),
        Fail::Bad(why) => Error::new(format!("{who}: {why}")),
    }
}

fn digest_of<D: Digest>(data: &[u8], stop: &dyn Fn() -> bool) -> Result<Vec<u8>, Fail> {
    let mut d = D::new();
    chunked(data, CHUNK, stop, |_, c| {
        d.update(c);
        Ok(())
    })?;
    Ok(d.finalize().to_vec())
}

fn hmac_of<M: Mac + KeyInit>(key: &[u8], data: &[u8], stop: &dyn Fn() -> bool) -> Result<Vec<u8>, Fail> {
    let mut m = <M as KeyInit>::new_from_slice(key).expect("HMAC takes any key");
    chunked(data, CHUNK, stop, |_, c| {
        m.update(c);
        Ok(())
    })?;
    Ok(m.finalize().into_bytes().to_vec())
}

fn digest(alg: Algorithm, data: &[u8], stop: &dyn Fn() -> bool) -> Result<Vec<u8>, Fail> {
    match alg {
        Algorithm::Sha256 => digest_of::<Sha256>(data, stop),
        Algorithm::Sha512 => digest_of::<Sha512>(data, stop),
    }
}

fn hmac(alg: Algorithm, key: &[u8], data: &[u8], stop: &dyn Fn() -> bool) -> Result<Vec<u8>, Fail> {
    // A key longer than a block stands for its digest (RFC 2104), made
    // here, where it can be stopped, rather than by `new_from_slice`.
    let hashed;
    let key = if key.len() > alg.block() {
        hashed = digest(alg, key, stop)?;
        &hashed[..]
    } else {
        key
    };
    match alg {
        Algorithm::Sha256 => hmac_of::<Hmac<Sha256>>(key, data, stop),
        Algorithm::Sha512 => hmac_of::<Hmac<Sha512>>(key, data, stop),
    }
}

/// `(%digest algorithm data)`: the digest of DATA by ALGORITHM, `sha-256`
/// or `sha-512`.
fn digest_native(vm: &mut Vm, args: usize, _: usize) -> R {
    let who = "digest";
    let alg = algorithm(arg(vm, args, 0), who)?;
    let data = data_arg(arg(vm, args, 1), who)?;
    let d = digest(alg, data, &stopping(vm));
    let d = d.map_err(|f| failure(vm, who, f))?;
    Ok(vm.make_bytevector_raw(&d))
}

/// `(%hmac algorithm key data)`: the HMAC of DATA under KEY.
fn hmac_native(vm: &mut Vm, args: usize, _: usize) -> R {
    let who = "hmac";
    let alg = algorithm(arg(vm, args, 0), who)?;
    let key = data_arg(arg(vm, args, 1), who)?;
    let data = data_arg(arg(vm, args, 2), who)?;
    let m = hmac(alg, key, data, &stopping(vm));
    let m = m.map_err(|f| failure(vm, who, f))?;
    Ok(vm.make_bytevector_raw(&m))
}

// Encodings make their output in Rust, then copy it to the heap: they
// admit both, and take their input again after admitting, which may
// collect and move it.

/// `(%base64-encode data url)`: DATA in base64, URL-safe and unpadded with
/// URL.
fn base64_encode(vm: &mut Vm, args: usize, _: usize) -> R {
    let who = "base64-encode";
    let len = data_arg(arg(vm, args, 0), who)?.len().div_ceil(3) * 4;
    vm.admit_items(len, 2, 64)?;
    let data = data_arg(arg(vm, args, 0), who)?;
    let engine = if arg(vm, args, 1).is_truthy() { &URL } else { &STANDARD };
    let mut text = String::with_capacity(len);
    // Whole groups of three bytes, so that only the last chunk pads.
    let done = chunked(data, 3 * CHUNK, &stopping(vm), |_, c| {
        engine.encode_string(c, &mut text);
        Ok(())
    });
    done.map_err(|f| failure(vm, who, f))?;
    Ok(vm.make_string_raw(&text))
}

/// `(%base64-decode text url)`: the bytes TEXT encodes in base64 (URL-safe
/// with URL, its padding optional but whole if there); anything else in
/// TEXT, whitespace too, or bits past the last byte, is an error.
fn base64_decode(vm: &mut Vm, args: usize, _: usize) -> R {
    use base64::DecodeError;
    let who = "base64-decode";
    let len = data_arg(arg(vm, args, 0), who)?.len() / 4 * 3 + 3;
    vm.admit_items(len, 2, 64)?;
    let text = data_arg(arg(vm, args, 0), who)?;
    let engine = match (arg(vm, args, 1).is_truthy(), text.last()) {
        (false, _) => &STANDARD,
        (true, Some(b'=')) => &URL_PADDED,
        (true, _) => &URL,
    };
    let mut bytes = Vec::with_capacity(len);
    // Whole groups of four digits: only the last chunk may pad, which each
    // chunk alone would not know.
    let done = chunked(text, 4 * CHUNK, &stopping(vm), |at, c| {
        if at + c.len() < text.len()
            && let Some(i) = c.iter().position(|&b| b == b'=')
        {
            return Err(format!("\"=\" at {} is not a base64 digit there", at + i));
        }
        engine.decode_vec(c, &mut bytes).map_err(|e| match e {
            DecodeError::InvalidByte(i, b) => format!("{} at {} is not a base64 digit there", shown(b), at + i),
            DecodeError::InvalidLastSymbol { offset, symbol, .. } => {
                format!("{} at {} has bits past the last byte", shown(symbol), at + offset)
            }
            DecodeError::InvalidLength(_) => format!("{} digits make no whole bytes", text.len()),
            DecodeError::InvalidPadding => "bad padding".into(),
        })
    });
    done.map_err(|f| failure(vm, who, f))?;
    Ok(vm.make_bytevector_raw(&bytes))
}

/// `(%hex-encode data)`: DATA as lowercase hexadecimal.
fn hex_encode(vm: &mut Vm, args: usize, _: usize) -> R {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let who = "hex-encode";
    let len = data_arg(arg(vm, args, 0), who)?.len() * 2;
    vm.admit_items(len, 2, 64)?;
    let data = data_arg(arg(vm, args, 0), who)?;
    let mut text = String::with_capacity(len);
    let done = chunked(data, CHUNK, &stopping(vm), |_, c| {
        text.extend(c.iter().flat_map(|b| [DIGITS[(b >> 4) as usize] as char, DIGITS[(b & 15) as usize] as char]));
        Ok(())
    });
    done.map_err(|f| failure(vm, who, f))?;
    Ok(vm.make_string_raw(&text))
}

/// `(%hex-decode text)`: the bytes TEXT writes in hexadecimal, either case.
fn hex_decode(vm: &mut Vm, args: usize, _: usize) -> R {
    let who = "hex-decode";
    let len = data_arg(arg(vm, args, 0), who)?.len();
    if len % 2 != 0 {
        return Err(Error::new(format!("{who}: an odd number of digits ({len})")));
    }
    vm.admit_items(len / 2, 2, 64)?;
    let text = data_arg(arg(vm, args, 0), who)?;
    let mut bytes = Vec::with_capacity(len / 2);
    let done = chunked(text, CHUNK, &stopping(vm), |at, c| {
        let digit = |i: usize| match c[i] {
            d @ b'0'..=b'9' => Ok(d - b'0'),
            d @ (b'a'..=b'f' | b'A'..=b'F') => Ok((d | 0x20) - b'a' + 10),
            d => Err(format!("{} at {} is not a hexadecimal digit", shown(d), at + i)),
        };
        (0..c.len()).step_by(2).try_for_each(|i| {
            bytes.push(digit(i)? << 4 | digit(i + 1)?);
            Ok(())
        })
    });
    done.map_err(|f| failure(vm, who, f))?;
    Ok(vm.make_bytevector_raw(&bytes))
}

/// `(%digest=? a b)`: whether A and B have the same bytes, in time that
/// depends on their lengths only.
fn digest_eq(vm: &mut Vm, args: usize, _: usize) -> R {
    let who = "digest=?";
    let a = data_arg(arg(vm, args, 0), who)?;
    let b = data_arg(arg(vm, args, 1), who)?;
    if a.len() != b.len() {
        return Ok(Value::FALSE);
    }
    let same = {
        let stop = stopping(vm);
        a.chunks(CHUNK).zip(b.chunks(CHUNK)).try_fold(subtle::Choice::from(1), |same, (x, y)| (!stop()).then(|| same & x.ct_eq(y)))
    };
    match same {
        Some(same) => Ok(Value::bool(same.into())),
        None => Err(vm.take_interrupt()),
    }
}

/// `(%random-bytes n)`: N bytes from the operating system's source of
/// randomness.
fn random_bytes(vm: &mut Vm, args: usize, _: usize) -> R {
    let who = "random-bytes";
    vm.need(Capability::Entropy, who)?;
    let n = index_arg(arg(vm, args, 0), who)?;
    vm.admit_items(n, 2, 64)?;
    let mut bytes = vec![0; n];
    let filled = {
        let stop = stopping(vm);
        bytes
            .chunks_mut(CHUNK)
            .try_for_each(|chunk| if stop() { Err(Fail::Stopped) } else { getrandom::fill(chunk).map_err(|e| Fail::Bad(e.to_string())) })
    };
    filled.map_err(|f| failure(vm, who, f))?;
    Ok(vm.make_bytevector_raw(&bytes))
}

pub fn install(vm: &mut Vm) {
    crate::natives! { vm;
        "(%digest algorithm data)" => digest_native;
        "(%hmac algorithm key data)" => hmac_native;
        "(%base64-encode data url)" => base64_encode;
        "(%base64-decode text url)" => base64_decode;
        "(%hex-encode data)" => hex_encode;
        "(%hex-decode text)" => hex_decode;
        "(%digest=? a b)" => digest_eq;
        "(%random-bytes n)" => random_bytes;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// The allocator raises the attention flag past the world's memory
    /// limit, with no stop pending: hashing goes on.
    #[test]
    fn the_attention_flag_alone_stops_nothing() {
        let mut vm = Vm::new();
        let _world = vm.enter_account();
        let data = vm.make_bytevector_raw(&vec![0; 4 * CHUNK]);
        vm.regs[0] = Value::symbol(crate::reader::intern("sha-256"));
        vm.regs[1] = data;
        vm.memory_waker()();
        assert!(vm.interrupt.load(Ordering::SeqCst));
        assert!(digest_native(&mut vm, 0, 2).is_ok());
        vm.regs[0] = data;
        assert!(hex_encode(&mut vm, 0, 1).is_ok());
        vm.regs[0] = Value::int_unchecked(4 * CHUNK as i64);
        assert!(random_bytes(&mut vm, 0, 1).is_ok());
    }

    #[test]
    fn long_inputs_are_hashed_in_chunks_that_an_interrupt_stops() {
        let data = vec![0u8; 4 * CHUNK + 1];
        let (stop, go): (&dyn Fn() -> bool, &dyn Fn() -> bool) = (&|| true, &|| false);
        let stopped = |r: Result<Vec<u8>, Fail>| matches!(r, Err(Fail::Stopped));
        assert!(stopped(digest(Algorithm::Sha256, &data, stop)));
        assert!(stopped(hmac(Algorithm::Sha512, b"k", &data, stop)));
        // A long key is hashed where it can be stopped too.
        assert!(stopped(hmac(Algorithm::Sha256, &data, b"", stop)));
        // Chunks hash as the whole does, and a long key as its digest.
        let ok = |r: Result<Vec<u8>, Fail>| r.ok().unwrap();
        assert_eq!(ok(digest(Algorithm::Sha256, &data, go)), Sha256::digest(&data).to_vec());
        let mut m = <Hmac<Sha256> as KeyInit>::new_from_slice(&data).unwrap();
        m.update(b"x");
        assert_eq!(ok(hmac(Algorithm::Sha256, &data, b"x", go)), m.finalize().into_bytes().to_vec());
        assert_eq!(hex(&ok(digest(Algorithm::Sha256, b"", go))), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }
}
