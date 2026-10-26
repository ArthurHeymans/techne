//! Files as a directory editor sees them (`lisp/editor/packages/dired.scm`):
//! what an entry is, and moving, copying and deleting entries. Nothing is
//! replaced: an operation whose destination exists is refused, so that
//! asking before overwriting stays the Lisp's to do.

use std::{
    fs,
    io::{self, Read},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
    time::UNIX_EPOCH,
};

use techne_vm::data::Data;

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

/// Move `from` to `to`, which must not exist: never replaced, even when it
/// appears meanwhile. Across file systems it is copied, then deleted once
/// the copy is whole.
pub fn rename(from: &Path, to: &Path) -> io::Result<()> {
    use rustix::{
        fs::{CWD, RenameFlags, renameat_with},
        io::Errno,
    };
    match renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        Err(Errno::EXIST) => Err(already_exists()),
        Err(Errno::XDEV) => {
            copy(from, to)?;
            delete(from, true)
        }
        // A file system without RENAME_NOREPLACE: checked, then renamed.
        Err(Errno::INVAL | Errno::NOSYS | Errno::OPNOTSUPP) => {
            if exists(to)? {
                return Err(already_exists());
            }
            fs::rename(from, to)
        }
        Err(e) => Err(e.into()),
    }
}

/// Copy `from` to `to`, which must not exist: a directory with all it
/// holds, a symbolic link as a link. Nothing is replaced: each file and
/// directory is created anew. A directory is not copied into itself; a
/// copy that fails part way is taken back.
pub fn copy(from: &Path, to: &Path) -> io::Result<()> {
    if fs::symlink_metadata(from)?.is_dir() {
        let source = fs::canonicalize(from)?;
        if techne_text::document::file_path(to)?.starts_with(&source) {
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
}
