//! What a supervisor and its worker say to each other over their socket:
//! frames, each a length (4 bytes, big-endian) and a message, `Data` in a
//! binary encoding of its own ([`encode`], [`decode`]). The supervisor
//! trusts nothing the worker says: a frame is bounded before it is read
//! (`MAX_FRAME`), decoding is bounded in depth and parts and needs no
//! world, and a message that is not one of those below, or names what it
//! was not given, is the worker's fault.

use std::io;

use techne_vm::data::Data;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The most bytes of a frame: a request or response of 16 MiB and its
/// encoding's overhead.
pub const MAX_FRAME: usize = 32 << 20;

/// The most bytes of a message of a REPL connection.
pub const REPL_CHUNK: usize = 64 << 10;

/// The most bytes of a deployment's files: they go in a frame.
pub const BUNDLE_BYTES: usize = 24 << 20;

/// How deep data nests in a message.
const MAX_DEPTH: usize = 64;

/// How many parts a message has, at most.
const MAX_PARTS: usize = 1 << 20;

/// What the supervisor says.
#[derive(Debug)]
pub enum ToWorker {
    /// Load a deployment from its files, publish it as `name`: for
    /// development, its dev world with a REPL, if `dev`.
    Deploy {
        id: u64,
        name: String,
        entry: String,
        files: Vec<(String, Vec<u8>)>,
        wall_ms: u64,
        dev: bool,
    },
    /// Serve `request` with the deployment published as `name`.
    Request {
        rid: u64,
        name: String,
        request: Data,
    },
    /// The request `rid` is abandoned.
    Cancel {
        rid: u64,
    },
    /// A REPL client connected, as connection `cid`; bytes from it; its
    /// end.
    ReplOpen {
        cid: u64,
    },
    ReplData {
        cid: u64,
        bytes: Vec<u8>,
    },
    ReplClose {
        cid: u64,
    },
}

/// What the worker says.
#[derive(Debug)]
pub enum FromWorker {
    /// Sandboxed, and serving.
    Ready,
    /// Deployment `id` loaded and published, or why not; what loading wrote.
    Deployed {
        id: u64,
        error: Option<String>,
        output: String,
    },
    /// The response to request `rid`.
    Response {
        rid: u64,
        response: Data,
    },
    /// A failure the worker answered for: its deployment, the request's
    /// number in the worker, the status and what happened.
    Failure {
        deployment: String,
        request: u64,
        status: u16,
        message: String,
    },
    /// What a request wrote.
    Output {
        deployment: String,
        request: u64,
        text: String,
        truncated: bool,
    },
    /// Bytes to REPL connection `cid`; its end.
    ReplData {
        cid: u64,
        bytes: Vec<u8>,
    },
    ReplClose {
        cid: u64,
    },
}

fn sym(s: &str) -> Data {
    Data::Symbol(s.into())
}

fn int(n: u64) -> Data {
    Data::Int(n as i64)
}

impl ToWorker {
    pub fn to_data(&self) -> Data {
        match self {
            ToWorker::Deploy { id, name, entry, files, wall_ms, dev } => Data::List(vec![
                sym("deploy"),
                int(*id),
                Data::String(name.clone()),
                Data::String(entry.clone()),
                Data::List(files.iter().map(|(p, b)| Data::List(vec![Data::String(p.clone()), Data::Bytes(b.clone())])).collect()),
                int(*wall_ms),
                Data::Bool(*dev),
            ]),
            ToWorker::Request { rid, name, request } => {
                Data::List(vec![sym("request"), int(*rid), Data::String(name.clone()), request.clone()])
            }
            ToWorker::Cancel { rid } => Data::List(vec![sym("cancel"), int(*rid)]),
            ToWorker::ReplOpen { cid } => Data::List(vec![sym("repl-open"), int(*cid)]),
            ToWorker::ReplData { cid, bytes } => Data::List(vec![sym("repl-data"), int(*cid), Data::Bytes(bytes.clone())]),
            ToWorker::ReplClose { cid } => Data::List(vec![sym("repl-close"), int(*cid)]),
        }
    }

    pub fn from_data(d: Data) -> io::Result<ToWorker> {
        let mut p = Parts::of(d)?;
        Ok(match p.symbol()?.as_str() {
            "deploy" => {
                let (id, name, entry) = (p.uint()?, p.string()?, p.string()?);
                let files = p
                    .list()?
                    .into_iter()
                    .map(|f| {
                        let mut f = Parts::of(f)?;
                        Ok((f.string()?, f.bytes()?))
                    })
                    .collect::<io::Result<_>>()?;
                ToWorker::Deploy { id, name, entry, files, wall_ms: p.uint()?, dev: p.flag()? }
            }
            "request" => ToWorker::Request { rid: p.uint()?, name: p.string()?, request: p.any()? },
            "cancel" => ToWorker::Cancel { rid: p.uint()? },
            "repl-open" => ToWorker::ReplOpen { cid: p.uint()? },
            "repl-data" => ToWorker::ReplData { cid: p.uint()?, bytes: p.at_most(REPL_CHUNK)? },
            "repl-close" => ToWorker::ReplClose { cid: p.uint()? },
            other => return Err(bad(&format!("no message {other}"))),
        })
    }
}

impl FromWorker {
    pub fn to_data(&self) -> Data {
        match self {
            FromWorker::Ready => Data::List(vec![sym("ready")]),
            FromWorker::Deployed { id, error, output } => Data::List(vec![
                sym("deployed"),
                int(*id),
                error.as_ref().map_or(Data::Bool(false), |e| Data::String(e.clone())),
                Data::String(output.clone()),
            ]),
            FromWorker::Response { rid, response } => Data::List(vec![sym("response"), int(*rid), response.clone()]),
            FromWorker::Failure { deployment, request, status, message } => Data::List(vec![
                sym("failure"),
                Data::String(deployment.clone()),
                int(*request),
                int((*status).into()),
                Data::String(message.clone()),
            ]),
            FromWorker::Output { deployment, request, text, truncated } => Data::List(vec![
                sym("output"),
                Data::String(deployment.clone()),
                int(*request),
                Data::String(text.clone()),
                Data::Bool(*truncated),
            ]),
            FromWorker::ReplData { cid, bytes } => Data::List(vec![sym("repl-data"), int(*cid), Data::Bytes(bytes.clone())]),
            FromWorker::ReplClose { cid } => Data::List(vec![sym("repl-close"), int(*cid)]),
        }
    }

    pub fn from_data(d: Data) -> io::Result<FromWorker> {
        let mut p = Parts::of(d)?;
        Ok(match p.symbol()?.as_str() {
            "ready" => FromWorker::Ready,
            "deployed" => {
                let id = p.uint()?;
                let error = match p.any()? {
                    Data::Bool(false) => None,
                    Data::String(e) => Some(e),
                    _ => return Err(bad("a deployment's error")),
                };
                FromWorker::Deployed { id, error, output: p.string()? }
            }
            "response" => FromWorker::Response { rid: p.uint()?, response: p.any()? },
            "failure" => FromWorker::Failure {
                deployment: p.string()?,
                request: p.uint()?,
                status: u16::try_from(p.uint()?).map_err(|_| bad("a status"))?,
                message: p.string()?,
            },
            "output" => FromWorker::Output { deployment: p.string()?, request: p.uint()?, text: p.string()?, truncated: p.flag()? },
            "repl-data" => FromWorker::ReplData { cid: p.uint()?, bytes: p.at_most(REPL_CHUNK)? },
            "repl-close" => FromWorker::ReplClose { cid: p.uint()? },
            other => return Err(bad(&format!("no message {other}"))),
        })
    }
}

/// A message's parts, taken in order; all of them, or it is malformed.
struct Parts(std::vec::IntoIter<Data>);

impl Parts {
    fn of(d: Data) -> io::Result<Parts> {
        match d {
            Data::List(items) => Ok(Parts(items.into_iter())),
            _ => Err(bad("a message is a list")),
        }
    }
    fn any(&mut self) -> io::Result<Data> {
        self.0.next().ok_or_else(|| bad("a part missing"))
    }
    fn symbol(&mut self) -> io::Result<String> {
        match self.any()? {
            Data::Symbol(s) => Ok(s),
            _ => Err(bad("a message's kind")),
        }
    }
    fn string(&mut self) -> io::Result<String> {
        match self.any()? {
            Data::String(s) => Ok(s),
            _ => Err(bad("a string")),
        }
    }
    /// Bytes, at most `n` of them.
    fn at_most(&mut self, n: usize) -> io::Result<Vec<u8>> {
        let bytes = self.bytes()?;
        match bytes.len() <= n {
            true => Ok(bytes),
            false => Err(bad("too many bytes")),
        }
    }
    fn bytes(&mut self) -> io::Result<Vec<u8>> {
        match self.any()? {
            Data::Bytes(b) => Ok(b),
            _ => Err(bad("bytes")),
        }
    }
    fn uint(&mut self) -> io::Result<u64> {
        match self.any()? {
            Data::Int(n) if n >= 0 => Ok(n as u64),
            _ => Err(bad("a number")),
        }
    }
    fn flag(&mut self) -> io::Result<bool> {
        match self.any()? {
            Data::Bool(b) => Ok(b),
            _ => Err(bad("a flag")),
        }
    }
    fn list(&mut self) -> io::Result<Vec<Data>> {
        match self.any()? {
            Data::List(items) => Ok(items),
            _ => Err(bad("a list")),
        }
    }
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("malformed message: {what}"))
}

const BOOL_FALSE: u8 = 0;
const BOOL_TRUE: u8 = 1;
const INT: u8 = 2;
const FLOAT: u8 = 3;
const CHAR: u8 = 4;
const STRING: u8 = 5;
const BYTES: u8 = 6;
const SYMBOL: u8 = 7;
const KEYWORD: u8 = 8;
const LIST: u8 = 9;
const VECTOR: u8 = 10;
const DICT: u8 = 11;

/// `d` encoded: a tag byte, then fixed-size numbers, or a length (4 bytes)
/// and bytes, or a count and the parts.
pub fn encode(d: &Data) -> Vec<u8> {
    fn go(d: &Data, out: &mut Vec<u8>) {
        let len = |out: &mut Vec<u8>, n: usize| out.extend_from_slice(&(n as u32).to_be_bytes());
        match d {
            Data::Bool(b) => out.push(if *b { BOOL_TRUE } else { BOOL_FALSE }),
            Data::Int(i) => {
                out.push(INT);
                out.extend_from_slice(&i.to_be_bytes());
            }
            Data::Float(f) => {
                out.push(FLOAT);
                out.extend_from_slice(&f.to_bits().to_be_bytes());
            }
            Data::Char(c) => {
                out.push(CHAR);
                out.extend_from_slice(&(*c as u32).to_be_bytes());
            }
            Data::String(s) | Data::Symbol(s) | Data::Keyword(s) => {
                out.push(match d {
                    Data::String(_) => STRING,
                    Data::Symbol(_) => SYMBOL,
                    _ => KEYWORD,
                });
                len(out, s.len());
                out.extend_from_slice(s.as_bytes());
            }
            Data::Bytes(b) => {
                out.push(BYTES);
                len(out, b.len());
                out.extend_from_slice(b);
            }
            Data::List(items) | Data::Vector(items) => {
                out.push(if matches!(d, Data::List(_)) { LIST } else { VECTOR });
                len(out, items.len());
                items.iter().for_each(|x| go(x, out));
            }
            Data::Dict(entries) => {
                out.push(DICT);
                len(out, entries.len());
                for (k, v) in entries {
                    go(k, out);
                    go(v, out);
                }
            }
        }
    }
    let mut out = Vec::new();
    go(d, &mut out);
    out
}

/// The data `bytes` encode, all of them, within `MAX_DEPTH` and
/// `MAX_PARTS`: no count or length is believed past the bytes left.
pub fn decode(bytes: &[u8]) -> io::Result<Data> {
    struct Reader<'a> {
        bytes: &'a [u8],
        parts: usize,
    }
    impl Reader<'_> {
        fn take(&mut self, n: usize) -> io::Result<&[u8]> {
            if n > self.bytes.len() {
                return Err(bad("ends early"));
            }
            let (head, rest) = self.bytes.split_at(n);
            self.bytes = rest;
            Ok(head)
        }
        fn u32(&mut self) -> io::Result<usize> {
            Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()) as usize)
        }
        fn u64(&mut self) -> io::Result<u64> {
            Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))
        }
        fn text(&mut self) -> io::Result<String> {
            let n = self.u32()?;
            String::from_utf8(self.take(n)?.to_vec()).map_err(|_| bad("text that is no UTF-8"))
        }
        /// A count of parts, each a byte at least: no more than are left.
        fn count(&mut self) -> io::Result<usize> {
            let n = self.u32()?;
            if n > self.bytes.len() { Err(bad("more parts than bytes")) } else { Ok(n) }
        }
        fn data(&mut self, depth: usize) -> io::Result<Data> {
            self.parts += 1;
            if depth > MAX_DEPTH || self.parts > MAX_PARTS {
                return Err(bad("too deep, or too many parts"));
            }
            let tag = self.take(1)?[0];
            Ok(match tag {
                BOOL_FALSE => Data::Bool(false),
                BOOL_TRUE => Data::Bool(true),
                INT => Data::Int(self.u64()? as i64),
                FLOAT => Data::Float(f64::from_bits(self.u64()?)),
                CHAR => Data::Char(char::from_u32(self.u32()? as u32).ok_or_else(|| bad("a char"))?),
                STRING => Data::String(self.text()?),
                SYMBOL => Data::Symbol(self.text()?),
                KEYWORD => Data::Keyword(self.text()?),
                BYTES => {
                    let n = self.u32()?;
                    Data::Bytes(self.take(n)?.to_vec())
                }
                LIST | VECTOR => {
                    let n = self.count()?;
                    let items = (0..n).map(|_| self.data(depth + 1)).collect::<io::Result<Vec<_>>>()?;
                    if tag == LIST { Data::List(items) } else { Data::Vector(items) }
                }
                DICT => {
                    let n = self.count()?;
                    Data::Dict((0..n).map(|_| Ok((self.data(depth + 1)?, self.data(depth + 1)?))).collect::<io::Result<_>>()?)
                }
                _ => return Err(bad("no such part")),
            })
        }
    }
    let mut r = Reader { bytes, parts: 0 };
    let d = r.data(0)?;
    if !r.bytes.is_empty() {
        return Err(bad("bytes after the message"));
    }
    Ok(d)
}

/// Write `message` as a frame.
pub async fn write_frame(w: &mut (impl AsyncWrite + Unpin), message: &Data) -> io::Result<()> {
    let bytes = encode(message);
    if bytes.len() > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "a message past the largest frame"));
    }
    w.write_all(&(bytes.len() as u32).to_be_bytes()).await?;
    w.write_all(&bytes).await?;
    w.flush().await
}

/// Read a frame's message; `None` at the end.
pub async fn read_frame(r: &mut (impl AsyncRead + Unpin)) -> io::Result<Option<Data>> {
    let mut len = [0; 4];
    match r.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(bad("a frame past the largest"));
    }
    let mut bytes = vec![0; len];
    r.read_exact(&mut bytes).await?;
    decode(&bytes).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_comes_back_as_it_went() {
        let d = Data::List(vec![
            Data::Bool(true),
            Data::Int(-5),
            Data::Float(1.5),
            Data::Char('λ'),
            Data::String("hé".into()),
            Data::Bytes(vec![0, 255]),
            Data::Symbol("s".into()),
            Data::Keyword("k".into()),
            Data::Vector(vec![]),
            Data::Dict(vec![(Data::Symbol("a".into()), Data::Int(1))]),
        ]);
        assert_eq!(format!("{:?}", decode(&encode(&d)).unwrap()), format!("{d:?}"));
    }

    #[test]
    fn what_is_no_message_is_refused() {
        // Counts past the bytes, depth, trailing bytes, unknown tags.
        assert!(decode(&[LIST, 0xff, 0xff, 0xff, 0xff]).is_err());
        let deep: Vec<u8> = std::iter::repeat_n([LIST, 0, 0, 0, 1], 100).flatten().chain([BOOL_TRUE]).collect();
        assert!(decode(&deep).is_err());
        assert!(decode(&[BOOL_TRUE, BOOL_TRUE]).is_err());
        assert!(decode(&[99]).is_err());
        assert!(FromWorker::from_data(Data::List(vec![Data::Symbol("response".into()), Data::Int(-1), Data::Int(0)])).is_err());
    }
}
