//! The edit journal: an append-only file of transactions after a base text.
//!
//! A file header names the base (its hash and revision); records follow, each
//! a length and CRC-32 header and a payload. A transaction is written with one
//! `write_all` before it is applied, so a killed process loses nothing it
//! acknowledged. Reading stops at the first record that is incomplete or does
//! not match its checksum: that is a torn last write, and it is discarded.
//! Surviving power loss would need an fsync per record; that policy is open
//! (EDITOR.md, section 13).
//!
//! A journal is its writer's alone: a lock file keeps another from opening
//! it. The journals opened on a thread are its `Fence`'s, which another
//! thread revokes to take them over from a writer that does not stop.

use std::{
    cell::RefCell,
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::Path,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use zerocopy::{
    FromBytes, Immutable, IntoBytes, KnownLayout,
    little_endian::{U32, U64},
};

use crate::{
    change::{ChangeSet, Op},
    document::{Group, Kind, OperationId, Revision, Transaction},
};

pub type Hash = [u8; 32];

pub fn hash(bytes: &[u8]) -> Hash {
    *blake3::hash(bytes).as_bytes()
}

const MAGIC: [u8; 8] = *b"TXJOURNL";
/// Version 2 adds transactions made in an operation (`TAG_TX_IN`). A
/// journal of version 1 is read as one with none; one is written as
/// version 2 when its document opens, so a binary knowing only version 1
/// refuses it rather than dropping those records as a torn tail.
const VERSION: u32 = 2;
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
    lock: Lock,
}

/// Who may write the journals opened on a thread (`enter`): until it is
/// revoked, as when a runtime that does not stop is replaced by another.
/// Then they refuse to write and their locks are released, so the other
/// can open them: two writers never write one journal.
#[derive(Clone, Debug, Default)]
pub struct Fence(Arc<Mutex<Fenced>>);

#[derive(Debug, Default)]
pub(crate) struct Fenced {
    revoked: bool,
    /// The lock files of its journals, by `Lock::slot`.
    locks: HashMap<u64, File>,
    next: u64,
}

thread_local! {
    static CURRENT: RefCell<Fence> = RefCell::default();
}

impl Fence {
    /// The journals opened on this thread from now on are this fence's.
    pub fn enter(&self) {
        CURRENT.with(|c| *c.borrow_mut() = self.clone());
    }

    /// Its journals refuse to write from now on, and their locks are
    /// released. A write under way is waited for: none is cut short.
    pub fn revoke(&self) {
        let mut fenced = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        fenced.revoked = true;
        fenced.locks.clear();
    }

    /// The fence active on the current thread.
    pub(crate) fn current() -> Self {
        CURRENT.with(|c| c.borrow().clone())
    }

    /// Held while writing: no revocation comes between.
    pub(crate) fn hold(&self) -> io::Result<MutexGuard<'_, Fenced>> {
        let fenced = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        match fenced.revoked {
            true => Err(io::Error::other("the journal was taken over; further edits cannot be recorded")),
            false => Ok(fenced),
        }
    }
}

/// A journal's lock file, kept by the thread's fence.
#[derive(Debug)]
pub(crate) struct Lock {
    fence: Fence,
    slot: u64,
}

impl Drop for Lock {
    fn drop(&mut self) {
        self.fence.0.lock().unwrap_or_else(PoisonError::into_inner).locks.remove(&self.slot);
    }
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
                Ok(()) => {
                    let fence = CURRENT.with(|c| c.borrow().clone());
                    let mut fenced = fence.hold()?;
                    let slot = fenced.next;
                    fenced.next += 1;
                    fenced.locks.insert(slot, file);
                    drop(fenced);
                    return Ok(Lock { fence, slot });
                }
                Err(std::fs::TryLockError::WouldBlock) => std::thread::sleep(wait),
                Err(std::fs::TryLockError::Error(e)) => return Err(e),
            }
        }
        Err(io::Error::new(io::ErrorKind::WouldBlock, "the journal is already open"))
    }
}

/// Whether the journal at `path` holds edits not saved.
pub(crate) fn has_unsaved(path: &Path) -> io::Result<bool> {
    let found = read(path).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(found.is_some_and(|f| f.records.iter().any(|r| matches!(r, Record::Tx { .. }))))
}

/// Fail closed if a journal at `path` already exists on disk or its lock is
/// held by another process, without creating any lock file, journal file, or
/// directory. Used before opening a file under `Retention::None`.
pub fn check_no_journal(path: &Path) -> io::Result<()> {
    let mut lock_name = path.as_os_str().to_owned();
    lock_name.push(".lock");
    let lock_path = Path::new(&lock_name);
    match OpenOptions::new().read(true).write(true).open(lock_path) {
        Ok(file) => match file.try_lock() {
            Ok(()) => drop(file),
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(io::Error::new(io::ErrorKind::WouldBlock, "the file is already open with a journal in another process"));
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(e),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    if path.try_exists()? {
        if has_unsaved(path)? {
            return Err(io::Error::other(
                "a journal with unsaved edits already exists for this file; recover or remove it before opening privately",
            ));
        }
        return Err(io::Error::other("an existing journal is present on disk for this file; remove it before opening privately"));
    }
    Ok(())
}

/// Sync the directories `paths` are in.
fn sync_parents(paths: &[&Path]) -> io::Result<()> {
    paths.iter().try_for_each(|path| {
        let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        File::open(parent)?.sync_all()
    })
}

/// Move a legacy journal to its canonical name, preserving every record.
/// Both names must be unowned; an existing destination is never overwritten.
pub fn migrate(from: &Path, to: &Path) -> io::Result<()> {
    if from == to {
        return Ok(());
    }
    let source = Lock::acquire(from)?;
    let _destination = Lock::acquire(to)?;
    if !from.try_exists()? {
        return Ok(());
    }
    if to.try_exists()? {
        return Err(io::Error::other("two journals exist for this file; recover them before opening it"));
    }
    let _held = source.fence.hold()?;
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
        let fence = lock.fence.clone();
        let _held = fence.hold()?;
        write_privately(path, &bytes)?;
        let file = OpenOptions::new().append(true).open(path)?;
        Ok(Journal { file, len: bytes.len() as u64, poisoned: false, lock })
    }

    /// Move the journal from `from` to `to`, whose lock `to_lock` is, and
    /// keep writing it there; across file systems it is copied. An error
    /// leaves it where it was, its own. Once it is at `to`, a move that may
    /// not survive a crash (a directory not synced) refuses further
    /// records, as a failed append does.
    pub(crate) fn move_to(&mut self, from: &Path, to: &Path, to_lock: Lock) -> io::Result<()> {
        let fence = self.lock.fence.clone();
        let held = fence.hold()?;
        let durable = match fs::rename(from, to) {
            // The open file stays this journal's: appends go on after it.
            Ok(()) => sync_parents(&[from, to]),
            Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
                write_privately(to, &fs::read(from)?)?;
                match OpenOptions::new().append(true).open(to) {
                    Ok(file) => self.file = file,
                    Err(e) => {
                        let _ = fs::remove_file(to);
                        return Err(e);
                    }
                }
                // Written at `to` from now on; a copy left behind is stale.
                fs::remove_file(from).and_then(|()| sync_parents(&[from]))
            }
            Err(e) => return Err(e),
        };
        self.poisoned |= durable.is_err();
        // Dropping the old lock takes the fence, held until now.
        drop(held);
        self.lock = to_lock;
        Ok(())
    }

    /// Start a new base without relinquishing ownership of the journal.
    pub(crate) fn reset(&mut self, path: &Path, base_revision: Revision, base_hash: &Hash) -> io::Result<()> {
        let fence = self.lock.fence.clone();
        let _held = fence.hold()?;
        self.reset_held(path, base_revision, base_hash)
    }

    /// Check `check`, record saving text of `hash`, `write` it, and start the
    /// base `base_revision` after it, all under one revocation fence. When
    /// `has_tail` is true (edits were appended after `base_revision`, e.g.
    /// while encoding asynchronously), `Saving` is placed right before those
    /// tail edits and they are preserved in the reset journal.
    pub(crate) fn save_checked<E>(
        &mut self,
        path: &Path,
        base_revision: Revision,
        hash: &Hash,
        has_tail: bool,
        check: impl FnOnce() -> Result<(), E>,
        write: impl FnOnce() -> Result<(), E>,
    ) -> Result<(), E>
    where
        E: From<io::Error>,
    {
        let fence = self.lock.fence.clone();
        let _held = fence.hold()?;
        if self.poisoned {
            return Err(io::Error::other("the journal is broken; further edits cannot be recorded").into());
        }
        check()?;
        if !has_tail {
            // Recorded and synced first: a power loss after the write finds the
            // file matching `Saving` instead of a JournalMismatch.
            self.append_held(&Record::Saving { hash: *hash }, |file, bytes| file.write_all(bytes))?;
            self.file.sync_data()?;
            write()?;
            self.reset_held(path, base_revision, hash)?;
            return Ok(());
        }
        let found = read(path).map_err(|e| io::Error::other(e.to_string()))?.ok_or_else(|| io::Error::other("the journal is gone"))?;
        let split = split_at_revision(&found, base_revision)?;
        let mut with_saving = Vec::with_capacity(found.records.len() + 1);
        with_saving.extend_from_slice(&found.records[..split]);
        with_saving.push(Record::Saving { hash: *hash });
        with_saving.extend_from_slice(&found.records[split..]);
        self.reset_with(path, found.base_revision, &found.base_hash, &with_saving)?;
        write()?;
        self.reset_with(path, base_revision, hash, &found.records[split..])?;
        Ok(())
    }

    /// Record saving text of `hash` at `base_revision` before an asynchronous
    /// remote write. When `has_tail` is true, places `Saving` before edits
    /// appended after `base_revision`.
    pub(crate) fn record_saving_at(&mut self, path: &Path, base_revision: Revision, hash: &Hash, has_tail: bool) -> io::Result<()> {
        let fence = self.lock.fence.clone();
        let _held = fence.hold()?;
        if self.poisoned {
            return Err(io::Error::other("the journal is broken; further edits cannot be recorded"));
        }
        if !has_tail {
            self.append_held(&Record::Saving { hash: *hash }, |file, bytes| file.write_all(bytes))?;
            self.file.sync_data()?;
            return Ok(());
        }
        let found = read(path).map_err(|e| io::Error::other(e.to_string()))?.ok_or_else(|| io::Error::other("the journal is gone"))?;
        let split = split_at_revision(&found, base_revision)?;
        let mut with_saving = Vec::with_capacity(found.records.len() + 1);
        with_saving.extend_from_slice(&found.records[..split]);
        with_saving.push(Record::Saving { hash: *hash });
        with_saving.extend_from_slice(&found.records[split..]);
        self.reset_with(path, found.base_revision, &found.base_hash, &with_saving)
    }

    /// The save of text of `hash`, recorded before, finished: start the
    /// base `base_revision` after it, keeping the records written since
    /// (edits made while the file was written elsewhere).
    pub(crate) fn checkpoint(&mut self, path: &Path, base_revision: Revision, hash: &Hash) -> io::Result<()> {
        let fence = self.lock.fence.clone();
        let _held = fence.hold()?;
        let found = read(path).map_err(|e| io::Error::other(e.to_string()))?.ok_or_else(|| io::Error::other("the journal is gone"))?;
        let mut tx_count = 0u64;
        let mut matched = None;
        for (i, r) in found.records.iter().enumerate() {
            match r {
                Record::Tx { .. } => tx_count += 1,
                Record::Saving { hash: h } if h == hash && found.base_revision + tx_count == base_revision => {
                    matched = Some(i);
                }
                _ => {}
            }
        }
        let after = matched.ok_or_else(|| io::Error::other("the save is not in the journal"))?;
        self.reset_with(path, base_revision, hash, &found.records[after + 1..])
    }

    fn reset_held(&mut self, path: &Path, base_revision: Revision, base_hash: &Hash) -> io::Result<()> {
        self.reset_with(path, base_revision, base_hash, &[])
    }

    fn reset_with(&mut self, path: &Path, base_revision: Revision, base_hash: &Hash, records: &[Record]) -> io::Result<()> {
        let bytes = journal_bytes(base_revision, base_hash, records)?;
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
        let fence = self.lock.fence.clone();
        let _held = fence.hold()?;
        self.append_held(record, write)
    }

    /// Append RECORDS in one write: all of them, or, failing, none.
    pub fn append_all(&mut self, records: &[Record]) -> io::Result<()> {
        let fence = self.lock.fence.clone();
        let _held = fence.hold()?;
        let mut bytes = Vec::new();
        for record in records {
            bytes.extend(record_bytes(record)?);
        }
        self.write_held(&bytes, |file, bytes| file.write_all(bytes))
    }

    fn append_held(&mut self, record: &Record, write: impl FnOnce(&mut File, &[u8]) -> io::Result<()>) -> io::Result<()> {
        let bytes = record_bytes(record)?;
        self.write_held(&bytes, write)
    }

    fn write_held(&mut self, bytes: &[u8], write: impl FnOnce(&mut File, &[u8]) -> io::Result<()>) -> io::Result<()> {
        if self.poisoned {
            return Err(io::Error::other("the journal is broken; further edits cannot be recorded"));
        }
        if let Err(error) = write(&mut self.file, bytes) {
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

/// Read the journal at `path`, if there is one, up to a torn tail. It is
/// left as it is: opening a document replaces it.
pub fn read(path: &Path) -> Result<Option<Found>, crate::document::OpenError> {
    use crate::document::OpenError;
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let (header, mut rest) = FileHeader::read_from_prefix(&bytes).map_err(|_| OpenError::BadJournal)?;
    if header.magic != MAGIC || !(1..=VERSION).contains(&header.version.get()) {
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

fn split_at_revision(found: &Found, target: Revision) -> io::Result<usize> {
    let needed = target.checked_sub(found.base_revision).ok_or_else(|| io::Error::other("the journal does not hold the saved revision"))?;
    if needed == 0 {
        return Ok(0);
    }
    let mut seen = 0u64;
    for (i, rec) in found.records.iter().enumerate() {
        if matches!(rec, Record::Tx { .. }) {
            seen += 1;
            if seen == needed {
                return Ok(i + 1);
            }
        }
    }
    Err(io::Error::other("the journal does not hold the saved revision"))
}

/// Write `path` through a temporary file and a rename, synced, so that
/// readers see the old or the new contents and never a mix. A new file has
/// the permissions new files get; a replaced one keeps its own.
pub fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_through_temp(path, bytes, false, false, false)
}

/// As `write_atomically`, but readable by its owner only (`0o600`), whether
/// creating a new file or replacing an existing one.
pub fn write_atomically_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_through_temp(path, bytes, true, true, false)
}

/// Publish a new file without clobbering an existing file at `path`.
/// Returns `AlreadyExists` if another writer created `path` concurrently.
pub fn write_atomically_if_absent(path: &Path, bytes: &[u8], private_new: bool) -> io::Result<()> {
    write_through_temp(path, bytes, private_new, false, true)
}

/// As `write_atomically`, but readable by its owner only, whatever it
/// replaces: a journal holds what is typed into files that may be private.
fn write_privately(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomically_private(path, bytes)
}

fn write_through_temp(path: &Path, bytes: &[u8], private_new: bool, force_private: bool, no_clobber: bool) -> io::Result<()> {
    use rustix::fs::{Mode, OFlags, openat};
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let leaf = path.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    let dir_fd =
        openat(rustix::fs::CWD, dir, OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC, Mode::empty()).map_err(io::Error::from)?;
    write_atomically_in_dir(&dir_fd, leaf, bytes, private_new, force_private, no_clobber)
}

/// Write `bytes` to `leaf` inside the already-verified directory handle
/// `dir_fd` through a temporary file staged inside a `0o700` owner-only
/// subdirectory of `dir_fd`, synced, and published strictly relative to
/// `dir_fd` (never re-resolving any parent path).
///
/// New non-private files are created with mode `0o666` inside the verified
/// `0o700` staging directory without a subsequent `fchmod`, so the kernel
/// applies the process `umask` and any inherited default ACL while the
/// `0o700` stage protects the staged bytes before publication. Private
/// files and replacements clear any inherited Linux default ACL from the
/// staging directory before creating the staged file with mode `0o600` from
/// birth; non-private replacements preserve the destination's standard
/// `0o0777` permission bits (stripping setuid/setgid/sticky bits) and, on
/// Linux, any extended POSIX access ACL on the destination itself without
/// adopting the parent directory's default ACL.
///
/// Before staging and again before replacement, the destination entry is
/// inspected without following symlinks, refusing symlinks, directories,
/// non-regular files, and inspection errors, and rejecting any observed
/// change in destination identity, mode, or (on Linux) POSIX access ACL
/// before publication. On the `no_clobber` path, `RENAME_NOREPLACE` /
/// `linkat` atomically refuses any existing entry at `leaf`. On the replace
/// path, `renameat` publishes the staged regular file atomically without
/// ever following a destination symlink.
pub fn write_atomically_in_dir(
    dir_fd: &rustix::fd::OwnedFd,
    leaf: &std::ffi::OsStr,
    bytes: &[u8],
    private_new: bool,
    force_private: bool,
    no_clobber: bool,
) -> io::Result<()> {
    write_atomically_in_dir_inner(dir_fd, leaf, bytes, private_new, force_private, no_clobber, || {})
}

#[cfg(target_os = "linux")]
const POSIX_ACL_ACCESS_XATTR: &str = "system.posix_acl_access";
#[cfg(target_os = "linux")]
const POSIX_ACL_DEFAULT_XATTR: &str = "system.posix_acl_default";

#[cfg(target_os = "linux")]
fn read_xattr_bytes<F>(mut read_fn: F) -> io::Result<Option<Vec<u8>>>
where
    F: FnMut(&mut [u8]) -> rustix::io::Result<usize>,
{
    let mut buf = vec![0u8; 1024];
    loop {
        match read_fn(&mut buf) {
            Ok(len) => {
                buf.truncate(len);
                return Ok(Some(buf));
            }
            Err(rustix::io::Errno::NODATA | rustix::io::Errno::OPNOTSUPP | rustix::io::Errno::NOSYS) => {
                return Ok(None);
            }
            Err(rustix::io::Errno::RANGE) => {
                let needed = match read_fn(&mut []) {
                    Ok(n) => n,
                    Err(rustix::io::Errno::NODATA | rustix::io::Errno::OPNOTSUPP | rustix::io::Errno::NOSYS) => {
                        return Ok(None);
                    }
                    Err(e) => return Err(io::Error::from(e)),
                };
                if needed == 0 {
                    return Ok(None);
                }
                buf.resize(needed, 0);
            }
            Err(e) => return Err(io::Error::from(e)),
        }
    }
}

#[cfg(target_os = "linux")]
fn read_fd_posix_acl(fd: impl rustix::fd::AsFd) -> io::Result<Option<Vec<u8>>> {
    let fd = fd.as_fd();
    read_xattr_bytes(|buf| rustix::fs::fgetxattr(fd, POSIX_ACL_ACCESS_XATTR, buf))
}

#[cfg(target_os = "linux")]
fn read_opath_posix_acl(fd: impl rustix::fd::AsFd) -> io::Result<Option<Vec<u8>>> {
    use rustix::fd::AsRawFd;
    let proc_path = format!("/proc/self/fd/{}", fd.as_fd().as_raw_fd());
    read_xattr_bytes(|buf| rustix::fs::getxattr(&proc_path, POSIX_ACL_ACCESS_XATTR, buf))
}

fn establish_replacement_ownership(
    file: &File,
    dest_uid: u32,
    dest_gid: u32,
    do_fchown: impl FnOnce(&File, Option<rustix::fs::Uid>, Option<rustix::fs::Gid>) -> io::Result<()>,
) -> io::Result<()> {
    use rustix::fs::{Gid, Uid, fstat};
    let staged_st = fstat(file).map_err(io::Error::from)?;
    #[allow(clippy::unnecessary_cast)]
    let (staged_uid, staged_gid) = (staged_st.st_uid as u32, staged_st.st_gid as u32);
    if staged_uid != dest_uid || staged_gid != dest_gid {
        let uid_arg = (staged_uid != dest_uid).then(|| Uid::from_raw(dest_uid));
        let gid_arg = (staged_gid != dest_gid).then(|| Gid::from_raw(dest_gid));
        do_fchown(file, uid_arg, gid_arg).map_err(|e| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("cannot preserve destination ownership ({dest_uid}:{dest_gid}) on replacement: {e}"),
            )
        })?;
        let verified_st = fstat(file).map_err(io::Error::from)?;
        #[allow(clippy::unnecessary_cast)]
        if verified_st.st_uid as u32 != dest_uid || verified_st.st_gid as u32 != dest_gid {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("staged file ownership verification failed for {dest_uid}:{dest_gid}"),
            ));
        }
    }
    Ok(())
}

fn write_atomically_in_dir_inner(
    dir_fd: &rustix::fd::OwnedFd,
    leaf: &std::ffi::OsStr,
    bytes: &[u8],
    private_new: bool,
    force_private: bool,
    no_clobber: bool,
    before_publish: impl FnOnce(),
) -> io::Result<()> {
    write_atomically_in_dir_with_chown(dir_fd, leaf, bytes, private_new, force_private, no_clobber, before_publish, |file, uid, gid| {
        rustix::fs::fchown(file, uid, gid).map_err(io::Error::from)
    })
}

#[allow(clippy::too_many_arguments)]
fn write_atomically_in_dir_with_chown(
    dir_fd: &rustix::fd::OwnedFd,
    leaf: &std::ffi::OsStr,
    bytes: &[u8],
    private_new: bool,
    force_private: bool,
    no_clobber: bool,
    before_publish: impl FnOnce(),
    do_fchown: impl FnOnce(&File, Option<rustix::fs::Uid>, Option<rustix::fs::Gid>) -> io::Result<()>,
) -> io::Result<()> {
    #[cfg(not(target_os = "linux"))]
    use rustix::fs::statat;
    use rustix::fs::{AtFlags, FileType, Mode, OFlags, fchmod, fstat, fsync, linkat, mkdirat, openat, renameat, unlinkat};
    #[cfg(target_os = "linux")]
    use rustix::fs::{XattrFlags, fremovexattr, fsetxattr};
    use std::sync::atomic::{AtomicU64, Ordering};

    static STAGE_SEQ: AtomicU64 = AtomicU64::new(0);

    #[derive(Clone, PartialEq, Eq)]
    struct DestMeta {
        dev: u64,
        ino: u64,
        uid: u32,
        gid: u32,
        raw_mode: u32,
        #[cfg(target_os = "linux")]
        acl: Option<Vec<u8>>,
    }

    let check_destination = |for_no_clobber: bool| -> io::Result<Option<DestMeta>> {
        #[cfg(target_os = "linux")]
        {
            let opath_fd = match openat(dir_fd, leaf, OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty()) {
                Ok(fd) => fd,
                Err(rustix::io::Errno::NOENT) => return Ok(None),
                Err(e) => return Err(io::Error::from(e)),
            };
            let st = fstat(&opath_fd).map_err(io::Error::from)?;
            let ft = FileType::from_raw_mode(st.st_mode);
            if ft == FileType::Symlink {
                return Err(io::Error::other("file target changed on disk while saving"));
            }
            if for_no_clobber {
                return Err(io::Error::new(io::ErrorKind::AlreadyExists, "file already exists"));
            }
            if ft == FileType::Directory {
                return Err(io::Error::new(io::ErrorKind::IsADirectory, "is a directory"));
            }
            if ft != FileType::RegularFile {
                return Err(io::Error::other("not a regular file"));
            }
            let acl = match openat(dir_fd, leaf, OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC, Mode::empty()) {
                Ok(rd_fd) => {
                    let rd_st = fstat(&rd_fd).map_err(io::Error::from)?;
                    if rd_st.st_dev != st.st_dev
                        || rd_st.st_ino != st.st_ino
                        || rd_st.st_uid != st.st_uid
                        || rd_st.st_gid != st.st_gid
                        || rd_st.st_mode != st.st_mode
                    {
                        return Err(io::Error::other("file target changed on disk while saving"));
                    }
                    read_fd_posix_acl(&rd_fd)?
                }
                Err(rustix::io::Errno::ACCESS) => read_opath_posix_acl(&opath_fd)?,
                Err(rustix::io::Errno::LOOP | rustix::io::Errno::NOENT) => {
                    return Err(io::Error::other("file target changed on disk while saving"));
                }
                Err(e) => return Err(io::Error::from(e)),
            };
            #[allow(clippy::unnecessary_cast)]
            let meta = DestMeta {
                dev: st.st_dev as u64,
                ino: st.st_ino as u64,
                uid: st.st_uid as u32,
                gid: st.st_gid as u32,
                raw_mode: st.st_mode as u32 & 0o7777,
                acl,
            };
            Ok(Some(meta))
        }
        #[cfg(not(target_os = "linux"))]
        {
            match statat(dir_fd, leaf, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(st) => {
                    let ft = FileType::from_raw_mode(st.st_mode);
                    if ft == FileType::Symlink {
                        return Err(io::Error::other("file target changed on disk while saving"));
                    }
                    if for_no_clobber {
                        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "file already exists"));
                    }
                    if ft == FileType::Directory {
                        return Err(io::Error::new(io::ErrorKind::IsADirectory, "is a directory"));
                    }
                    if ft != FileType::RegularFile {
                        return Err(io::Error::other("not a regular file"));
                    }
                    #[allow(clippy::unnecessary_cast)]
                    let meta = DestMeta {
                        dev: st.st_dev as u64,
                        ino: st.st_ino as u64,
                        uid: st.st_uid as u32,
                        gid: st.st_gid as u32,
                        raw_mode: st.st_mode as u32 & 0o7777,
                    };
                    Ok(Some(meta))
                }
                Err(rustix::io::Errno::NOENT) => Ok(None),
                Err(e) => Err(io::Error::from(e)),
            }
        }
    };

    let initial_dest = check_destination(no_clobber)?;

    struct StageGuard<'a> {
        dir_fd: &'a rustix::fd::OwnedFd,
        stage_fd: rustix::fd::OwnedFd,
        stage_name: String,
        tmp_present: bool,
    }
    impl Drop for StageGuard<'_> {
        fn drop(&mut self) {
            if self.tmp_present {
                let _ = unlinkat(&self.stage_fd, "tmp", AtFlags::empty());
            }
            let _ = unlinkat(self.dir_fd, &self.stage_name, AtFlags::REMOVEDIR);
        }
    }

    let pid = std::process::id();
    let euid = rustix::process::geteuid().as_raw();
    let mut guard = None;
    for _ in 0..128 {
        let seq = STAGE_SEQ.fetch_add(1, Ordering::Relaxed);
        let stage_name = format!(".techne-stage-{pid}-{seq}");
        match mkdirat(dir_fd, &stage_name, Mode::from_raw_mode(0o700)) {
            Ok(()) => {}
            Err(rustix::io::Errno::EXIST) => continue,
            Err(e) => return Err(io::Error::from(e)),
        }
        let stage_fd =
            match openat(dir_fd, &stage_name, OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty()) {
                Ok(fd) => fd,
                Err(e) => {
                    let _ = unlinkat(dir_fd, &stage_name, AtFlags::REMOVEDIR);
                    return Err(io::Error::from(e));
                }
            };
        let st = match fstat(&stage_fd) {
            Ok(st) => st,
            Err(e) => {
                let _ = unlinkat(dir_fd, &stage_name, AtFlags::REMOVEDIR);
                return Err(io::Error::from(e));
            }
        };
        if FileType::from_raw_mode(st.st_mode) != FileType::Directory || st.st_uid != euid || (st.st_mode & 0o077) != 0 {
            let _ = unlinkat(dir_fd, &stage_name, AtFlags::REMOVEDIR);
            return Err(io::Error::other("staging directory failed ownership or permission verification"));
        }
        guard = Some(StageGuard { dir_fd, stage_fd, stage_name, tmp_present: false });
        break;
    }
    let mut guard = guard.ok_or_else(|| io::Error::other("could not create staging directory"))?;

    // For a brand-new non-private file, let the kernel apply the process umask
    // and any inherited default ACL directly (protected before publication by
    // the verified 0o700 staging directory). For private files or replacements,
    // clear any inherited default ACL on the staging directory first so the
    // staged file is born 0o600 with no directory-inherited extended ACL.
    let isolate_from_dir_acl = force_private || private_new || initial_dest.is_some();
    #[cfg(target_os = "linux")]
    if isolate_from_dir_acl {
        match fremovexattr(&guard.stage_fd, POSIX_ACL_DEFAULT_XATTR) {
            Ok(()) | Err(rustix::io::Errno::NODATA | rustix::io::Errno::OPNOTSUPP | rustix::io::Errno::NOSYS) => {}
            Err(e) => return Err(io::Error::from(e)),
        }
    }

    let create_mode = if isolate_from_dir_acl { 0o600 } else { 0o666 };
    let tmp_fd = openat(
        &guard.stage_fd,
        "tmp",
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(create_mode),
    )
    .map_err(io::Error::from)?;
    guard.tmp_present = true;
    let mut file = File::from(tmp_fd);

    #[cfg(target_os = "linux")]
    if isolate_from_dir_acl && read_fd_posix_acl(&file)?.is_some() {
        return Err(io::Error::other("staged file unexpectedly inherited a POSIX ACL"));
    }

    file.write_all(bytes)?;
    if !force_private && let Some(dest) = &initial_dest {
        // Establish matching UID/GID BEFORE applying destination mode or ACL so
        // group/owner bits and ACL_GROUP_OBJ never grant access to a different
        // principal (e.g., a setgid parent directory's group). Fails closed if
        // ownership cannot be matched.
        establish_replacement_ownership(&file, dest.uid, dest.gid, do_fchown)?;
        #[cfg(target_os = "linux")]
        if let Some(acl) = &dest.acl {
            // Restoring the destination's POSIX access ACL also sets the
            // matching 0o0777 mode bits while keeping setuid/setgid/sticky
            // bits clear on the newly created 0o600 inode.
            fsetxattr(&file, POSIX_ACL_ACCESS_XATTR, acl, XattrFlags::empty()).map_err(io::Error::from)?;
        } else {
            // Strip setuid, setgid, and sticky bits: an ordinary editor save
            // creates a new inode owned by the saving process and must never
            // publish set-ID executables.
            let replacement_mode = dest.raw_mode & 0o0777;
            fchmod(&file, Mode::from_raw_mode(replacement_mode)).map_err(io::Error::from)?;
        }
        #[cfg(not(target_os = "linux"))]
        {
            let replacement_mode = dest.raw_mode & 0o0777;
            fchmod(&file, Mode::from_raw_mode(replacement_mode)).map_err(io::Error::from)?;
        }
    }
    file.sync_all()?;

    before_publish();

    if no_clobber {
        let publish_via_link = |guard: &mut StageGuard<'_>| -> io::Result<()> {
            match linkat(&guard.stage_fd, "tmp", dir_fd, leaf, AtFlags::empty()) {
                Ok(()) => {
                    let _ = unlinkat(&guard.stage_fd, "tmp", AtFlags::empty());
                    guard.tmp_present = false;
                    Ok(())
                }
                Err(rustix::io::Errno::EXIST) => Err(io::Error::new(io::ErrorKind::AlreadyExists, "file already exists")),
                Err(e) => Err(io::Error::from(e)),
            }
        };
        #[cfg(target_os = "linux")]
        {
            use rustix::fs::{RenameFlags, renameat_with};
            match renameat_with(&guard.stage_fd, "tmp", dir_fd, leaf, RenameFlags::NOREPLACE) {
                Ok(()) => {
                    guard.tmp_present = false;
                }
                Err(rustix::io::Errno::EXIST) => {
                    return Err(io::Error::new(io::ErrorKind::AlreadyExists, "file already exists"));
                }
                Err(rustix::io::Errno::NOSYS | rustix::io::Errno::INVAL | rustix::io::Errno::OPNOTSUPP) => {
                    publish_via_link(&mut guard)?;
                }
                Err(e) => return Err(io::Error::from(e)),
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            publish_via_link(&mut guard)?;
        }
    } else {
        // Re-check destination before rename, failing closed on symlinks,
        // non-regular files, inspection errors, or any observed change in
        // destination identity, permissions, or POSIX access ACL. POSIX
        // `renameat` publishes the staged regular file atomically without
        // following a destination symlink.
        let current_dest = check_destination(false)?;
        if current_dest != initial_dest {
            return Err(io::Error::other("file target changed on disk while saving"));
        }
        renameat(&guard.stage_fd, "tmp", dir_fd, leaf).map_err(io::Error::from)?;
        guard.tmp_present = false;
    }
    drop(file);
    drop(guard);
    fsync(dir_fd).map_err(io::Error::from)
}

// Payloads: little-endian integers, strings as a u32 length and UTF-8 bytes.

const TAG_TX: u8 = 1;
const TAG_SAVING: u8 = 2;
/// A transaction made in an operation: `TAG_TX`'s payload, then the
/// operation's runtime and serial. Journals written before operations
/// have only `TAG_TX`, whose transactions were made in none.
const TAG_TX_IN: u8 = 3;
const OP_RETAIN: u8 = 0;
const OP_DELETE: u8 = 1;
const OP_INSERT: u8 = 2;

fn encode(record: &Record) -> Vec<u8> {
    let mut out = Vec::new();
    match record {
        Record::Tx { tx, kind } => {
            out.push(if tx.operation.is_some() { TAG_TX_IN } else { TAG_TX });
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
            if let Some(op) = tx.operation {
                out.extend_from_slice(&op.runtime.to_le_bytes());
                out.extend_from_slice(&op.serial.to_le_bytes());
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
        tag @ (TAG_TX | TAG_TX_IN) => {
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
            let operation = if tag == TAG_TX_IN { Some(OperationId { runtime: r.u64()?, serial: r.u64()? }) } else { None };
            Record::Tx { tx: Transaction { base, actor, changes, group, operation }, kind }
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
        let private_target = dir.path().join("private");
        fs::write(&reference, b"normal creation").unwrap();
        write_atomically(&target, b"atomic creation").unwrap();
        write_atomically_if_absent(&private_target, b"secret", true).unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode(&target), mode(&reference));
        assert_eq!(mode(&private_target), 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn write_atomically_strips_special_bits_on_replacement() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("special");
        fs::write(&target, b"old").unwrap();
        // Set setuid + setgid + sticky + 0o755 if permitted by the filesystem.
        fs::set_permissions(&target, fs::Permissions::from_mode(0o7755)).unwrap();
        let before = fs::metadata(&target).unwrap().permissions().mode() & 0o7777;
        assert_ne!(before & 0o7000, 0, "expected at least one special bit set before replacement");
        write_atomically(&target, b"new").unwrap();
        let after = fs::metadata(&target).unwrap().permissions().mode() & 0o7777;
        assert_eq!(after, 0o755, "special bits must be stripped on replacement, got {after:#o}");
        assert_eq!(fs::read(&target).unwrap(), b"new");
    }

    #[cfg(unix)]
    #[test]
    fn write_atomically_fails_closed_when_destination_mode_inode_or_presence_changes() {
        use rustix::fs::{Mode, OFlags, openat};
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let dir_fd = openat(rustix::fs::CWD, dir.path(), OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC, Mode::empty()).unwrap();

        // 1. Mode narrowed from 0o644 to 0o600 mid-save must fail closed and keep 0o600 + old bytes.
        let narrowed = dir.path().join("narrowed");
        fs::write(&narrowed, b"original").unwrap();
        fs::set_permissions(&narrowed, fs::Permissions::from_mode(0o644)).unwrap();
        let err = write_atomically_in_dir_inner(&dir_fd, std::ffi::OsStr::new("narrowed"), b"updated", false, false, false, || {
            fs::set_permissions(&narrowed, fs::Permissions::from_mode(0o600)).unwrap();
        })
        .unwrap_err();
        assert!(err.to_string().contains("changed on disk"), "unexpected error: {err}");
        assert_eq!(fs::metadata(&narrowed).unwrap().permissions().mode() & 0o7777, 0o600);
        assert_eq!(fs::read(&narrowed).unwrap(), b"original");

        // 2. Destination replaced with a different inode (even with identical mode) fails closed.
        let swapped = dir.path().join("swapped");
        fs::write(&swapped, b"v1").unwrap();
        fs::set_permissions(&swapped, fs::Permissions::from_mode(0o644)).unwrap();
        let err = write_atomically_in_dir_inner(&dir_fd, std::ffi::OsStr::new("swapped"), b"v2", false, false, false, || {
            let replacement = dir.path().join("swapped.new");
            fs::write(&replacement, b"external").unwrap();
            fs::set_permissions(&replacement, fs::Permissions::from_mode(0o644)).unwrap();
            fs::rename(&replacement, &swapped).unwrap();
        })
        .unwrap_err();
        assert!(err.to_string().contains("changed on disk"), "unexpected error: {err}");
        assert_eq!(fs::read(&swapped).unwrap(), b"external");

        // 3. Previously absent file created as 0o600 mid-save fails closed instead of publishing wider mode.
        let appeared = dir.path().join("appeared");
        let err = write_atomically_in_dir_inner(&dir_fd, std::ffi::OsStr::new("appeared"), b"public-attempt", false, false, false, || {
            fs::write(&appeared, b"private-concurrent").unwrap();
            fs::set_permissions(&appeared, fs::Permissions::from_mode(0o600)).unwrap();
        })
        .unwrap_err();
        assert!(err.to_string().contains("changed on disk"), "unexpected error: {err}");
        assert_eq!(fs::metadata(&appeared).unwrap().permissions().mode() & 0o7777, 0o600);
        assert_eq!(fs::read(&appeared).unwrap(), b"private-concurrent");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn write_atomically_preserves_destination_acl_and_blocks_dir_default_acl_on_replacement() {
        use rustix::fs::{Mode, OFlags, XattrFlags, fsetxattr, openat, setxattr};
        use std::os::unix::fs::PermissionsExt;

        fn encode_posix_acl(entries: &[(u16, u16, u32)]) -> Vec<u8> {
            let mut out = Vec::with_capacity(4 + entries.len() * 8);
            out.extend_from_slice(&2u32.to_ne_bytes());
            for &(tag, perm, id) in entries {
                out.extend_from_slice(&tag.to_ne_bytes());
                out.extend_from_slice(&perm.to_ne_bytes());
                out.extend_from_slice(&id.to_ne_bytes());
            }
            out
        }

        const ACL_USER_OBJ: u16 = 0x01;
        const ACL_USER: u16 = 0x02;
        const ACL_GROUP_OBJ: u16 = 0x04;
        const ACL_MASK: u16 = 0x10;
        const ACL_OTHER: u16 = 0x20;
        const UNDEFINED_ID: u32 = u32::MAX;

        let dir = tempfile::tempdir().unwrap();
        let dir_fd = openat(rustix::fs::CWD, dir.path(), OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC, Mode::empty()).unwrap();

        // Create files BEFORE adding the directory default ACL so `plain` has
        // no extended ACL of its own while `custom` has its own distinct ACL.
        let plain = dir.path().join("plain");
        fs::write(&plain, b"plain-v1").unwrap();
        fs::set_permissions(&plain, fs::Permissions::from_mode(0o640)).unwrap();

        let custom = dir.path().join("custom");
        fs::write(&custom, b"custom-v1").unwrap();
        let bob_uid = 65533u32;
        let bob_acl = encode_posix_acl(&[
            (ACL_USER_OBJ, 6, UNDEFINED_ID),
            (ACL_USER, 4, bob_uid),
            (ACL_GROUP_OBJ, 0, UNDEFINED_ID),
            (ACL_MASK, 4, UNDEFINED_ID),
            (ACL_OTHER, 0, UNDEFINED_ID),
        ]);
        match setxattr(&custom, POSIX_ACL_ACCESS_XATTR, &bob_acl, XattrFlags::empty()) {
            Ok(()) => {}
            Err(rustix::io::Errno::OPNOTSUPP | rustix::io::Errno::NOSYS) => return,
            Err(e) => panic!("failed to set destination ACL: {e}"),
        }

        // Attach a directory default ACL granting uid 65534 rwx.
        let mallory_uid = 65534u32;
        let dir_default_acl = encode_posix_acl(&[
            (ACL_USER_OBJ, 7, UNDEFINED_ID),
            (ACL_USER, 7, mallory_uid),
            (ACL_GROUP_OBJ, 5, UNDEFINED_ID),
            (ACL_MASK, 7, UNDEFINED_ID),
            (ACL_OTHER, 5, UNDEFINED_ID),
        ]);
        fsetxattr(&dir_fd, POSIX_ACL_DEFAULT_XATTR, &dir_default_acl, XattrFlags::empty()).unwrap();

        let read_path_acl = |p: &Path| {
            let f = File::open(p).unwrap();
            read_fd_posix_acl(&f).unwrap()
        };

        // 1. Replacing `plain` (0o640, no ACL) must NOT inherit or unmask the
        //    directory default ACL's mallory entry.
        write_atomically_in_dir(&dir_fd, std::ffi::OsStr::new("plain"), b"plain-v2", false, false, false).unwrap();
        assert_eq!(fs::read(&plain).unwrap(), b"plain-v2");
        assert_eq!(fs::metadata(&plain).unwrap().permissions().mode() & 0o7777, 0o640);
        assert_eq!(read_path_acl(&plain), None, "plain replacement must not gain an extended ACL");

        // 2. Replacing `custom` (bob ACL) must preserve bob's ACL and not adopt mallory's.
        write_atomically_in_dir(&dir_fd, std::ffi::OsStr::new("custom"), b"custom-v2", false, false, false).unwrap();
        assert_eq!(fs::read(&custom).unwrap(), b"custom-v2");
        assert_eq!(fs::metadata(&custom).unwrap().permissions().mode() & 0o7777, 0o640);
        assert_eq!(read_path_acl(&custom), Some(bob_acl.clone()));

        // 3. Mid-save destination ACL mutation must fail closed and keep the mutated ACL.
        let carol_uid = 65532u32;
        let carol_acl = encode_posix_acl(&[
            (ACL_USER_OBJ, 6, UNDEFINED_ID),
            (ACL_USER, 4, carol_uid),
            (ACL_GROUP_OBJ, 0, UNDEFINED_ID),
            (ACL_MASK, 4, UNDEFINED_ID),
            (ACL_OTHER, 0, UNDEFINED_ID),
        ]);
        let err = write_atomically_in_dir_inner(&dir_fd, std::ffi::OsStr::new("custom"), b"custom-stale", false, false, false, || {
            setxattr(&custom, POSIX_ACL_ACCESS_XATTR, &carol_acl, XattrFlags::empty()).unwrap();
        })
        .unwrap_err();
        assert!(err.to_string().contains("changed on disk"), "unexpected error: {err}");
        assert_eq!(fs::read(&custom).unwrap(), b"custom-v2");
        assert_eq!(read_path_acl(&custom), Some(carol_acl));

        // 4. Forced-private replacement strips any existing ACL and leaves 0o600 with no ACL.
        write_atomically_in_dir(&dir_fd, std::ffi::OsStr::new("custom"), b"custom-private", true, true, false).unwrap();
        assert_eq!(fs::read(&custom).unwrap(), b"custom-private");
        assert_eq!(fs::metadata(&custom).unwrap().permissions().mode() & 0o7777, 0o600);
        assert_eq!(read_path_acl(&custom), None, "forced-private replacement must strip ACLs");

        // 5. Brand-new private file in a default-ACL dir is born 0o600 with no extended ACL.
        let new_priv = dir.path().join("new-private");
        write_atomically_in_dir(&dir_fd, std::ffi::OsStr::new("new-private"), b"secret", true, false, false).unwrap();
        assert_eq!(fs::metadata(&new_priv).unwrap().permissions().mode() & 0o7777, 0o600);
        assert_eq!(read_path_acl(&new_priv), None, "new private file must not inherit default ACL");

        // 6. Brand-new non-private file inherits the directory's default ACL.
        let new_pub = dir.path().join("new-public");
        write_atomically_in_dir(&dir_fd, std::ffi::OsStr::new("new-public"), b"public", false, false, false).unwrap();
        let inherited = read_path_acl(&new_pub).expect("new non-private file should inherit directory default ACL");
        let has_mallory = inherited[4..].as_chunks::<8>().0.iter().any(|entry| {
            let tag = u16::from_ne_bytes([entry[0], entry[1]]);
            let id = u32::from_ne_bytes([entry[4], entry[5], entry[6], entry[7]]);
            tag == ACL_USER && id == mallory_uid
        });
        assert!(has_mallory, "expected inherited default ACL to contain mallory uid");
    }

    #[cfg(unix)]
    #[test]
    fn write_atomically_preserves_destination_ownership_and_fails_closed_when_chown_fails() {
        use rustix::fs::{Gid, Mode, OFlags, chown, openat};
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        // 1. Unconditional verification of establish_replacement_ownership failure paths:
        //    both a failing fchown (EPERM) and a silent no-op fchown must fail closed.
        let dir = tempfile::tempdir().unwrap();
        let probe_path = dir.path().join("probe");
        let probe = File::create(&probe_path).unwrap();
        let probe_meta = probe.metadata().unwrap();
        let wrong_uid = probe_meta.uid().wrapping_add(1);
        let wrong_gid = probe_meta.gid().wrapping_add(1);

        let err = establish_replacement_ownership(&probe, wrong_uid, probe_meta.gid(), |_f, uid, gid| {
            assert_eq!(uid.map(|u| u.as_raw()), Some(wrong_uid));
            assert_eq!(gid, None);
            Err(io::Error::from(rustix::io::Errno::PERM))
        })
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);

        let err = establish_replacement_ownership(&probe, probe_meta.uid(), wrong_gid, |_f, uid, gid| {
            assert_eq!(uid, None);
            assert_eq!(gid.map(|g| g.as_raw()), Some(wrong_gid));
            Ok(())
        })
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);

        // 2. Real setgid directory with two distinct groups (when the process belongs
        //    to at least two groups): verify replacement preserves GID + mode + POSIX ACL,
        //    refuses chown EPERM without touching destination or leaving staging dirs, and
        //    aborts on mid-save chgrp.
        let egid = rustix::process::getegid();
        let groups = rustix::process::getgroups().unwrap_or_default();
        let Some(other_gid) = groups.into_iter().find(|&g| g != egid) else {
            return;
        };

        let setgid_dir = dir.path().join("setgid");
        fs::create_dir(&setgid_dir).unwrap();
        chown(&setgid_dir, None, Some(other_gid)).unwrap();
        fs::set_permissions(&setgid_dir, fs::Permissions::from_mode(0o2755)).unwrap();
        let setgid_dir_fd =
            openat(rustix::fs::CWD, &setgid_dir, OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC, Mode::empty()).unwrap();

        // Confirm newly created files inside setgid_dir really inherit other_gid.
        let check_inherit = setgid_dir.join("inherit-check");
        fs::write(&check_inherit, b"check").unwrap();
        assert_eq!(fs::metadata(&check_inherit).unwrap().gid(), other_gid.as_raw());

        // Plain 0o640 file owned by egid inside a setgid other_gid directory:
        // replacement must keep egid and 0o640 rather than leaking access to other_gid.
        let plain = setgid_dir.join("plain");
        fs::write(&plain, b"plain-v1").unwrap();
        chown(&plain, None, Some(egid)).unwrap();
        fs::set_permissions(&plain, fs::Permissions::from_mode(0o640)).unwrap();

        write_atomically(&plain, b"plain-v2").unwrap();
        let plain_meta = fs::metadata(&plain).unwrap();
        assert_eq!(fs::read(&plain).unwrap(), b"plain-v2");
        assert_eq!(plain_meta.gid(), egid.as_raw(), "replacement must preserve destination GID");
        assert_eq!(plain_meta.permissions().mode() & 0o7777, 0o640);

        // Linux extended POSIX ACL with ACL_GROUP_OBJ in the setgid directory:
        // replacement must preserve BOTH destination GID and the ACL.
        #[cfg(target_os = "linux")]
        {
            use rustix::fs::{XattrFlags, setxattr};
            fn encode_posix_acl(entries: &[(u16, u16, u32)]) -> Vec<u8> {
                let mut out = Vec::with_capacity(4 + entries.len() * 8);
                out.extend_from_slice(&2u32.to_ne_bytes());
                for &(tag, perm, id) in entries {
                    out.extend_from_slice(&tag.to_ne_bytes());
                    out.extend_from_slice(&perm.to_ne_bytes());
                    out.extend_from_slice(&id.to_ne_bytes());
                }
                out
            }
            let acl_file = setgid_dir.join("acl-file");
            fs::write(&acl_file, b"acl-v1").unwrap();
            chown(&acl_file, None, Some(egid)).unwrap();
            let group_acl = encode_posix_acl(&[
                (0x01, 6, u32::MAX), // ACL_USER_OBJ rw-
                (0x02, 4, 65533),    // ACL_USER r--
                (0x04, 4, u32::MAX), // ACL_GROUP_OBJ r-- (must stay bound to egid)
                (0x10, 4, u32::MAX), // ACL_MASK r--
                (0x20, 0, u32::MAX), // ACL_OTHER ---
            ]);
            if setxattr(&acl_file, POSIX_ACL_ACCESS_XATTR, &group_acl, XattrFlags::empty()).is_ok() {
                write_atomically(&acl_file, b"acl-v2").unwrap();
                let acl_meta = fs::metadata(&acl_file).unwrap();
                assert_eq!(fs::read(&acl_file).unwrap(), b"acl-v2");
                assert_eq!(acl_meta.gid(), egid.as_raw(), "ACL replacement must preserve destination GID");
                let saved_acl = read_fd_posix_acl(File::open(&acl_file).unwrap()).unwrap();
                assert_eq!(saved_acl, Some(group_acl));
            }
        }

        // Simulated EPERM on fchown during replacement must fail closed, leave
        // the destination untouched, and clean up its staging directory.
        let mut chown_invoked = false;
        let err = write_atomically_in_dir_with_chown(
            &setgid_dir_fd,
            std::ffi::OsStr::new("plain"),
            b"should-not-publish",
            false,
            false,
            false,
            || {},
            |_file, uid, gid| {
                chown_invoked = true;
                assert_eq!(uid, None);
                assert_eq!(gid, Some(Gid::from_raw(egid.as_raw())));
                Err(io::Error::from(rustix::io::Errno::PERM))
            },
        )
        .unwrap_err();
        assert!(chown_invoked, "expected fchown hook to be invoked for GID mismatch");
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(fs::read(&plain).unwrap(), b"plain-v2");
        assert_eq!(fs::metadata(&plain).unwrap().gid(), egid.as_raw());
        let leftover_stages: Vec<_> = fs::read_dir(&setgid_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".techne-stage-"))
            .collect();
        assert!(leftover_stages.is_empty(), "leftover staging directories: {leftover_stages:?}");

        // Mid-save chgrp on the destination before publish must fail closed.
        let err =
            write_atomically_in_dir_inner(&setgid_dir_fd, std::ffi::OsStr::new("plain"), b"stale-gid-save", false, false, false, || {
                chown(&plain, None, Some(other_gid)).unwrap();
            })
            .unwrap_err();
        assert!(err.to_string().contains("changed on disk"), "unexpected error: {err}");
        assert_eq!(fs::read(&plain).unwrap(), b"plain-v2");
        assert_eq!(fs::metadata(&plain).unwrap().gid(), other_gid.as_raw());
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
        assert!(
            journal
                .save_checked(&path, 0, &hash(b"save"), false, || Ok::<(), io::Error>(()), || Ok(()))
                .unwrap_err()
                .to_string()
                .contains("broken")
        );
        assert!(journal.record_saving_at(&path, 0, &hash(b"save"), false).unwrap_err().to_string().contains("broken"));
    }

    #[test]
    fn check_no_journal_refuses_existing_or_locked_without_creating_files() {
        let lock_of = |p: &Path| {
            let mut s = p.as_os_str().to_os_string();
            s.push(".lock");
            std::path::PathBuf::from(s)
        };
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("does-not-exist").join("secret.journal");
        check_no_journal(&nested).unwrap();
        assert!(!dir.path().join("does-not-exist").exists(), "check_no_journal must not create directories");
        assert!(!lock_of(&nested).exists(), "check_no_journal must not create a lock file");

        // An active locked journal fails closed with WouldBlock.
        let path = dir.path().join("active.journal");
        let j = Journal::create(&path, 0, &hash(b"base")).unwrap();
        assert_eq!(check_no_journal(&path).unwrap_err().kind(), io::ErrorKind::WouldBlock);

        // After dropping the lock, even if the lock file is removed, an existing journal fails closed
        // without creating a new lock file.
        drop(j);
        let lp = lock_of(&path);
        let _ = fs::remove_file(&lp);
        let err = check_no_journal(&path).unwrap_err();
        assert!(err.to_string().contains("existing journal"), "unexpected err: {err}");
        assert!(!lp.exists(), "check_no_journal must not recreate lock file");

        // A journal with unsaved edits reports unsaved edits specifically.
        let path2 = dir.path().join("dirty.journal");
        let mut j2 = Journal::create(&path2, 0, &hash(b"a")).unwrap();
        let tx = Transaction {
            base: 0,
            actor: Arc::from("me"),
            changes: ChangeSet::from_edits(1, [(1..1, "b")]).unwrap(),
            group: Group::New,
            operation: None,
        };
        j2.append(&Record::Tx { tx, kind: Kind::Edit }).unwrap();
        drop(j2);
        let _ = fs::remove_file(lock_of(&path2));
        let err2 = check_no_journal(&path2).unwrap_err();
        assert!(err2.to_string().contains("unsaved edits"), "unexpected err: {err2}");
        assert!(!lock_of(&path2).exists());
    }
}
