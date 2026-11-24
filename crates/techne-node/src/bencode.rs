//! Bencode, nREPL's wire format: integers, byte strings, lists and
//! dictionaries with byte-string keys (kept sorted when encoding).

use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub enum B {
    Int(i64),
    Bytes(Vec<u8>),
    List(Vec<B>),
    Dict(BTreeMap<Vec<u8>, B>),
}

impl B {
    pub fn str(s: impl AsRef<str>) -> B {
        B::Bytes(s.as_ref().as_bytes().to_vec())
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            B::Bytes(b) => std::str::from_utf8(b).ok(),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            B::Int(i) => Some(*i),
            // Some clients send numbers as strings.
            B::Bytes(_) => self.as_str()?.trim().parse().ok(),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&B> {
        match self {
            B::Dict(d) => d.get(key.as_bytes()),
            _ => None,
        }
    }

    pub fn encode(&self, out: &mut Vec<u8>) {
        match self {
            B::Int(i) => {
                out.push(b'i');
                push_i64_decimal(out, *i);
                out.push(b'e');
            }
            B::Bytes(b) => {
                push_u64_decimal(out, b.len() as u64);
                out.push(b':');
                out.extend_from_slice(b);
            }
            B::List(items) => {
                out.push(b'l');
                items.iter().for_each(|i| i.encode(out));
                out.push(b'e');
            }
            B::Dict(d) => {
                out.push(b'd');
                for (k, v) in d {
                    B::Bytes(k.clone()).encode(out);
                    v.encode(out);
                }
                out.push(b'e');
            }
        }
    }
}

fn push_u64_decimal(out: &mut Vec<u8>, mut n: u64) {
    let mut digits = [0u8; 20];
    let mut i = digits.len();
    loop {
        i -= 1;
        let q = n / 10;
        digits[i] = b'0' + (n - q * 10) as u8;
        n = q;
        if n == 0 {
            break;
        }
    }
    out.extend_from_slice(&digits[i..]);
}

fn push_i64_decimal(out: &mut Vec<u8>, n: i64) {
    if n < 0 {
        out.push(b'-');
    }
    push_u64_decimal(out, n.unsigned_abs());
}

/// A dictionary from string keys.
pub fn dict<'a>(entries: impl IntoIterator<Item = (&'a str, B)>) -> B {
    B::Dict(entries.into_iter().map(|(k, v)| (k.as_bytes().to_vec(), v)).collect())
}

const MAX_DEPTH: usize = 64;

#[derive(Debug, PartialEq, Eq)]
enum NumberError<'a> {
    TooLong,
    BadBytes(&'a [u8]),
}

impl NumberError<'_> {
    fn into_string(self) -> String {
        match self {
            NumberError::TooLong => "number too long".into(),
            NumberError::BadBytes(raw) => match std::str::from_utf8(raw) {
                Ok(text) => format!("bad number {text:?}"),
                Err(_) => "bad number".into(),
            },
        }
    }
}

/// Parse a base-10 signed 64-bit integer from raw bytes with the exact semantics
/// of `std::str::from_utf8(bytes).ok().and_then(|s| s.parse::<i64>().ok())`.
fn parse_i64_bytes(bytes: &[u8]) -> Option<i64> {
    let (&first, rest) = bytes.split_first()?;
    let (neg, digits) = match first {
        b'-' => (true, rest),
        b'+' => (false, rest),
        _ => (false, bytes),
    };
    if digits.is_empty() {
        return None;
    }
    let mut acc: i64 = 0;
    for &b in digits {
        if !b.is_ascii_digit() {
            return None;
        }
        let d = i64::from(b - b'0');
        acc = acc.checked_mul(10)?;
        acc = if neg { acc.checked_sub(d)? } else { acc.checked_add(d)? };
    }
    Some(acc)
}

fn parse_number(buf: &[u8], end: u8) -> Result<Option<(i64, usize)>, NumberError<'_>> {
    let Some(len) = buf.iter().position(|&c| c == end) else {
        return if buf.len() > 20 { Err(NumberError::TooLong) } else { Ok(None) };
    };
    let raw = &buf[..len];
    let n = parse_i64_bytes(raw).ok_or(NumberError::BadBytes(raw))?;
    Ok(Some((n, len + 1)))
}

#[derive(Debug, PartialEq, Eq)]
enum BytesError<'a> {
    Number(NumberError<'a>),
    NegativeLength,
    TooLong(usize),
}

impl BytesError<'_> {
    fn into_string(self) -> String {
        match self {
            BytesError::Number(e) => e.into_string(),
            BytesError::NegativeLength => "negative length".into(),
            BytesError::TooLong(len) => format!("string of {len} bytes is too long"),
        }
    }
}

/// Parse a bencode byte-string header and check that its payload fits in `buf`,
/// returning `(start, end)` offsets into `buf` when complete.
fn parse_bytes_range(buf: &[u8]) -> Result<Option<(usize, usize)>, BytesError<'_>> {
    let Some((len, used)) = parse_number(buf, b':').map_err(BytesError::Number)? else {
        return Ok(None);
    };
    let len = usize::try_from(len).map_err(|_| BytesError::NegativeLength)?;
    if len > crate::protocol::MAX_FRAME {
        return Err(BytesError::TooLong(len));
    }
    Ok((buf.len() >= used + len).then_some((used, used + len)))
}

/// Parse one value from the start of `buf`: `Ok(None)` if more bytes are
/// needed, otherwise the value and how many bytes it used.
pub fn parse(buf: &[u8]) -> Result<Option<(B, usize)>, String> {
    fn value(buf: &[u8], depth: usize) -> Result<Option<(B, usize)>, String> {
        if depth > MAX_DEPTH {
            return Err("nesting too deep".into());
        }
        let Some(&first) = buf.first() else { return Ok(None) };
        match first {
            b'i' => Ok(parse_number(&buf[1..], b'e').map_err(NumberError::into_string)?.map(|(n, used)| (B::Int(n), 1 + used))),
            b'0'..=b'9' => {
                let Some((start, end)) = parse_bytes_range(buf).map_err(BytesError::into_string)? else {
                    return Ok(None);
                };
                Ok(Some((B::Bytes(buf[start..end].to_vec()), end)))
            }
            b'l' | b'd' => {
                let mut pos = 1;
                let mut items = Vec::new();
                loop {
                    match buf.get(pos) {
                        None => return Ok(None),
                        Some(b'e') => break,
                        Some(_) => {
                            let Some((v, used)) = value(&buf[pos..], depth + 1)? else { return Ok(None) };
                            items.push(v);
                            pos += used;
                        }
                    }
                }
                let v = if first == b'l' {
                    B::List(items)
                } else {
                    if items.len() % 2 != 0 {
                        return Err("dictionary with a key and no value".into());
                    }
                    let mut d = BTreeMap::new();
                    let mut it = items.into_iter();
                    while let (Some(k), Some(v)) = (it.next(), it.next()) {
                        let B::Bytes(k) = k else { return Err("dictionary key is not a string".into()) };
                        d.insert(k, v);
                    }
                    B::Dict(d)
                };
                Ok(Some((v, pos + 1)))
            }
            c => Err(format!("unexpected byte {c:#x}")),
        }
    }
    value(buf, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let v = dict([("op", B::str("eval")), ("id", B::Int(7)), ("args", B::List(vec![B::str("a"), dict([])]))]);
        let mut bytes = Vec::new();
        v.encode(&mut bytes);
        assert_eq!(bytes, b"d4:argsl1:adee2:idi7e2:op4:evale");
        assert_eq!(parse(&bytes).unwrap(), Some((v, bytes.len())));
        // Incomplete input asks for more; garbage is an error.
        assert_eq!(parse(&bytes[..bytes.len() - 1]).unwrap(), None);
        assert_eq!(parse(b"5:ab").unwrap(), None);
        assert!(parse(b"x").is_err());
        assert!(parse(b"d1:ae").is_err());
    }

    #[test]
    fn decimal_roundtrip_and_parse_i64_bytes_match_std() {
        for n in [i64::MIN, i64::MIN + 1, -1, 0, 1, i64::MAX - 1, i64::MAX] {
            let mut out = Vec::new();
            B::Int(n).encode(&mut out);
            assert_eq!(out, format!("i{n}e").into_bytes());
            assert_eq!(parse(&out).unwrap(), Some((B::Int(n), out.len())));
        }
        for sample in [
            b"".as_slice(),
            b"-",
            b"+",
            b"0",
            b"-0",
            b"+0",
            b"+42",
            b"-9223372036854775808",
            b"-9223372036854775809",
            b"9223372036854775807",
            b"9223372036854775808",
            b" 1",
            b"1_0",
            b"\xff",
        ] {
            let expected = std::str::from_utf8(sample).ok().and_then(|s| s.parse::<i64>().ok());
            assert_eq!(parse_i64_bytes(sample), expected, "sample {sample:?}");
        }
    }
}

#[cfg(kani)]
mod kani_proofs {
    use super::*;

    /// Bounded over all `i8` values (`-128..=127`, covering sign, zero, and 1..=3 digit
    /// widths): `push_i64_decimal` round-trips through `parse_number`, every strict prefix
    /// returns `Ok(None)`, and any trailing byte is left unconsumed.
    #[kani::proof]
    #[kani::unwind(6)]
    fn decimal_encode_parse_number_roundtrip_i8() {
        let n = i64::from(kani::any::<i8>());
        let mut buf = Vec::with_capacity(8);
        push_i64_decimal(&mut buf, n);
        buf.push(b'e');
        let orig_len = buf.len();

        let mode: u8 = kani::any();
        kani::assume(mode < 3);
        let slice_len = match mode {
            0 => orig_len,
            1 => {
                let prefix_len: usize = kani::any();
                kani::assume(prefix_len < orig_len);
                prefix_len
            }
            _ => {
                let tail: u8 = kani::any();
                buf.push(tail);
                orig_len + 1
            }
        };

        match (mode, parse_number(&buf[..slice_len], b'e')) {
            (0 | 2, Ok(Some((got, used)))) => {
                assert!(got == n);
                assert!(used == orig_len);
            }
            (1, Ok(None)) => {}
            _ => panic!("unexpected parse_number result for encoded i8"),
        }
    }

    /// Exact boundary checks for `i64::MIN`, `i64::MAX`, `i64::MAX + 1`, `i64::MIN - 1`,
    /// `MAX_FRAME`, `MAX_FRAME + 1`, and negative byte-string lengths.
    #[kani::proof]
    #[kani::unwind(24)]
    fn parse_number_and_bytes_range_exact_boundaries() {
        let mut min_buf = Vec::with_capacity(22);
        push_i64_decimal(&mut min_buf, i64::MIN);
        min_buf.push(b'e');
        assert!(matches!(parse_number(&min_buf, b'e'), Ok(Some((i64::MIN, 21)))));
        assert!(matches!(parse_number(&min_buf[..20], b'e'), Ok(None)));

        let mut max_buf = Vec::with_capacity(22);
        push_i64_decimal(&mut max_buf, i64::MAX);
        max_buf.push(b'e');
        assert!(matches!(parse_number(&max_buf, b'e'), Ok(Some((i64::MAX, 20)))));

        // One past i64::MAX and one below i64::MIN are rejected as BadBytes.
        assert!(matches!(parse_number(b"9223372036854775808e", b'e'), Err(NumberError::BadBytes(_))));
        assert!(matches!(parse_number(b"-9223372036854775809e", b'e'), Err(NumberError::BadBytes(_))));

        // Exact MAX_FRAME (16 MiB = 16_777_216) header with no payload yet asks for more bytes.
        assert!(matches!(parse_bytes_range(b"16777216:"), Ok(None)));
        // MAX_FRAME + 1 is rejected immediately.
        assert!(matches!(parse_bytes_range(b"16777217:"), Err(BytesError::TooLong(16_777_217))));
        // Negative lengths and i64::MAX + 1 overflow are rejected.
        assert!(matches!(parse_bytes_range(b"-1:"), Err(BytesError::NegativeLength)));
        assert!(matches!(parse_bytes_range(b"9223372036854775808:"), Err(BytesError::Number(NumberError::BadBytes(_)))));
    }

    /// Independent unsigned-magnitude reference oracle for base-10 signed 64-bit integers.
    /// Unlike `parse_i64_bytes` (which accumulates a signed `i64` via `checked_mul`/`checked_add`/
    /// `checked_sub`), this bounds the unsigned magnitude against `i64::MAX` / `|i64::MIN|`
    /// using the exact threshold `922_337_203_685_477_580` and negates via two's complement.
    fn oracle_parse_i64(raw: &[u8]) -> Option<i64> {
        const LIMIT_Q: u64 = 922_337_203_685_477_580;
        if raw.is_empty() {
            return None;
        }
        let (neg, start) = match raw[0] {
            b'-' => (true, 1),
            b'+' => (false, 1),
            _ => (false, 0),
        };
        if start == raw.len() {
            return None;
        }
        let limit_r = if neg { 8u64 } else { 7u64 };
        let mut mag: u64 = 0;
        let mut i = start;
        while i < raw.len() {
            let b = raw[i];
            if !b.is_ascii_digit() {
                return None;
            }
            let d = u64::from(b - b'0');
            if mag > LIMIT_Q || (mag == LIMIT_Q && d > limit_r) {
                return None;
            }
            mag = (mag << 3) + (mag << 1) + d;
            i += 1;
        }
        Some(if neg { mag.wrapping_neg() as i64 } else { mag as i64 })
    }

    /// Exhaustive over arbitrary byte slices up to 7 bytes with either bencode delimiter
    /// (`b'e'` or `b':'`): `parse_number` matches the independent unsigned-magnitude decimal
    /// oracle on both parsed values and exact error outcomes (including `+`/`-` signs,
    /// leading zeros, empty inputs, non-digits, and unterminated slices). Full 19..=21-byte
    /// 64-bit overflow boundaries and the 20-byte unterminated cap are verified separately.
    #[kani::proof]
    #[kani::unwind(9)]
    fn parse_number_matches_oracle_up_to_7() {
        let buf: [u8; 7] = kani::any();
        let len: usize = kani::any();
        kani::assume(len <= buf.len());
        let end = if kani::any() { b'e' } else { b':' };
        let slice = &buf[..len];

        let mut first_end = None;
        let mut i = 0;
        while i < len {
            if slice[i] == end {
                first_end = Some(i);
                break;
            }
            i += 1;
        }

        let got = parse_number(slice, end);
        match first_end {
            None => {
                assert!(matches!(got, Ok(None)));
            }
            Some(pos) => {
                let raw = &slice[..pos];
                match oracle_parse_i64(raw) {
                    Some(expected) => match got {
                        Ok(Some((v, used))) => {
                            assert!(v == expected);
                            assert!(used == pos + 1);
                            assert!(slice[used - 1] == end);
                        }
                        _ => panic!("valid decimal integer must parse to oracle value"),
                    },
                    None => match got {
                        Err(NumberError::BadBytes(err_raw)) => {
                            assert!(err_raw == raw);
                        }
                        _ => panic!("invalid decimal integer must fail with BadBytes"),
                    },
                }
            }
        }
    }

    /// Exhaustive over all unterminated byte slices up to 22 bytes and arbitrary delimiter
    /// `end`: `parse_number` enforces the exact 20-byte unterminated cap (`Ok(None)` for
    /// `<= 20`, `Err(TooLong)` for `> 20`).
    #[kani::proof]
    #[kani::unwind(24)]
    fn parse_number_unterminated_cap_up_to_22() {
        let buf: [u8; 22] = kani::any();
        let len: usize = kani::any();
        kani::assume(len <= buf.len());
        let end: u8 = kani::any();
        let slice = &buf[..len];

        let mut i = 0;
        while i < len {
            kani::assume(slice[i] != end);
            i += 1;
        }

        let got = parse_number(slice, end);
        if len > 20 {
            assert!(matches!(got, Err(NumberError::TooLong)));
        } else {
            assert!(matches!(got, Ok(None)));
        }
    }

    /// Exhaustive over arbitrary 2-byte suffixes around the 64-bit `i64::MIN`/`i64::MAX`
    /// overflow boundary (unsigned, `+`-signed, `-`-signed, and 21-byte zero-padded):
    /// `parse_i64_bytes` matches the independent unsigned-magnitude oracle on every valid
    /// value and every overflow/non-digit rejection.
    #[kani::proof]
    #[kani::unwind(24)]
    fn parse_i64_bytes_matches_oracle_at_64bit_boundaries() {
        let d1: u8 = kani::any();
        let d2: u8 = kani::any();
        let kind: u8 = kani::any();
        kani::assume(kind < 4);
        match kind {
            0 => {
                // Unsigned 19-byte near i64::MAX (92233720368547758??)
                let raw = [b'9', b'2', b'2', b'3', b'3', b'7', b'2', b'0', b'3', b'6', b'8', b'5', b'4', b'7', b'7', b'5', b'8', d1, d2];
                assert!(parse_i64_bytes(&raw) == oracle_parse_i64(&raw));
            }
            1 => {
                // '+'-signed 20-byte near i64::MAX (+92233720368547758??)
                let raw =
                    [b'+', b'9', b'2', b'2', b'3', b'3', b'7', b'2', b'0', b'3', b'6', b'8', b'5', b'4', b'7', b'7', b'5', b'8', d1, d2];
                assert!(parse_i64_bytes(&raw) == oracle_parse_i64(&raw));
            }
            2 => {
                // '-'-signed 20-byte near i64::MIN (-92233720368547758??)
                let raw =
                    [b'-', b'9', b'2', b'2', b'3', b'3', b'7', b'2', b'0', b'3', b'6', b'8', b'5', b'4', b'7', b'7', b'5', b'8', d1, d2];
                assert!(parse_i64_bytes(&raw) == oracle_parse_i64(&raw));
            }
            _ => {
                // Zero-padded 21-byte signed number (-000000000000000000??)
                let raw = [
                    b'-', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', b'0', d1,
                    d2,
                ];
                assert!(parse_i64_bytes(&raw) == oracle_parse_i64(&raw));
            }
        }
    }

    /// Symbolic 0..=4 byte payload round-trip through `push_u64_decimal` + `parse_bytes_range`,
    /// strict prefix incompleteness, and trailing byte preservation.
    #[kani::proof]
    #[kani::unwind(6)]
    fn parse_bytes_range_roundtrip_up_to_4_bytes() {
        let payload_len: usize = kani::any();
        kani::assume(payload_len <= 4);
        let p: [u8; 4] = kani::any();
        let mut buf = Vec::with_capacity(8);
        push_u64_decimal(&mut buf, payload_len as u64);
        buf.push(b':');
        let hdr_len = buf.len();
        buf.extend_from_slice(&p[..payload_len]);
        let full_len = buf.len();

        match parse_bytes_range(&buf) {
            Ok(Some((start, end))) => {
                assert!(start == hdr_len);
                assert!(end == full_len);
                assert!(&buf[start..end] == &p[..payload_len]);
            }
            _ => panic!("valid byte string must parse"),
        }

        let prefix_len: usize = kani::any();
        kani::assume(prefix_len < full_len);
        assert!(matches!(parse_bytes_range(&buf[..prefix_len]), Ok(None)));

        let tail: u8 = kani::any();
        buf.push(tail);
        match parse_bytes_range(&buf) {
            Ok(Some((start, end))) => {
                assert!(start == hdr_len && end == full_len);
            }
            _ => panic!("trailing byte must remain unconsumed"),
        }
    }

    /// Exhaustive over arbitrary byte slices up to 12 bytes: `parse_bytes_range` matches
    /// the independent unsigned-magnitude numeric oracle on both success ranges and every
    /// error variant.
    #[kani::proof]
    #[kani::unwind(16)]
    fn parse_bytes_range_arbitrary_bytes_up_to_12() {
        let buf: [u8; 12] = kani::any();
        let len: usize = kani::any();
        kani::assume(len <= buf.len());
        let slice = &buf[..len];

        let mut first_colon = None;
        let mut i = 0;
        while i < len {
            if slice[i] == b':' {
                first_colon = Some(i);
                break;
            }
            i += 1;
        }

        let got = parse_bytes_range(slice);
        match first_colon {
            None => {
                assert!(matches!(got, Ok(None)));
            }
            Some(pos) => {
                let raw = &slice[..pos];
                match oracle_parse_i64(raw) {
                    None => match got {
                        Err(BytesError::Number(NumberError::BadBytes(err_raw))) => {
                            assert!(err_raw == raw);
                        }
                        _ => panic!("invalid byte-string length must fail with BadBytes"),
                    },
                    Some(n) if n < 0 => {
                        assert!(matches!(got, Err(BytesError::NegativeLength)));
                    }
                    Some(n) => {
                        let need = n as usize;
                        if need > crate::protocol::MAX_FRAME {
                            match got {
                                Err(BytesError::TooLong(v)) => {
                                    assert!(v == need);
                                }
                                _ => panic!("length above MAX_FRAME must fail with TooLong"),
                            }
                        } else {
                            let start = pos + 1;
                            if len >= start + need {
                                match got {
                                    Ok(Some((s, e))) => {
                                        assert!(s == start);
                                        assert!(e == start + need);
                                    }
                                    _ => panic!("complete byte-string frame must return range"),
                                }
                            } else {
                                assert!(matches!(got, Ok(None)));
                            }
                        }
                    }
                }
            }
        }
    }
}
