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
            B::Int(i) => out.extend_from_slice(format!("i{i}e").as_bytes()),
            B::Bytes(b) => {
                out.extend_from_slice(format!("{}:", b.len()).as_bytes());
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

/// A dictionary from string keys.
pub fn dict<'a>(entries: impl IntoIterator<Item = (&'a str, B)>) -> B {
    B::Dict(entries.into_iter().map(|(k, v)| (k.as_bytes().to_vec(), v)).collect())
}

/// Parse one value from the start of `buf`: `Ok(None)` if more bytes are
/// needed, otherwise the value and how many bytes it used.
pub fn parse(buf: &[u8]) -> Result<Option<(B, usize)>, String> {
    fn number(buf: &[u8], end: u8) -> Result<Option<(i64, usize)>, String> {
        let Some(len) = buf.iter().position(|&c| c == end) else {
            return if buf.len() > 20 { Err("number too long".into()) } else { Ok(None) };
        };
        let text = std::str::from_utf8(&buf[..len]).map_err(|_| "bad number")?;
        let n = text.parse().map_err(|_| format!("bad number {text:?}"))?;
        Ok(Some((n, len + 1)))
    }
    fn value(buf: &[u8], depth: usize) -> Result<Option<(B, usize)>, String> {
        if depth > 64 {
            return Err("nesting too deep".into());
        }
        let Some(&first) = buf.first() else { return Ok(None) };
        match first {
            b'i' => Ok(number(&buf[1..], b'e')?.map(|(n, used)| (B::Int(n), 1 + used))),
            b'0'..=b'9' => {
                let Some((len, used)) = number(buf, b':')? else { return Ok(None) };
                let len = usize::try_from(len).map_err(|_| "negative length")?;
                if len > crate::protocol::MAX_FRAME {
                    return Err(format!("string of {len} bytes is too long"));
                }
                Ok((buf.len() >= used + len).then(|| (B::Bytes(buf[used..used + len].to_vec()), used + len)))
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
}
