//! What a sandboxed process can do, for the tests (`tests/sandbox.rs`):
//! `techne-sandbox-probe --read DIR --scratch DIR [--keep FD] [--gate] TRY...`
//! enters a sandbox reading DIR, its tmpfs on the scratch DIR, then tries
//! each TRY, printing `TRY: ok` or `TRY: denied (why)`. It exits 77 if the
//! sandbox cannot be entered here (unprivileged user namespaces
//! disabled), saying why. With `--gate` it waits for a byte on standard
//! input before it enters, so that it can be put in a cgroup first.

use std::{io::Write, path::PathBuf};

fn main() {
    let mut read = Vec::new();
    let mut scratch = None;
    let mut keep = Vec::new();
    let mut tries = Vec::new();
    let mut gate = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--read" => read.push(PathBuf::from(args.next().unwrap())),
            "--scratch" => scratch = Some(PathBuf::from(args.next().unwrap())),
            "--keep" => keep.push(args.next().unwrap().parse().unwrap()),
            "--gate" => gate = true,
            _ => tries.push(a),
        }
    }
    if gate {
        let _ = std::io::Read::read(&mut std::io::stdin(), &mut [0]);
    }
    let started = std::time::Instant::now();
    let policy = techne_sandbox::Policy { read, scratch: scratch.expect("--scratch"), scratch_bytes: 1 << 20, keep };
    if let Err(e) = techne_sandbox::enter(&policy) {
        if e.kind() == std::io::ErrorKind::PermissionDenied || e.to_string().contains("unshare") {
            eprintln!("unavailable: {e}");
            std::process::exit(77);
        }
        eprintln!("enter: {e}");
        std::process::exit(1);
    }
    eprintln!("entered in {} us", started.elapsed().as_micros());
    for t in tries {
        let result = attempt(&t);
        let line = match result {
            Ok(what) => format!("{t}: ok{}", what.map(|w| format!(" {w}")).unwrap_or_default()),
            Err(e) => format!("{t}: denied ({e})"),
        };
        // Standard output is closed: what is printed goes to standard error.
        let _ = writeln!(std::io::stderr(), "{line}");
    }
}

/// Try `what`; what it found, if it says.
fn attempt(what: &str) -> std::io::Result<Option<String>> {
    let (op, arg) = what.split_once('=').unwrap_or((what, ""));
    match op {
        "read" => std::fs::read(arg).map(|b| Some(b.len().to_string())),
        "write" => std::fs::write(arg, b"x").map(|()| None),
        "list" => std::fs::read_dir(arg).map(|d| Some(d.count().to_string())),
        "pid" => Ok(Some(std::process::id().to_string())),
        "processes" => {
            let n = std::fs::read_dir("/proc")?.flatten().filter(|e| e.file_name().to_string_lossy().parse::<u32>().is_ok()).count();
            Ok(Some(n.to_string()))
        }
        "socket" => {
            // SAFETY: a system call taking integers.
            let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
            if fd == -1 { Err(std::io::Error::last_os_error()) } else { Ok(None) }
        }
        "unix-socket" => std::os::unix::net::UnixStream::connect(arg).map(|_| None),
        "exec" => std::process::Command::new(arg).status().map(|s| Some(s.to_string())),
        "thread" => std::thread::spawn(|| 1).join().map(|_| None).map_err(|_| std::io::Error::other("panicked")),
        "fork" => {
            // SAFETY: a forked child that exits at once.
            match unsafe { libc::fork() } {
                -1 => Err(std::io::Error::last_os_error()),
                0 => unsafe { libc::_exit(0) },
                _ => Ok(None),
            }
        }
        "fd" => {
            let fd: i32 = arg.parse().unwrap();
            // SAFETY: asking about a descriptor number.
            if unsafe { libc::fcntl(fd, libc::F_GETFD) } == -1 { Err(std::io::Error::last_os_error()) } else { Ok(None) }
        }
        "alloc" => {
            let mb: usize = arg.parse().unwrap();
            let v = vec![1u8; mb << 20];
            Ok(Some(std::hint::black_box(v).iter().map(|&b| b as usize).sum::<usize>().to_string()))
        }
        _ => Err(std::io::Error::other("unknown")),
    }
}
