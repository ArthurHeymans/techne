//! The Lisp as a Hermit unikernel: the REPL on the serial console, or the
//! file the command line names (as `techne run`).
//!
//! The guest has no disk. Techne's Lisp directory and the language suites
//! are built in; the host may hand over more files, its program among them
//! (`run-file.py`). All are unpacked at boot into Hermit's in-memory file
//! system at the paths they have on the host, so that paths mean the same
//! inside and out.

#[cfg(target_os = "hermit")]
use hermit as _;

#[cfg(target_os = "hermit")]
mod fw_cfg;
#[cfg(target_os = "hermit")]
mod mem;

use std::{
    io::{Error, ErrorKind, Result},
    path::{Path, PathBuf},
    process::ExitCode,
};

use include_dir::{Dir, include_dir};

/// The VM's stack. Stacker cannot tell where Hermit's thread stacks end,
/// so every deep recursion (`techne_vm::nested`) would switch to a fresh
/// segment; running on one of its segments, it knows.
const STACK: usize = 64 << 20;

static LISP: Dir = include_dir!("$CARGO_MANIFEST_DIR/../../lisp");
static SUITES: Dir = include_dir!("$CARGO_MANIFEST_DIR/../techne-vm/tests/suites");

/// `create_dir_all`: Hermit's file system fails to make a directory in a
/// missing one with EBADF, which std does not take for "not found".
fn create_dirs(dir: &Path) -> Result<()> {
    let mut ancestors: Vec<_> = dir.ancestors().collect();
    ancestors.reverse();
    ancestors.into_iter().filter(|d| !d.is_dir()).try_for_each(std::fs::create_dir)
}

fn write(path: &Path, contents: &[u8]) -> Result<()> {
    create_dirs(path.parent().unwrap_or(Path::new("/")))?;
    std::fs::write(path, contents)
}

fn unpack(dir: &Dir, to: &Path) -> Result<()> {
    create_dirs(to)?;
    dir.files().try_for_each(|f| std::fs::write(to.join(f.path().file_name().expect("a file name")), f.contents()))?;
    dir.dirs().try_for_each(|d| unpack(d, &to.join(d.path().file_name().expect("a directory name"))))
}

/// The files of an archive from `run-file.py`: each a little-endian u32
/// path length, the path, a u64 size and the contents; or, for a
/// directory, `DIRECTORY` for the size and no contents.
const DIRECTORY: u64 = u64::MAX;

fn unpack_archive(mut archive: &[u8]) -> Result<()> {
    fn take<'a>(at: &mut &'a [u8], n: usize) -> Result<&'a [u8]> {
        let (head, rest) = at.split_at_checked(n).ok_or_else(|| Error::new(ErrorKind::UnexpectedEof, "truncated archive"))?;
        *at = rest;
        Ok(head)
    }
    while !archive.is_empty() {
        let len = u32::from_le_bytes(take(&mut archive, 4)?.try_into().unwrap()) as usize;
        let path = PathBuf::from(String::from_utf8_lossy(take(&mut archive, len)?).into_owned());
        match u64::from_le_bytes(take(&mut archive, 8)?.try_into().unwrap()) {
            DIRECTORY => create_dirs(&path)?,
            size => write(&path, take(&mut archive, size as usize)?)?,
        }
    }
    Ok(())
}

/// What the host handed over.
#[cfg(target_os = "hermit")]
fn host_files() -> Result<()> {
    fw_cfg::file("opt/techne/files").map_or(Ok(()), |a| unpack_archive(&a))
}

#[cfg(not(target_os = "hermit"))]
fn host_files() -> Result<()> {
    Ok(())
}

/// `set_current_dir`, which Hermit's std does not offer though its
/// kernel does.
#[cfg(target_os = "hermit")]
fn chdir(dir: &Path) -> Result<()> {
    unsafe extern "C" {
        fn sys_chdir(path: *mut std::ffi::c_char) -> i32;
    }
    let path = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes()).map_err(Error::other)?;
    // SAFETY: a NUL-terminated path, which the kernel only reads.
    match unsafe { sys_chdir(path.as_ptr().cast_mut()) } {
        0 => Ok(()),
        e => Err(Error::from_raw_os_error(e.abs())),
    }
}

#[cfg(not(target_os = "hermit"))]
fn chdir(dir: &Path) -> Result<()> {
    std::env::set_current_dir(dir)
}

fn main() -> ExitCode {
    techne_vm::alloc::contain_out_of_memory();
    // The repository's path on the host: Hermit resolves no `..`.
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).expect("crates/techne-hermit");
    let lisp = repo.join("lisp");
    let unpacked = unpack(&LISP, &lisp)
        .and_then(|()| unpack(&SUITES, &repo.join("crates/techne-vm/tests/suites")))
        .and_then(|()| host_files());
    if let Err(e) = unpacked {
        eprintln!("cannot unpack the files: {e}");
        return ExitCode::FAILURE;
    }
    // SAFETY: no other thread yet.
    unsafe {
        std::env::set_var("TECHNE_LISP_DIR", &lisp);
        // Programs expect one, as R7RS's own tests do.
        if std::env::var_os("PATH").is_none() {
            std::env::set_var("PATH", "/bin");
        }
        // One CPU: Hermit does not preempt the VM for the JIT's thread,
        // so compile in line.
        if std::thread::available_parallelism().is_ok_and(|n| n.get() == 1) {
            std::env::set_var("TECHNE_JIT_SYNC", "1");
        }
    }
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(at) = args.iter().position(|a| a == "--cwd") {
        let dir = args.drain(at..at + 2).nth(1).unwrap_or_default();
        if let Err(e) = create_dirs(Path::new(&dir)).and_then(|()| chdir(Path::new(&dir))) {
            eprintln!("cannot enter {dir}: {e}");
        }
    }
    stacker::grow(STACK, || techne_vm::cli::main(args))
}
