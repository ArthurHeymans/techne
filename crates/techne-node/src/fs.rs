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

/// Why a write did not happen: the file is not as expected. The hash of
/// what it has, `None` when there is none.
pub struct Conflict(pub Option<Hash>);

/// Replace the file at `path` with `data` through a temporary file and a
/// rename, keeping its permissions, if it is as `expect` says. A symbolic
/// link is followed, its target replaced. Between the check and the
/// rename another writer can still change the file: this refuses a
/// stale save, it is not a lock. Writes this node makes are one at a
/// time, so two it is asked for do not both pass the check.
pub fn write(path: &[u8], data: &[u8], expect: Expect) -> io::Result<Result<Hash, Conflict>> {
    static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());
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
    techne_text::journal::write_atomically(&path, data)?;
    Ok(Ok(hash(data)))
}

/// The entries of the directory at `path`, sorted by name.
pub fn list(path: &[u8]) -> io::Result<Vec<DirEntry>> {
    let dir = expand(path);
    let mut entries = std::fs::read_dir(&dir)?
        .filter_map(|e| {
            let e = e.ok()?;
            // A symbolic link to a directory is one to go into.
            let directory = e.file_type().ok()?.is_dir() || (e.file_type().ok()?.is_symlink() && e.path().is_dir());
            Some(DirEntry { name: e.file_name().as_bytes().to_vec(), directory })
        })
        .collect::<Vec<_>>();
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
        let first = write(p, b"one", Expect::Absent).unwrap().ok().unwrap();
        assert!(matches!(write(p, b"two", Expect::Absent).unwrap(), Err(Conflict(Some(h))) if h == first));
        let second = write(p, b"two", Expect::Hash(first)).unwrap().ok().unwrap();
        assert!(matches!(write(p, b"three", Expect::Hash(first)).unwrap(), Err(Conflict(Some(h))) if h == second));
        assert_eq!(read(p).unwrap(), Some((b"two".to_vec(), second)));
        write(p, b"four", Expect::Any).unwrap().ok().unwrap();
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
        write(link.as_os_str().as_bytes(), b"b", Expect::Hash(hash(b"a"))).unwrap().ok().unwrap();
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read(&target).unwrap(), b"b");
        assert_eq!(target.metadata().unwrap().permissions().mode() & 0o777, 0o751);
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
}
