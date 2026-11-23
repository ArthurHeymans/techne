//! A node's file operations and the facts it reports (doc/remote.md):
//! typed, knowing neither Lisp nor the protocol's encoding. Paths are
//! bytes; `~` and relative paths start at the node's home.

use std::{
    ffi::OsStr,
    io,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::{Path, PathBuf},
};

use crate::protocol::{DirEntry, Expect, Facts, FileKind, FileStat, MAX_FRAME};

pub type Hash = [u8; 32];

/// The hash a file's revision is named by, as documents name theirs.
pub fn hash(bytes: &[u8]) -> Hash {
    techne_text::journal::hash(bytes)
}

/// The node's home directory.
pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// `path` as this node means it: `~` and `~/...` in the home directory,
/// a relative path from there.
pub fn expand(path: &[u8]) -> PathBuf {
    let path = Path::new(OsStr::from_bytes(path));
    match path.strip_prefix("~") {
        Ok(rest) => home().join(rest),
        Err(_) if path.is_relative() => home().join(path),
        Err(_) => path.to_path_buf(),
    }
}

/// What is at `path`, without following a symbolic link there.
pub fn stat(path: &[u8]) -> io::Result<Option<FileStat>> {
    let m = match std::fs::symlink_metadata(expand(path)) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let t = m.file_type();
    let kind = if t.is_file() {
        FileKind::File
    } else if t.is_dir() {
        FileKind::Directory
    } else if t.is_symlink() {
        FileKind::Symlink
    } else {
        FileKind::Other
    };
    Ok(Some(FileStat { kind, size: m.len(), modified: m.mtime(), mode: m.mode(), device: m.dev(), inode: m.ino() }))
}

/// The most bytes of a file read in one reply, below the frame limit.
pub const MAX_READ: usize = MAX_FRAME - 4096;

/// The bytes of the regular file at `path`, and their hash; `None` if
/// there is no file. It is opened without waiting, so that a FIFO with no
/// writer is refused rather than holding the node up.
pub fn read(path: &[u8]) -> io::Result<Option<(Vec<u8>, Hash)>> {
    use std::{io::Read, os::unix::fs::OpenOptionsExt};
    let path = expand(path);
    let file = match std::fs::OpenOptions::new().read(true).custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32).open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let m = file.metadata()?;
    if m.is_dir() {
        return Err(io::Error::new(io::ErrorKind::IsADirectory, "is a directory"));
    }
    if !m.file_type().is_file() {
        return Err(io::Error::other("not a regular file"));
    }
    if m.len() as usize > MAX_READ {
        return Err(io::Error::other(format!("larger than the {MAX_READ} bytes a node sends at once")));
    }
    let mut bytes = Vec::new();
    (&file).take(MAX_READ as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_READ {
        return Err(io::Error::other(format!("larger than the {MAX_READ} bytes a node sends at once")));
    }
    let h = hash(&bytes);
    Ok(Some((bytes, h)))
}

/// Read the regular file at an already-canonicalized absolute `path` via
/// directory-handle-relative no-follow traversal (`O_NOFOLLOW`), refusing any
/// symlink at a parent component or at the leaf.
pub fn read_pinned(path: &[u8]) -> io::Result<Option<(Vec<u8>, Hash)>> {
    let path = Path::new(OsStr::from_bytes(path));
    match techne_text::document::read_pinned_bytes(path, Some(MAX_READ), "reading")? {
        Some(bytes) => {
            let h = hash(&bytes);
            Ok(Some((bytes, h)))
        }
        None => Ok(None),
    }
}

/// Why a write did not happen: the file is not as expected. The hash of
/// what it has, `None` when there is none.
#[derive(Debug)]
pub struct Conflict(pub Option<Hash>);

/// The canonical path of `path` on this node, following symlinks (and
/// canonicalizing the existing parent when the leaf does not exist yet).
pub fn canonical_path(path: &[u8]) -> io::Result<PathBuf> {
    techne_text::document::file_path(&expand(path))
}

static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Replace the file at `path` with `data` through a temporary file and a
/// rename, keeping its permissions, if it is as `expect` says. A symbolic
/// link is followed, its target replaced. When `private` is true, the written
/// file always gets mode `0o600` (both new files and replacements). When
/// `expect` is `Expect::Absent`, creation uses a no-clobber persist so a
/// concurrent creator cannot be overwritten. Between the check and the rename
/// another writer can still change an existing file: this refuses a stale
/// save, it is not a lock. Writes this node makes are one at a time, so two
/// it is asked for do not both pass the check.
pub fn write(path: &[u8], data: &[u8], expect: Expect, private: bool) -> io::Result<Result<Hash, Conflict>> {
    let _one = WRITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let path = techne_text::document::file_path(&expand(path))?;
    if expect != Expect::Any {
        let now = match read(path.as_os_str().as_bytes()) {
            Ok(found) => found.map(|(_, h)| h),
            // Too large to read is not what was read: changed.
            Err(e) if e.kind() == io::ErrorKind::Other => Some([0; 32]),
            Err(e) => return Err(e),
        };
        let expected = match expect {
            Expect::Absent => None,
            Expect::Hash(h) => Some(h),
            Expect::Any => unreachable!(),
        };
        if now != expected {
            return Ok(Err(Conflict(now)));
        }
    }
    if expect == Expect::Absent {
        match techne_text::journal::write_atomically_if_absent(&path, data, private) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                let now = read(path.as_os_str().as_bytes()).ok().flatten().map(|(_, h)| h);
                return Ok(Err(Conflict(now)));
            }
            Err(e) => return Err(e),
        }
    } else if private {
        techne_text::journal::write_atomically_private(&path, data)?;
    } else {
        techne_text::journal::write_atomically(&path, data)?;
    }
    Ok(Ok(hash(data)))
}

/// As `write`, for an already-classified absolute canonical `path`: never
/// re-canonicalizes `path`, traverses every parent component and opens the
/// leaf with `O_NOFOLLOW`, stream-hashes for conflict detection without
/// buffering unbounded bytes, and stages/publishes strictly relative to the
/// verified parent directory handle.
pub fn write_pinned(path: &[u8], data: &[u8], expect: Expect, private: bool) -> io::Result<Result<Hash, Conflict>> {
    let _one = WRITING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let path = Path::new(OsStr::from_bytes(path));
    let (dir_fd, leaf) = techne_text::document::open_pinned_parent_dir(path, "saving")?;

    let stream_leaf_hash = || -> io::Result<Option<Hash>> {
        match techne_text::document::open_leaf_in_dir(&dir_fd, &leaf, "saving")? {
            Some(file) => match techne_text::document::stream_regular_file_hash(file, Some(MAX_READ))? {
                Some(h) => Ok(Some(h)),
                // Too large to read is not what was read: changed.
                None => Ok(Some([0; 32])),
            },
            None => Ok(None),
        }
    };

    if expect != Expect::Any {
        let now = stream_leaf_hash()?;
        let expected = match expect {
            Expect::Absent => None,
            Expect::Hash(h) => Some(h),
            Expect::Any => unreachable!(),
        };
        if now != expected {
            return Ok(Err(Conflict(now)));
        }
    } else if let Some(file) = techne_text::document::open_leaf_in_dir(&dir_fd, &leaf, "saving")? {
        let kind = file.metadata()?.file_type();
        if kind.is_dir() {
            return Err(io::Error::new(io::ErrorKind::IsADirectory, "is a directory"));
        }
        if !kind.is_file() {
            return Err(io::Error::other("not a regular file"));
        }
    }

    let no_clobber = expect == Expect::Absent;
    match techne_text::journal::write_atomically_in_dir(&dir_fd, &leaf, data, private, private && !no_clobber, no_clobber) {
        Ok(()) => Ok(Ok(hash(data))),
        Err(e) if no_clobber && e.kind() == io::ErrorKind::AlreadyExists => {
            let now = stream_leaf_hash().ok().flatten();
            Ok(Err(Conflict(now)))
        }
        Err(e) => Err(e),
    }
}

/// The entries of the directory at `path`, sorted by name.
pub fn list(path: &[u8]) -> io::Result<Vec<DirEntry>> {
    let dir = expand(path);
    let mut entries = std::fs::read_dir(&dir)?
        .map(|e| {
            let e = e?;
            let t = e.file_type()?;
            // A symbolic link to a directory is one to go into.
            let directory = t.is_dir() || (t.is_symlink() && e.path().is_dir());
            Ok(DirEntry { name: e.file_name().as_bytes().to_vec(), directory })
        })
        .collect::<io::Result<Vec<_>>>()?;
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

/// The nearest directory from `dir` up with an entry named one of `names`.
pub fn find_up(dir: &[u8], names: &[String]) -> Option<PathBuf> {
    expand(dir).ancestors().find(|d| names.iter().any(|n| d.join(n).symlink_metadata().is_ok())).map(Path::to_path_buf)
}

fn trimmed(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn namespace(kind: &str) -> Option<u64> {
    std::fs::metadata(format!("/proc/self/ns/{kind}")).ok().map(|m| m.ino())
}

/// The arguments after the program that start this node (`node`, for
/// `techne node`), which `cli::main` sets.
pub static ARGUMENTS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();

/// What this node is.
pub fn facts() -> Facts {
    let uid = rustix::process::getuid().as_raw();
    let (user, shell) = std::fs::read_to_string("/etc/passwd")
        .ok()
        .and_then(|passwd| {
            passwd
                .lines()
                .map(|l| l.split(':').collect::<Vec<_>>())
                .find(|f| f.len() >= 7 && f[2].parse() == Ok(uid))
                .map(|f| (Some(f[0].to_string()), Some(f[6].to_string()).filter(|s| !s.is_empty())))
        })
        .unwrap_or((None, None));
    Facts {
        machine: trimmed("/etc/machine-id"),
        boot: trimmed("/proc/sys/kernel/random/boot_id"),
        uid,
        user: user.or_else(|| std::env::var("USER").ok()),
        home: home().as_os_str().as_bytes().to_vec(),
        shell: shell.or_else(|| std::env::var("SHELL").ok()),
        mount_namespace: namespace("mnt"),
        pid_namespace: namespace("pid"),
        arch: std::env::consts::ARCH.to_string(),
        os: std::env::consts::OS.to_string(),
        host: trimmed("/proc/sys/kernel/hostname"),
        program: std::env::current_exe().map(|p| p.into_os_string().into_encoded_bytes()).unwrap_or_default(),
        arguments: ARGUMENTS.get().cloned().unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_write_replaces_only_what_it_expects() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        let p = path.as_os_str().as_bytes();
        assert!(read(p).unwrap().is_none());
        let first = write(p, b"one", Expect::Absent, false).unwrap().ok().unwrap();
        assert!(matches!(write(p, b"two", Expect::Absent, false).unwrap(), Err(Conflict(Some(h))) if h == first));
        let second = write(p, b"two", Expect::Hash(first), false).unwrap().ok().unwrap();
        assert!(matches!(write(p, b"three", Expect::Hash(first), false).unwrap(), Err(Conflict(Some(h))) if h == second));
        assert_eq!(read(p).unwrap(), Some((b"two".to_vec(), second)));
        write(p, b"four", Expect::Any, false).unwrap().ok().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"four");
    }

    #[test]
    fn a_write_follows_a_link_and_keeps_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        std::fs::write(&target, "a").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o751)).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        write(link.as_os_str().as_bytes(), b"b", Expect::Hash(hash(b"a")), false).unwrap().ok().unwrap();
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read(&target).unwrap(), b"b");
        assert_eq!(target.metadata().unwrap().permissions().mode() & 0o777, 0o751);
        assert_eq!(canonical_path(link.as_os_str().as_bytes()).unwrap(), std::fs::canonicalize(&target).unwrap());
    }

    #[test]
    fn a_private_write_enforces_0600_on_create_and_replace() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret");
        write(path.as_os_str().as_bytes(), b"cipher", Expect::Absent, true).unwrap().ok().unwrap();
        assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        // Existing overly-permissive file is tightened to 0o600 on private replacement.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        write(path.as_os_str().as_bytes(), b"cipher2", Expect::Hash(hash(b"cipher")), true).unwrap().ok().unwrap();
        assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn paths_start_at_home_and_roots_are_found_upwards() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("p/.git")).unwrap();
        std::fs::create_dir_all(dir.path().join("p/a/b")).unwrap();
        let from = dir.path().join("p/a/b");
        assert_eq!(find_up(from.as_os_str().as_bytes(), &[".jj".into(), ".git".into()]), Some(dir.path().join("p")));
        assert_eq!(expand(b"~/x"), home().join("x"));
        assert_eq!(expand(b"x"), home().join("x"));
        assert_eq!(expand(b"/x"), PathBuf::from("/x"));
        let names: Vec<_> = list(dir.path().join("p").as_os_str().as_bytes()).unwrap().into_iter().map(|e| (e.name, e.directory)).collect();
        assert_eq!(names, vec![(b".git".to_vec(), true), (b"a".to_vec(), true)]);
    }

    #[test]
    fn pinned_read_and_write_refuse_leaf_and_parent_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let pub_dir = root.join("pub");
        let priv_dir = root.join("priv");
        std::fs::create_dir(&pub_dir).unwrap();
        std::fs::create_dir(&priv_dir).unwrap();

        let pub_file = pub_dir.join("note.txt");
        let priv_file = priv_dir.join("secret.txt");
        std::fs::write(&pub_file, "public").unwrap();
        std::fs::write(&priv_file, "SECRET").unwrap();

        // Real canonical path works for pinned read and write.
        let (bytes, h) = read_pinned(pub_file.as_os_str().as_bytes()).unwrap().unwrap();
        assert_eq!(bytes, b"public");
        let h2 = write_pinned(pub_file.as_os_str().as_bytes(), b"updated", Expect::Hash(h), false).unwrap().ok().unwrap();
        assert_eq!(h2, hash(b"updated"));

        // Relative path refused.
        assert!(read_pinned(b"relative.txt").is_err());
        assert!(write_pinned(b"relative.txt", b"x", Expect::Any, false).is_err());

        // Leaf symlink swap refused for both pinned read and write (including Expect::Any).
        std::fs::remove_file(&pub_file).unwrap();
        std::os::unix::fs::symlink(&priv_file, &pub_file).unwrap();
        assert!(
            read_pinned(pub_file.as_os_str().as_bytes()).unwrap_err().to_string().contains("file target changed on disk while reading")
        );
        assert!(
            write_pinned(pub_file.as_os_str().as_bytes(), b"overwrite", Expect::Any, false)
                .unwrap_err()
                .to_string()
                .contains("file target changed on disk while saving")
        );
        assert_eq!(std::fs::read_to_string(&priv_file).unwrap(), "SECRET");

        // Parent directory symlink swap refused for both pinned read and write.
        let pub_bak = root.join("pub.bak");
        let priv_same = priv_dir.join("same.txt");
        std::fs::write(&priv_same, "SECRET-SAME").unwrap();
        std::fs::rename(&pub_dir, &pub_bak).unwrap();
        std::os::unix::fs::symlink(&priv_dir, &pub_dir).unwrap();
        let swapped = pub_dir.join("same.txt");
        assert!(read_pinned(swapped.as_os_str().as_bytes()).unwrap_err().to_string().contains("file target changed on disk while reading"));
        assert!(
            write_pinned(swapped.as_os_str().as_bytes(), b"pwn", Expect::Hash(hash(b"SECRET-SAME")), false)
                .unwrap_err()
                .to_string()
                .contains("file target changed on disk while saving")
        );
        assert_eq!(std::fs::read_to_string(&priv_same).unwrap(), "SECRET-SAME");
    }
}
