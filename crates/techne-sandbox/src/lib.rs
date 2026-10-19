//! A worker process sandboxing itself, before it reads any tenant code
//! (doc/serverless.md, "Workers"), and the cgroups its supervisor puts
//! workers in (`cgroup`).
//!
//! [`enter`] makes the process, which must be single-threaded, its own
//! world: new user, mount, network, IPC, UTS, PID and cgroup namespaces
//! (no network but loopback, which nothing listens on; no host processes;
//! no other user), with the process continuing as PID 1 of its PID
//! namespace in a child (the outer process waits, and exits as it does;
//! the child dies with it), a fresh `/proc` of that namespace, and a
//! private tmpfs where it may write. Then no new privileges; Landlock,
//! which lets it read only what its [`Policy`] names (and its `/proc`) and
//! write only its tmpfs; and a seccomp filter allowing the system calls a
//! VM needs (memory, threads, futexes, clocks, its event loop, reading and
//! writing descriptors it has, the files Landlock allows) and refusing
//! every other with `EPERM`: no sockets of its own, no `execve`, no
//! `ptrace`, no mounts, no new processes but its own threads, and no
//! capabilities its user namespace gave it. What it may talk to is the
//! descriptors it keeps, a socket to its supervisor: every other is
//! closed (standard input too; standard error stays, for the runtime's
//! diagnostics).
//!
//! Call it first, before the process has threads or has read anything of
//! a tenant's, and after its supervisor has put it in its cgroup: what it
//! forks is in the cgroup it is in then.
//!
//! Where the kernel refuses a step (unprivileged user namespaces disabled,
//! as Ubuntu's AppArmor does; Landlock missing), `enter` fails before it
//! has changed anything a later step relies on, and says which.

use std::{io, path::PathBuf};

pub mod cgroup;
#[cfg(target_os = "linux")]
mod linux;

/// What a sandboxed process may reach of the filesystem.
#[derive(Clone, Debug)]
pub struct Policy {
    /// Directories it may read, with all under them.
    pub read: Vec<PathBuf>,
    /// An existing directory on which a tmpfs of its own is mounted, of
    /// `scratch_bytes` at most: the only place it may write.
    pub scratch: PathBuf,
    pub scratch_bytes: usize,
    /// Descriptors it keeps (beside standard error): every other is
    /// closed, for what was open stays open to it whatever else holds.
    pub keep: Vec<i32>,
}

/// Sandbox this process, as the module says. It must be single-threaded.
/// It returns in the sandbox; the process outside never returns.
#[cfg(target_os = "linux")]
pub fn enter(policy: &Policy) -> io::Result<()> {
    linux::enter(policy)
}

#[cfg(not(target_os = "linux"))]
pub fn enter(_: &Policy) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "sandboxing needs Linux"))
}
