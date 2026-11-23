//! Files and directories beyond reading and writing one: what an entry
//! is, listing, making, moving, copying and deleting entries, for a
//! directory editor (`lisp/editor/packages/dired.scm`) and for scripts
//! (`techne run`) alike. Nothing is replaced: an operation whose
//! destination exists is refused, so that asking before overwriting
//! stays the Lisp's to do.

use std::{
    fs,
    io::{self, Read},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
    time::UNIX_EPOCH,
};

use crate::data::Data;

/// What is at `path`, a symbolic link not followed: (kind size modified
/// mode target), or `None` when nothing is there. `kind` is `file`,
/// `directory`, `symlink` or `other`; `modified` in seconds since the
/// epoch; `mode` the permission bits; `target` a link's, else #f.
pub fn attributes(path: &Path) -> io::Result<Option<Data>> {
    let m = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let kind = match m.file_type() {
        t if t.is_symlink() => "symlink",
        t if t.is_dir() => "directory",
        t if t.is_file() => "file",
        _ => "other",
    };
    let modified = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64);
    let target = match kind {
        "symlink" => fs::read_link(path).map_or(Data::Bool(false), |t| Data::String(t.to_string_lossy().into_owned())),
        _ => Data::Bool(false),
    };
    Ok(Some(Data::List(vec![
        Data::Symbol(kind.into()),
        Data::Int(m.size() as i64),
        Data::Int(modified),
        Data::Int(i64::from(m.permissions().mode() & 0o7777)),
        target,
    ])))
}

/// `path` absolute, its symbolic links and `.` and `..` resolved; a file
/// that does not exist yet is named in its directory's true name.
pub fn truename(path: &Path) -> io::Result<std::path::PathBuf> {
    match fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
            let name = path.file_name().ok_or_else(|| io::Error::other("no file name"))?;
            Ok(fs::canonicalize(parent)?.join(name))
        }
        Err(e) => Err(e),
    }
}

/// The entries of `dir`, sorted, a directory's name with a slash after it.
/// One that cannot be read is an error, not a listing short of it.
pub fn list(dir: &Path) -> io::Result<Vec<String>> {
    let mut names = fs::read_dir(dir)?
        .map(|e| {
            let e = e?;
            let dir = e.path().is_dir();
            Ok(e.file_name().to_string_lossy().into_owned() + if dir { "/" } else { "" })
        })
        .collect::<io::Result<Vec<String>>>()?;
    names.sort();
    Ok(names)
}

/// Make the directory `path`, with permission bits `mode` less the umask,
/// in its parent, which is there: an error if anything is at `path`.
pub fn create_directory(path: &Path, mode: u32) -> io::Result<()> {
    std::os::unix::fs::DirBuilderExt::mode(&mut fs::DirBuilder::new(), mode).create(path)
}

fn exists(to: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(to) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

fn already_exists() -> io::Error {
    io::Error::new(io::ErrorKind::AlreadyExists, "already exists")
}

/// Write `text` to `path` in place of what is there, at once: whole to a
/// new file beside it, readable by its user only, then renamed over it.
/// A write that fails leaves the old file as it was, and no new one.
pub fn replace(path: &Path, text: &str) -> io::Result<()> {
    use std::{
        io::Write,
        sync::atomic::{AtomicU64, Ordering},
    };
    // A name of its own for each write, in this process and others; one
    // taken, by a process that crashed with the same id say, is passed
    // over, never removed.
    static WRITES: AtomicU64 = AtomicU64::new(0);
    let file_name = path.file_name().ok_or_else(|| io::Error::other("no file name"))?.to_os_string();
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let (new, mut file) = std::iter::repeat_with(|| {
        let mut name = file_name.clone();
        name.push(format!(".{}.{}.new", std::process::id(), WRITES.fetch_add(1, Ordering::Relaxed)));
        let new = path.with_file_name(name);
        options.open(&new).map(|f| (new, f))
    })
    .take(16)
    .find(|made| !made.as_ref().is_err_and(|e| e.kind() == io::ErrorKind::AlreadyExists))
    .unwrap_or_else(|| Err(io::Error::other("no free name beside it")))?;
    let written = file.write_all(text.as_bytes()).and_then(|()| file.sync_all()).and_then(|()| std::fs::rename(&new, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&new);
    }
    written
}

/// Move `from` to `to`, which must not exist: never replaced, even when it
/// appears meanwhile, where the system and file system can refuse to
/// replace it (renameat2's RENAME_NOREPLACE); elsewhere checked first.
/// Across file systems it is copied, then deleted once the copy is whole.
pub fn rename(from: &Path, to: &Path) -> io::Result<()> {
    use rustix::io::Errno;
    match rename_noreplace(from, to) {
        Ok(()) => Ok(()),
        Err(Errno::EXIST) => Err(already_exists()),
        Err(Errno::XDEV) => {
            copy(from, to)?;
            delete(from, true)
        }
        // A file system or a system without RENAME_NOREPLACE: checked,
        // then renamed. Not atomic: what another process makes at `to` in
        // between is replaced. Nothing better is there to do it with.
        Err(Errno::INVAL | Errno::NOSYS | Errno::OPNOTSUPP) => {
            if exists(to)? {
                return Err(already_exists());
            }
            fs::rename(from, to)
        }
        Err(e) => Err(e.into()),
    }
}

/// renameat2's RENAME_NOREPLACE, where the system has it.
#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple", target_os = "redox"))]
fn rename_noreplace(from: &Path, to: &Path) -> rustix::io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE)
}

/// Elsewhere, as on a file system without it: `rename` checks first.
#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple", target_os = "redox")))]
fn rename_noreplace(_: &Path, _: &Path) -> rustix::io::Result<()> {
    Err(rustix::io::Errno::NOSYS)
}

/// Copy `from` to `to`, which must not exist: a directory with all it
/// holds, a symbolic link as a link. Nothing is replaced: each file and
/// directory is created anew. A directory is not copied into itself; a
/// copy that fails part way is taken back.
pub fn copy(from: &Path, to: &Path) -> io::Result<()> {
    if fs::symlink_metadata(from)?.is_dir() {
        let source = fs::canonicalize(from)?;
        if truename(to)?.starts_with(&source) {
            return Err(io::Error::other("a directory cannot be copied into itself"));
        }
    }
    // Only what this made is taken back, never what appeared meanwhile.
    let mut made = false;
    copy_new(from, to, &mut made).inspect_err(|_| {
        if made {
            let _ = delete(to, true);
        }
    })
}

/// Copy `from` to `to`, each created anew; `made` once `to` is.
fn copy_new(from: &Path, to: &Path, made: &mut bool) -> io::Result<()> {
    let m = fs::symlink_metadata(from)?;
    if m.file_type().is_symlink() {
        std::os::unix::fs::symlink(fs::read_link(from)?, to)?;
        *made = true;
        Ok(())
    } else if m.is_dir() {
        // Its owner's only until it is whole: then the source's mode.
        std::os::unix::fs::DirBuilderExt::mode(&mut fs::DirBuilder::new(), 0o700).create(to)?;
        *made = true;
        fs::read_dir(from)?.try_for_each(|e| {
            let e = e?;
            copy_new(&e.path(), &to.join(e.file_name()), &mut false)
        })?;
        fs::set_permissions(to, m.permissions())
    } else if m.is_file() {
        use std::os::unix::fs::OpenOptionsExt;
        // Readable by no more than the source while it is written.
        let mut out = fs::OpenOptions::new().write(true).create_new(true).mode(m.permissions().mode() & 0o777).open(to)?;
        *made = true;
        io::copy(&mut fs::File::open(from)?, &mut out)?;
        out.set_permissions(m.permissions())
    } else {
        // A FIFO or a device would block or never end.
        Err(io::Error::other(format!("{}: not a regular file", from.display())))
    }
}

/// Delete `path`: a directory only when it is empty, unless `recursive`.
/// A symbolic link is deleted, not what it points to.
pub fn delete(path: &Path, recursive: bool) -> io::Result<()> {
    let m = fs::symlink_metadata(path)?;
    match (m.is_dir(), recursive) {
        (false, _) => fs::remove_file(path),
        (true, false) => fs::remove_dir(path),
        (true, true) => fs::remove_dir_all(path),
    }
}

/// The first `bytes` bytes of the file at `path` as text, what is not
/// UTF-8 replaced; a character cut at the end is left out.
/// Only a regular file is read, so that a FIFO cannot block.
pub fn head(path: &Path, bytes: usize) -> io::Result<String> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = fs::OpenOptions::new().read(true).custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32).open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("not a regular file"));
    }
    let mut buf = Vec::with_capacity(bytes.min(1 << 20));
    file.take(bytes as u64).read_to_end(&mut buf)?;
    let end = match std::str::from_utf8(&buf) {
        Err(e) if e.error_len().is_none() => e.valid_up_to(),
        _ => buf.len(),
    };
    Ok(String::from_utf8_lossy(&buf[..end]).into_owned())
}

/// How `open_pinned_dir` reacts when a directory component does not exist.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RootMissing {
    Error,
    ReturnNone,
}

/// Validate that `leaf` is a single normal path component (no slashes, `.`, or `..`).
pub fn validate_leaf_name(leaf: &Path) -> io::Result<&std::ffi::OsStr> {
    use std::path::Component;
    let mut comps = leaf.components();
    match (comps.next(), comps.next()) {
        (Some(Component::Normal(name)), None) if !name.is_empty() => Ok(name),
        _ => Err(io::Error::new(io::ErrorKind::InvalidInput, "leaf must be a single file name")),
    }
}

/// Open `root` by walking each path component from `/` with
/// `O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC`, refusing any symlink
/// or `..` component.
pub fn open_pinned_dir(root: &Path, missing: RootMissing) -> io::Result<Option<rustix::fd::OwnedFd>> {
    use rustix::fs::{Mode, OFlags, openat};
    use std::path::Component;

    if !root.is_absolute() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "root must be an absolute path"));
    }
    let mut cur =
        openat(rustix::fs::CWD, "/", OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC, Mode::empty()).map_err(io::Error::from)?;
    for comp in root.components() {
        match comp {
            Component::RootDir | Component::CurDir => {}
            Component::ParentDir | Component::Prefix(_) => {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "root must not contain parent components"));
            }
            Component::Normal(seg) => {
                match openat(&cur, seg, OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty()) {
                    Ok(next) => cur = next,
                    Err(rustix::io::Errno::NOENT) if missing == RootMissing::ReturnNone => return Ok(None),
                    Err(rustix::io::Errno::NOENT) => {
                        return Err(io::Error::new(io::ErrorKind::NotFound, "allowed root does not exist"));
                    }
                    Err(rustix::io::Errno::LOOP | rustix::io::Errno::NOTDIR | rustix::io::Errno::MLINK) => {
                        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "allowed root contains a symlink or non-directory"));
                    }
                    Err(e) => return Err(io::Error::from(e)),
                }
            }
        }
    }
    Ok(Some(cur))
}

/// Open `root` via no-follow traversal from `/`, creating any missing tail
/// directories strictly relative to verified parent fds with `create_mode`.
/// Refuses any symlink, non-directory, or `..` component BEFORE creating
/// anything. When `chmod_leaf` is true, verifies the final directory is owned
/// by the current effective UID and sets its mode via `fchmod`.
pub fn open_or_create_pinned_dir(root: &Path, create_mode: u32, chmod_leaf: bool) -> io::Result<rustix::fd::OwnedFd> {
    use rustix::fs::{FileType, Mode, OFlags, fchmod, fstat, mkdirat, openat};
    use std::path::Component;

    if !root.is_absolute() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "directory must be an absolute path"));
    }
    let euid = rustix::process::geteuid().as_raw();
    let mut cur =
        openat(rustix::fs::CWD, "/", OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC, Mode::empty()).map_err(io::Error::from)?;
    let mut has_normal = false;
    for comp in root.components() {
        match comp {
            Component::RootDir | Component::CurDir => {}
            Component::ParentDir | Component::Prefix(_) => {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "directory must not contain parent components"));
            }
            Component::Normal(seg) => {
                has_normal = true;
                match openat(&cur, seg, OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty()) {
                    Ok(next) => cur = next,
                    Err(rustix::io::Errno::NOENT) => {
                        match mkdirat(&cur, seg, Mode::from_raw_mode(create_mode)) {
                            Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                            Err(e) => return Err(io::Error::from(e)),
                        }
                        let next =
                            openat(&cur, seg, OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty())
                                .map_err(|e| match e {
                                    rustix::io::Errno::LOOP | rustix::io::Errno::NOTDIR | rustix::io::Errno::MLINK => {
                                        io::Error::new(io::ErrorKind::PermissionDenied, "directory contains a symlink or non-directory")
                                    }
                                    other => io::Error::from(other),
                                })?;
                        let st = fstat(&next).map_err(io::Error::from)?;
                        if FileType::from_raw_mode(st.st_mode) != FileType::Directory || st.st_uid != euid {
                            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "created directory failed ownership verification"));
                        }
                        cur = next;
                    }
                    Err(rustix::io::Errno::LOOP | rustix::io::Errno::NOTDIR | rustix::io::Errno::MLINK) => {
                        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "directory contains a symlink or non-directory"));
                    }
                    Err(e) => return Err(io::Error::from(e)),
                }
            }
        }
    }
    if chmod_leaf && has_normal {
        let st = fstat(&cur).map_err(io::Error::from)?;
        if FileType::from_raw_mode(st.st_mode) != FileType::Directory || st.st_uid != euid {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "directory is not owned by current user"));
        }
        fchmod(&cur, Mode::from_raw_mode(create_mode)).map_err(io::Error::from)?;
    }
    Ok(cur)
}

/// Read up to `max_bytes` bytes from regular file `leaf` directly inside pinned `dir_fd`
/// without following symlinks. Returns `Ok(None)` if `leaf` does not exist.
pub fn read_pinned_bytes_in_dir(dir_fd: &rustix::fd::OwnedFd, leaf: &Path, max_bytes: usize) -> io::Result<Option<Vec<u8>>> {
    use rustix::fs::{FileType, Mode, OFlags, fstat, openat};

    let name = validate_leaf_name(leaf)?;
    let fd = match openat(dir_fd, name, OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC, Mode::empty()) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(rustix::io::Errno::LOOP | rustix::io::Errno::MLINK) => {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "symlink is not allowed"));
        }
        Err(e) => return Err(io::Error::from(e)),
    };
    let st = fstat(&fd).map_err(io::Error::from)?;
    if FileType::from_raw_mode(st.st_mode) != FileType::RegularFile {
        return Err(io::Error::other("not a regular file"));
    }
    let file = fs::File::from(fd);
    let mut buf = Vec::with_capacity(max_bytes.min(64 * 1024));
    file.take(max_bytes.saturating_add(1) as u64).read_to_end(&mut buf)?;
    if buf.len() > max_bytes {
        return Err(io::Error::other(format!("file exceeds maximum size ({max_bytes} bytes)")));
    }
    Ok(Some(buf))
}

/// Atomically publish `bytes` to regular file `leaf` strictly inside pinned `dir_fd`
/// with mode `0o600` (owner-only, isolated from any Linux directory default ACL)
/// through an owner-only `0o700` staging directory inside `dir_fd`.
/// Syncs the staged file before `renameat`, but does NOT `fsync(dir_fd)` so callers
/// can track publication separately from directory durability.
pub fn write_pinned_private_file_in_dir_unsynced(dir_fd: &rustix::fd::OwnedFd, leaf: &Path, bytes: &[u8]) -> io::Result<()> {
    use rustix::fs::{AtFlags, FileType, Mode, OFlags, fstat, mkdirat, openat, renameat, statat, unlinkat};
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    static STAGE_SEQ: AtomicU64 = AtomicU64::new(0);

    let name = validate_leaf_name(leaf)?;
    let check_dest = || -> io::Result<()> {
        match statat(dir_fd, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(st) => {
                let ft = FileType::from_raw_mode(st.st_mode);
                if ft == FileType::Symlink {
                    Err(io::Error::new(io::ErrorKind::PermissionDenied, "destination is a symlink"))
                } else if ft == FileType::Directory {
                    Err(io::Error::new(io::ErrorKind::IsADirectory, "destination is a directory"))
                } else if ft != FileType::RegularFile {
                    Err(io::Error::other("destination is not a regular file"))
                } else {
                    Ok(())
                }
            }
            Err(rustix::io::Errno::NOENT) => Ok(()),
            Err(e) => Err(io::Error::from(e)),
        }
    };
    check_dest()?;

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
        let stage_name = format!(".techne-vm-stage-{pid}-{seq}");
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

    #[cfg(target_os = "linux")]
    match rustix::fs::fremovexattr(&guard.stage_fd, "system.posix_acl_default") {
        Ok(()) | Err(rustix::io::Errno::NODATA | rustix::io::Errno::OPNOTSUPP | rustix::io::Errno::NOSYS) => {}
        Err(e) => return Err(io::Error::from(e)),
    }

    let tmp_fd = openat(
        &guard.stage_fd,
        "tmp",
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )
    .map_err(io::Error::from)?;
    guard.tmp_present = true;
    let mut file = fs::File::from(tmp_fd);

    #[cfg(target_os = "linux")]
    match rustix::fs::fgetxattr(&file, "system.posix_acl_access", &mut [0u8; 64]) {
        Err(rustix::io::Errno::NODATA | rustix::io::Errno::OPNOTSUPP | rustix::io::Errno::NOSYS) => {}
        Ok(_) | Err(rustix::io::Errno::RANGE) => {
            return Err(io::Error::other("staged private file unexpectedly inherited a POSIX ACL"));
        }
        Err(e) => return Err(io::Error::from(e)),
    }

    file.write_all(bytes)?;
    file.sync_all()?;
    check_dest()?;
    renameat(&guard.stage_fd, "tmp", dir_fd, name).map_err(io::Error::from)?;
    guard.tmp_present = false;
    drop(file);
    drop(guard);
    Ok(())
}

/// Atomically write `bytes` to regular file `leaf` strictly inside pinned `dir_fd`
/// with mode `0o600` and `fsync(dir_fd)` for directory durability.
pub fn write_pinned_private_file_in_dir(dir_fd: &rustix::fd::OwnedFd, leaf: &Path, bytes: &[u8]) -> io::Result<()> {
    write_pinned_private_file_in_dir_unsynced(dir_fd, leaf, bytes)?;
    rustix::fs::fsync(dir_fd).map_err(io::Error::from)
}

/// Unlink regular file `leaf` strictly inside pinned `dir_fd` without following symlinks
/// and without calling `fsync(dir_fd)` so callers can track removal separately from
/// directory durability. Returns `Ok(true)` if removed, `Ok(false)` if absent.
pub fn delete_pinned_file_in_dir_unsynced(dir_fd: &rustix::fd::OwnedFd, leaf: &Path) -> io::Result<bool> {
    use rustix::fs::{AtFlags, FileType, statat, unlinkat};

    let name = validate_leaf_name(leaf)?;
    match statat(dir_fd, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(st) => {
            let ft = FileType::from_raw_mode(st.st_mode);
            if ft != FileType::RegularFile {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "target is not a regular file"));
            }
        }
        Err(rustix::io::Errno::NOENT) => return Ok(false),
        Err(e) => return Err(io::Error::from(e)),
    }
    match unlinkat(dir_fd, name, AtFlags::empty()) {
        Ok(()) => Ok(true),
        Err(rustix::io::Errno::NOENT) => Ok(false),
        Err(e) => Err(io::Error::from(e)),
    }
}

/// Delete regular file `leaf` strictly inside pinned `dir_fd` without following symlinks,
/// and `fsync(dir_fd)` when a file was removed.
/// Returns `Ok(true)` if removed, `Ok(false)` if absent; refuses symlinks and directories.
pub fn delete_pinned_file_in_dir(dir_fd: &rustix::fd::OwnedFd, leaf: &Path) -> io::Result<bool> {
    let removed = delete_pinned_file_in_dir_unsynced(dir_fd, leaf)?;
    if removed {
        rustix::fs::fsync(dir_fd).map_err(io::Error::from)?;
    }
    Ok(removed)
}

/// Read up to `max_bytes` raw bytes from `leaf` inside `root` without following any symlink
/// in `root` or `leaf`. Returns `Ok(None)` when `root` or `leaf` is absent.
pub fn read_confined_bytes_nofollow(root: &Path, leaf: &Path, max_bytes: usize) -> io::Result<Option<Vec<u8>>> {
    let Some(dir_fd) = open_pinned_dir(root, RootMissing::ReturnNone)? else {
        return Ok(None);
    };
    read_pinned_bytes_in_dir(&dir_fd, leaf, max_bytes)
}

/// A pinned directory descriptor opened via no-follow traversal from `/`.
/// Every operation on this handle stays anchored to the same directory inode
/// across reads, publication, unlink, `fsync`, and rollback even if the path
/// is renamed or replaced concurrently.
pub struct PinnedDir {
    dir_fd: Option<rustix::fd::OwnedFd>,
}

impl PinnedDir {
    /// Open `root` via no-follow traversal from `/`. When `create_dirs` is true,
    /// creates missing tail directories with mode `0o700` strictly relative to
    /// verified parent fds and returns `Ok(Some(_))`. When `create_dirs` is false,
    /// returns `Ok(None)` if `root` does not exist.
    pub fn open(root: &Path, create_dirs: bool) -> io::Result<Option<Self>> {
        let dir_fd = if create_dirs {
            Some(open_or_create_pinned_dir(root, 0o700, false)?)
        } else {
            open_pinned_dir(root, RootMissing::ReturnNone)?
        };
        Ok(dir_fd.map(|fd| Self { dir_fd: Some(fd) }))
    }

    fn dir_fd(&self) -> io::Result<&rustix::fd::OwnedFd> {
        self.dir_fd.as_ref().ok_or_else(|| io::Error::other("pinned directory has been closed"))
    }

    /// Explicitly close the pinned directory descriptor.
    pub fn close(&mut self) {
        self.dir_fd.take();
    }

    /// Read up to `max_bytes` raw bytes from regular file `leaf` in this pinned directory.
    pub fn read_bytes(&self, leaf: &Path, max_bytes: usize) -> io::Result<Option<Vec<u8>>> {
        read_pinned_bytes_in_dir(self.dir_fd()?, leaf, max_bytes)
    }

    /// Atomically publish `bytes` to private regular file `leaf` in this pinned directory
    /// without calling `fsync` on the directory fd.
    pub fn publish_private(&self, leaf: &Path, bytes: &[u8]) -> io::Result<()> {
        write_pinned_private_file_in_dir_unsynced(self.dir_fd()?, leaf, bytes)
    }

    /// Atomically write `bytes` to private regular file `leaf` in this pinned directory
    /// and `fsync` the directory fd.
    pub fn write_private(&self, leaf: &Path, bytes: &[u8]) -> io::Result<()> {
        write_pinned_private_file_in_dir(self.dir_fd()?, leaf, bytes)
    }

    /// Unlink regular file `leaf` in this pinned directory without calling `fsync` on the
    /// directory fd. Returns `Ok(true)` if removed, `Ok(false)` if absent.
    pub fn unlink(&self, leaf: &Path) -> io::Result<bool> {
        delete_pinned_file_in_dir_unsynced(self.dir_fd()?, leaf)
    }

    /// Delete regular file `leaf` in this pinned directory and `fsync` the directory fd
    /// when a file was removed.
    pub fn delete(&self, leaf: &Path) -> io::Result<bool> {
        delete_pinned_file_in_dir(self.dir_fd()?, leaf)
    }

    /// `fsync` this pinned directory descriptor directly.
    pub fn sync(&self) -> io::Result<()> {
        rustix::fs::fsync(self.dir_fd()?).map_err(io::Error::from)
    }

    /// Try to acquire a non-blocking exclusive advisory lock (`flock`) on this pinned
    /// directory descriptor. Returns `Ok(true)` if acquired, `Ok(false)` if another open
    /// handle (in this process or another process) currently holds the lock.
    pub fn try_lock(&self) -> io::Result<bool> {
        match rustix::fs::flock(self.dir_fd()?, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => Ok(true),
            Err(rustix::io::Errno::WOULDBLOCK) => Ok(false),
            Err(e) => Err(io::Error::from(e)),
        }
    }
}

/// Atomically write `bytes` to `leaf` inside `root` with mode `0o600` without following
/// any symlink in `root` or `leaf`. When `create_dirs` is true, creates missing tail
/// directories with mode `0o700` strictly relative to verified parent fds.
pub fn write_confined_file_private_nofollow(root: &Path, leaf: &Path, bytes: &[u8], create_dirs: bool) -> io::Result<()> {
    let dir_fd = if create_dirs {
        open_or_create_pinned_dir(root, 0o700, false)?
    } else {
        open_pinned_dir(root, RootMissing::Error)?.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "directory does not exist"))?
    };
    write_pinned_private_file_in_dir(&dir_fd, leaf, bytes)
}

/// Delete regular file `leaf` inside `root` without following any symlink in `root` or `leaf`.
/// Returns `Ok(false)` when `root` or `leaf` does not exist.
pub fn delete_confined_file_nofollow(root: &Path, leaf: &Path) -> io::Result<bool> {
    let Some(dir_fd) = open_pinned_dir(root, RootMissing::ReturnNone)? else {
        return Ok(false);
    };
    delete_pinned_file_in_dir(&dir_fd, leaf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copying_moving_and_deleting_refuse_to_replace() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        fs::create_dir_all(d.join("a/b")).unwrap();
        fs::write(d.join("a/b/f"), "text").unwrap();
        std::os::unix::fs::symlink("b/f", d.join("a/link")).unwrap();
        copy(&d.join("a"), &d.join("c")).unwrap();
        assert_eq!(fs::read_to_string(d.join("c/b/f")).unwrap(), "text");
        assert_eq!(fs::read_link(d.join("c/link")).unwrap(), Path::new("b/f"));
        assert!(copy(&d.join("a"), &d.join("c")).is_err());
        assert!(rename(&d.join("a"), &d.join("c")).is_err());
        assert!(delete(&d.join("c"), false).is_err(), "not empty");
        delete(&d.join("c"), true).unwrap();
        rename(&d.join("a"), &d.join("c")).unwrap();
        assert!(d.join("c/b/f").exists() && !d.join("a").exists());
        let link = attributes(&d.join("c/link")).unwrap().unwrap();
        assert!(matches!(&link, Data::List(l) if l[0] == Data::Symbol("symlink".into()) && l[4] == Data::String("b/f".into())));
        assert_eq!(attributes(&d.join("nothing")).unwrap(), None);
    }

    #[test]
    fn a_directory_is_not_copied_into_itself() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        fs::create_dir_all(d.join("a/b")).unwrap();
        assert!(copy(&d.join("a"), &d.join("a/b/c")).is_err());
        assert!(!d.join("a/b/c").exists());
        std::os::unix::fs::symlink(d.join("a"), d.join("alias")).unwrap();
        assert!(copy(&d.join("a"), &d.join("alias/c")).is_err(), "nor through a link to it");
    }

    #[test]
    fn a_fifo_is_neither_read_nor_copied() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("d")).unwrap();
        let f = dir.path().join("d/fifo");
        rustix::fs::mknodat(rustix::fs::CWD, &f, rustix::fs::FileType::Fifo, rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR, 0).unwrap();
        assert!(head(&f, 10).is_err());
        assert!(copy(&dir.path().join("d"), &dir.path().join("e")).is_err());
        assert!(!dir.path().join("e").exists(), "the partial copy is taken back");
    }

    #[test]
    fn a_head_ends_on_a_character() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("f");
        fs::write(&f, "aé").unwrap();
        assert_eq!(head(&f, 2).unwrap(), "a");
        assert_eq!(head(&f, 3).unwrap(), "aé");
    }

    #[test]
    fn pinned_nofollow_ops_create_read_write_delete_and_refuse_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let target = root.join("real-target");
        fs::create_dir(&target).unwrap();
        let alias = root.join("alias");
        std::os::unix::fs::symlink(&target, &alias).unwrap();

        // Symlinked ancestor refuses write and creates NO subdirectory in target.
        assert!(write_confined_file_private_nofollow(&alias.join("sub"), Path::new("entry"), b"hello", true).is_err());
        assert!(!target.join("sub").exists(), "rejected alias must not create directories in target");

        // Real path creates nested dirs 0o700, writes 0o600, reads back, and deletes cleanly.
        let nested = root.join("store/allow");
        write_confined_file_private_nofollow(&nested, Path::new("rec"), b"payload\n", true).unwrap();
        assert_eq!(fs::metadata(&nested).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(nested.join("rec")).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(read_confined_bytes_nofollow(&nested, Path::new("rec"), 1024).unwrap(), Some(b"payload\n".to_vec()));
        assert!(delete_confined_file_nofollow(&nested, Path::new("rec")).unwrap());
        assert_eq!(read_confined_bytes_nofollow(&nested, Path::new("rec"), 1024).unwrap(), None);
        assert!(!delete_confined_file_nofollow(&nested, Path::new("rec")).unwrap());

        // Leaf symlink is refused for read, write, and delete.
        let outside = root.join("outside.txt");
        fs::write(&outside, b"keep").unwrap();
        std::os::unix::fs::symlink(&outside, nested.join("link")).unwrap();
        assert!(read_confined_bytes_nofollow(&nested, Path::new("link"), 1024).is_err());
        assert!(write_confined_file_private_nofollow(&nested, Path::new("link"), b"overwrite", false).is_err());
        assert!(delete_confined_file_nofollow(&nested, Path::new("link")).is_err());
        assert_eq!(fs::read(&outside).unwrap(), b"keep");
    }

    #[test]
    fn pinned_dir_stays_anchored_across_rename_sync_and_rollback() {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let store = root.join("store/allow");
        let moved = root.join("store/allow-moved");
        let target = root.join("real-target");
        fs::create_dir(&target).unwrap();
        let alias = root.join("alias");
        std::os::unix::fs::symlink(&target, &alias).unwrap();

        // Refuses symlinks and parent components on open.
        assert!(PinnedDir::open(&alias, false).is_err());
        assert!(PinnedDir::open(&alias.join("sub"), true).is_err());
        assert!(PinnedDir::open(&root.join("real-target/../real-target"), false).is_err());
        assert!(PinnedDir::open(&root.join("missing"), false).unwrap().is_none());

        // Open pinned dir (creating 0o700 tail), publish 0o600 record, then rename and replace
        // the directory pathname before sync and rollback.
        let mut handle = PinnedDir::open(&store, true).unwrap().unwrap();
        assert_eq!(fs::metadata(&store).unwrap().permissions().mode() & 0o777, 0o700);
        handle.publish_private(Path::new("rec"), b"allow\n").unwrap();
        assert_eq!(fs::metadata(store.join("rec")).unwrap().permissions().mode() & 0o777, 0o600);

        fs::rename(&store, &moved).unwrap();
        fs::create_dir(&store).unwrap();
        fs::write(store.join("rec"), b"decoy\n").unwrap();

        // Sync and rollback operate on the original directory inode (now at `moved`),
        // leaving the replacement directory untouched.
        handle.sync().unwrap();
        assert_eq!(handle.read_bytes(Path::new("rec"), 1024).unwrap(), Some(b"allow\n".to_vec()));
        assert!(handle.delete(Path::new("rec")).unwrap());
        assert!(!moved.join("rec").exists(), "rollback removed published record from original renamed dir");
        assert_eq!(fs::read(store.join("rec")).unwrap(), b"decoy\n", "replacement dir untouched");

        // Unlink + restore also stays anchored to the original directory inode.
        handle.write_private(Path::new("deny"), b"old-deny\n").unwrap();
        assert!(moved.join("deny").exists());
        assert!(handle.unlink(Path::new("deny")).unwrap());
        assert!(!moved.join("deny").exists());
        handle.write_private(Path::new("deny"), b"old-deny\n").unwrap();
        assert_eq!(fs::read(moved.join("deny")).unwrap(), b"old-deny\n");

        // Leaf symlinks refused and closed handle fails closed.
        let outside = root.join("outside");
        fs::write(&outside, b"keep").unwrap();
        std::os::unix::fs::symlink(&outside, moved.join("link")).unwrap();
        assert!(handle.publish_private(Path::new("link"), b"bad").is_err());
        assert!(handle.unlink(Path::new("link")).is_err());
        assert_eq!(fs::read(&outside).unwrap(), b"keep");

        // Advisory exclusive flock on the pinned directory serializes separate handles
        // in the same process and releases automatically on close.
        assert!(handle.try_lock().unwrap());
        let handle2 = PinnedDir::open(&moved, false).unwrap().unwrap();
        assert!(!handle2.try_lock().unwrap());

        handle.close();
        assert!(handle.sync().is_err());
        assert!(handle.try_lock().is_err());
        assert!(handle.read_bytes(Path::new("deny"), 1024).is_err());
        assert!(handle2.try_lock().unwrap());
    }
}
