//! The edit journal: an append-only file of transactions after a base text.
//!
//! A file header names the base (its hash and revision); records follow, each
//! a length and CRC-32 header and a payload. A transaction is written with one
//! `write_all` before it is applied, so a killed process loses nothing it
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
    len: u64,
    poisoned: bool,
    // A separate inode: replacing the journal must not release its lock.
    _lock: Lock,
}

#[derive(Debug)]
pub(crate) struct Lock {
    _file: File,
}

impl Lock {
    pub(crate) fn acquire(path: &Path) -> io::Result<Self> {
        let mut name = path.as_os_str().to_owned();
        name.push(".lock");
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(Path::new(&name))?;
        // The lock belongs to the open file, which a child forked meanwhile
        // shares until it executes its program: a lock just released may
        // still be held for that long. Another owner holds it for good.
        for wait in (0..8).map(|i| std::time::Duration::from_millis(1 << i)) {
            match file.try_lock() {
                Ok(()) => return Ok(Lock { _file: file }),
                Err(std::fs::TryLockError::WouldBlock) => std::thread::sleep(wait),
                Err(std::fs::TryLockError::Error(e)) => return Err(e),
            }
        }
        Err(io::Error::new(io::ErrorKind::WouldBlock, "the journal is already open"))
    }
}

/// Move a legacy journal to its canonical name, preserving every record.
/// Both names must be unowned; an existing destination is never overwritten.
pub fn migrate(from: &Path, to: &Path) -> io::Result<()> {
    if from == to {
        return Ok(());
    }
    let _source = Lock::acquire(from)?;
    let _destination = Lock::acquire(to)?;
    if !from.try_exists()? {
        return Ok(());
    }
    if to.try_exists()? {
        return Err(io::Error::other("two journals exist for this file; recover them before opening it"));
    }
    fs::rename(from, to)?;
    for path in [from, to] {
        let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

impl Journal {
    /// Replace any journal at `path` with an empty one for this base.
    pub fn create(path: &Path, base_revision: Revision, base_hash: &Hash) -> io::Result<Journal> {
        let path = crate::document::file_path(path)?;
        Self::create_with(&path, base_revision, base_hash, &[], Lock::acquire(&path)?)
    }

    /// Replace a journal in one atomic write, including all recovered edits.
    pub(crate) fn create_with(path: &Path, base_revision: Revision, base_hash: &Hash, records: &[Record], lock: Lock) -> io::Result<Self> {
        let bytes = journal_bytes(base_revision, base_hash, records)?;
        write_privately(path, &bytes)?;
        let file = OpenOptions::new().append(true).open(path)?;
        Ok(Journal { file, len: bytes.len() as u64, poisoned: false, _lock: lock })
    }

    /// Start a new base without relinquishing ownership of the journal.
    pub(crate) fn reset(&mut self, path: &Path, base_revision: Revision, base_hash: &Hash) -> io::Result<()> {
        let bytes = journal_bytes(base_revision, base_hash, &[])?;
        // A rename may succeed even if the directory sync fails. On any
        // failure, never append to a potentially replaced journal inode.
        self.poisoned = true;
        write_privately(path, &bytes)?;
        self.file = OpenOptions::new().append(true).open(path)?;
        self.len = bytes.len() as u64;
        self.poisoned = false;
        Ok(())
    }

    pub fn append(&mut self, record: &Record) -> io::Result<()> {
        self.append_using(record, |file, bytes| file.write_all(bytes))
    }

    fn append_using(&mut self, record: &Record, write: impl FnOnce(&mut File, &[u8]) -> io::Result<()>) -> io::Result<()> {
        if self.poisoned {
            return Err(io::Error::other("the journal is broken; further edits cannot be recorded"));
        }
        let bytes = record_bytes(record)?;
        if let Err(error) = write(&mut self.file, &bytes) {
            // A later successful record must not sit behind a torn one.
            if self.file.set_len(self.len).is_err() {
                self.poisoned = true;
            }
            return Err(error);
        }
        self.len += bytes.len() as u64;
        Ok(())
    }
}

fn journal_bytes(base_revision: Revision, base_hash: &Hash, records: &[Record]) -> io::Result<Vec<u8>> {
    let header = FileHeader {
        magic: MAGIC,
        version: VERSION.into(),
        reserved: 0.into(),
        base_revision: base_revision.into(),
        base_hash: *base_hash,
    };
    let mut bytes = header.as_bytes().to_vec();
    for record in records {
        bytes.extend_from_slice(&record_bytes(record)?);
    }
    Ok(bytes)
}

fn record_bytes(record: &Record) -> io::Result<Vec<u8>> {
    let payload = encode(record);
    if payload.len() > MAX_RECORD {
        return Err(io::Error::other("journal record is too large"));
    }
    let header = RecordHeader { len: (payload.len() as u32).into(), crc: crc32fast::hash(&payload).into() };
    let mut buf = Vec::with_capacity(size_of::<RecordHeader>() + payload.len());
    buf.extend_from_slice(header.as_bytes());
    buf.extend_from_slice(&payload);
    Ok(buf)
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
/// readers see the old or the new contents and never a mix. A new file has
/// the permissions new files get; a replaced one keeps its own.
pub fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_through_temp(path, bytes, false)
}

/// As `write_atomically`, but readable by its owner only, whatever it
/// replaces: a journal holds what is typed into files that may be private.
fn write_privately(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_through_temp(path, bytes, true)
}

fn write_through_temp(path: &Path, bytes: &[u8], private: bool) -> io::Result<()> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let mut builder = tempfile::Builder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(if private { 0o600 } else { 0o666 }));
    }
    let mut temp = builder.tempfile_in(dir)?;
    match fs::metadata(path) {
        Ok(_) if private => {}
        Ok(metadata) => temp.as_file().set_permissions(metadata.permissions())?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn new_atomic_files_use_the_normal_umask_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let reference = dir.path().join("reference");
        let target = dir.path().join("target");
        fs::write(&reference, b"normal creation").unwrap();
        write_atomically(&target, b"atomic creation").unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&target), mode(&reference));
    }

    #[cfg(unix)]
    #[test]
    fn journals_are_readable_by_their_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal");
        // Also one made readable by others before is replaced as private.
        fs::write(&path, b"old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let mut journal = Journal::create(&path, 0, &hash(b"base")).unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        journal.reset(&path, 1, &hash(b"next")).unwrap();
        assert_eq!(mode(&path), 0o600);
    }

    #[test]
    fn migration_preserves_records_and_refuses_active_or_existing_owners() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old");
        let new = dir.path().join("new");
        let mut journal = Journal::create(&old, 0, &hash(b"base")).unwrap();
        journal.append(&Record::Saving { hash: hash(b"save") }).unwrap();
        assert_eq!(migrate(&old, &new).unwrap_err().kind(), io::ErrorKind::WouldBlock);
        let bytes = fs::read(&old).unwrap();
        drop(journal);
        migrate(&old, &new).unwrap();
        assert_eq!(fs::read(&new).unwrap(), bytes);
        assert!(!old.exists());
        fs::write(&old, b"another journal").unwrap();
        assert!(migrate(&old, &new).is_err());
        assert_eq!(fs::read(&new).unwrap(), bytes);
        assert_eq!(fs::read(&old).unwrap(), b"another journal");
    }

    #[test]
    fn a_partial_append_is_removed_before_the_next_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal");
        let mut journal = Journal::create(&path, 0, &hash(b"base")).unwrap();
        let first = Record::Saving { hash: hash(b"first") };
        let failed = Record::Saving { hash: hash(b"failed") };
        let last = Record::Saving { hash: hash(b"last") };
        journal.append(&first).unwrap();
        let before = fs::read(&path).unwrap();
        assert!(
            journal
                .append_using(&failed, |file, bytes| {
                    file.write_all(&bytes[..5])?;
                    Err(io::Error::other("injected short write"))
                })
                .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        journal.append(&last).unwrap();
        let found = read(&path).unwrap().unwrap();
        assert!(!found.torn);
        assert_eq!(found.records, [first, last]);
    }

    #[test]
    fn failed_rollback_poisoned_the_journal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal");
        let mut journal = Journal::create(&path, 0, &hash(b"base")).unwrap();
        // A read-only descriptor cannot write or truncate.
        journal.file = File::open(&path).unwrap();
        let record = Record::Saving { hash: hash(b"save") };
        assert!(journal.append(&record).is_err());
        assert!(journal.poisoned);
        assert!(journal.append(&record).unwrap_err().to_string().contains("broken"));
    }
}
