//! The edit journal: an append-only file of transactions after a base text.
//!
//! A file header names the base (its hash and revision); records follow, each
//! a length and CRC-32 header and a payload. A transaction is written with one
//! `write` before it is applied, so a killed process loses nothing it
//! acknowledged. Reading stops at the first record that is incomplete or does
//! not match its checksum: that is a torn last write, and it is discarded.
//! Surviving power loss would need an fsync per record; that policy is open
//! (EDITOR.md, section 13).

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::Path,
    sync::Arc,
};

use zerocopy::{
    FromBytes, Immutable, IntoBytes, KnownLayout,
    little_endian::{U32, U64},
};

use crate::{
    change::{ChangeSet, Op},
    document::{Group, Kind, Revision, Transaction},
};

pub type Hash = [u8; 32];

pub fn hash(bytes: &[u8]) -> Hash {
    *blake3::hash(bytes).as_bytes()
}

const MAGIC: [u8; 8] = *b"TXJOURNL";
const VERSION: u32 = 1;
/// Larger records are treated as corrupt.
const MAX_RECORD: usize = 1 << 30;

#[derive(FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct FileHeader {
    magic: [u8; 8],
    version: U32,
    reserved: U32,
    base_revision: U64,
    base_hash: Hash,
}

#[derive(FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
struct RecordHeader {
    len: U32,
    crc: U32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Record {
    Tx {
        tx: Transaction,
        kind: Kind,
    },
    /// About to replace the file with text of this hash.
    Saving {
        hash: Hash,
    },
}

#[derive(Debug)]
pub struct Journal {
    file: File,
}

impl Journal {
    /// Replace any journal at `path` with an empty one for this base.
    pub fn create(path: &Path, base_revision: Revision, base_hash: &Hash) -> io::Result<Journal> {
        let header = FileHeader {
            magic: MAGIC,
            version: VERSION.into(),
            reserved: 0.into(),
            base_revision: base_revision.into(),
            base_hash: *base_hash,
        };
        write_atomically(path, header.as_bytes())?;
        let file = OpenOptions::new().append(true).open(path)?;
        Ok(Journal { file })
    }

    pub fn append(&mut self, record: &Record) -> io::Result<()> {
        let payload = encode(record);
        let header = RecordHeader { len: (payload.len() as u32).into(), crc: crc32fast::hash(&payload).into() };
        let mut buf = Vec::with_capacity(size_of::<RecordHeader>() + payload.len());
        buf.extend_from_slice(header.as_bytes());
        buf.extend_from_slice(&payload);
        self.file.write_all(&buf)
    }
}

#[derive(Debug)]
pub struct Found {
    pub base_revision: Revision,
    pub base_hash: Hash,
    pub records: Vec<Record>,
    /// A partial or corrupt record was found after the last good one.
    pub torn: bool,
}

/// Read the journal at `path`, if there is one, and cut off a torn tail.
pub fn read(path: &Path) -> Result<Option<Found>, crate::document::OpenError> {
    use crate::document::OpenError;
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let (header, mut rest) = FileHeader::read_from_prefix(&bytes).map_err(|_| OpenError::BadJournal)?;
    if header.magic != MAGIC || header.version.get() != VERSION {
        return Err(OpenError::BadJournal);
    }
    let mut records = Vec::new();
    let torn = loop {
        if rest.is_empty() {
            break false;
        }
        let Some((record, after)) = next_record(rest) else { break true };
        records.push(record);
        rest = after;
    };
    if torn {
        let good = bytes.len() - rest.len();
        OpenOptions::new().write(true).open(path)?.set_len(good as u64)?;
    }
    Ok(Some(Found { base_revision: header.base_revision.get(), base_hash: header.base_hash, records, torn }))
}

fn next_record(bytes: &[u8]) -> Option<(Record, &[u8])> {
    let (header, rest) = RecordHeader::read_from_prefix(bytes).ok()?;
    let len = header.len.get() as usize;
    if len > MAX_RECORD || len > rest.len() {
        return None;
    }
    let (payload, rest) = rest.split_at(len);
    if crc32fast::hash(payload) != header.crc.get() {
        return None;
    }
    Some((decode(payload)?, rest))
}

/// Write `path` through a temporary file and a rename, synced, so that
/// readers see the old or the new contents and never a mix.
pub fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = path.file_name().ok_or_else(|| io::Error::other("no file name"))?;
    let tmp = dir.join(format!(".{}.techne-tmp", name.to_string_lossy()));
    let mut f = File::create(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    fs::rename(&tmp, path)?;
    File::open(dir)?.sync_all()
}

// Payloads: little-endian integers, strings as a u32 length and UTF-8 bytes.

const TAG_TX: u8 = 1;
const TAG_SAVING: u8 = 2;
const OP_RETAIN: u8 = 0;
const OP_DELETE: u8 = 1;
const OP_INSERT: u8 = 2;

fn encode(record: &Record) -> Vec<u8> {
    let mut out = Vec::new();
    match record {
        Record::Tx { tx, kind } => {
            out.push(TAG_TX);
            out.extend_from_slice(&tx.base.to_le_bytes());
            out.push(match kind {
                Kind::Edit => 0,
                Kind::Undo => 1,
                Kind::Redo => 2,
            });
            out.push(match tx.group {
                Group::New => 0,
                Group::Extend => 1,
            });
            put_str(&mut out, &tx.actor);
            out.extend_from_slice(&(tx.changes.ops().len() as u32).to_le_bytes());
            for op in tx.changes.ops() {
                match op {
                    Op::Retain(n) => {
                        out.push(OP_RETAIN);
                        out.extend_from_slice(&(*n as u64).to_le_bytes());
                    }
                    Op::Delete(n) => {
                        out.push(OP_DELETE);
                        out.extend_from_slice(&(*n as u64).to_le_bytes());
                    }
                    Op::Insert(s) => {
                        out.push(OP_INSERT);
                        put_str(&mut out, s);
                    }
                }
            }
        }
        Record::Saving { hash } => {
            out.push(TAG_SAVING);
            out.extend_from_slice(hash);
        }
    }
    out
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u32).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        if n > self.0.len() {
            return None;
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Some(a)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.bytes(1)?[0])
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.bytes(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.bytes(8)?.try_into().ok()?))
    }
    fn str(&mut self) -> Option<&'a str> {
        let n = self.u32()? as usize;
        std::str::from_utf8(self.bytes(n)?).ok()
    }
}

fn decode(payload: &[u8]) -> Option<Record> {
    let mut r = Reader(payload);
    let record = match r.u8()? {
        TAG_TX => {
            let base = r.u64()?;
            let kind = match r.u8()? {
                0 => Kind::Edit,
                1 => Kind::Undo,
                2 => Kind::Redo,
                _ => return None,
            };
            let group = match r.u8()? {
                0 => Group::New,
                1 => Group::Extend,
                _ => return None,
            };
            let actor: Arc<str> = Arc::from(r.str()?);
            let n = r.u32()? as usize;
            // Rebuild through edits so the change is canonical and consistent.
            let mut pos = 0;
            let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();
            for _ in 0..n {
                match r.u8()? {
                    OP_RETAIN => pos += r.u64()? as usize,
                    OP_DELETE => {
                        let n = r.u64()? as usize;
                        match edits.last_mut() {
                            Some((range, _)) if range.end == pos => range.end += n,
                            _ => edits.push((pos..pos + n, String::new())),
                        }
                        pos += n;
                    }
                    OP_INSERT => edits.push((pos..pos, r.str()?.to_string())),
                    _ => return None,
                }
            }
            let changes = ChangeSet::from_edits(pos, edits).ok()?;
            Record::Tx { tx: Transaction { base, actor, changes, group }, kind }
        }
        TAG_SAVING => Record::Saving { hash: r.bytes(32)?.try_into().ok()? },
        _ => return None,
    };
    r.0.is_empty().then_some(record)
}
