//! `enter` on Linux: the steps, in the order each needs the one before.

use std::{collections::BTreeMap, ffi::CString, io, os::unix::ffi::OsStrExt, path::Path};

use landlock::{
    ABI, Access, AccessFs, CompatLevel, Compatible, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr, RulesetStatus,
};
use seccompiler::{BpfProgram, SeccompAction, SeccompFilter, TargetArch};

use crate::Policy;

pub(crate) fn enter(policy: &Policy) -> io::Result<()> {
    // A user namespace is a process's only if it has one thread.
    if threads()? != 1 {
        return Err(io::Error::other("a process enters a sandbox while it has one thread"));
    }
    // Gone with its supervisor, as the process inside is with it.
    // SAFETY: a system call taking integers.
    unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) };
    close_all_but(&policy.keep)?;
    // SAFETY: system calls taking nothing.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    let flags = libc::CLONE_NEWUSER
        | libc::CLONE_NEWNS
        | libc::CLONE_NEWNET
        | libc::CLONE_NEWIPC
        | libc::CLONE_NEWUTS
        | libc::CLONE_NEWPID
        | libc::CLONE_NEWCGROUP;
    // SAFETY: a system call taking flags.
    check(unsafe { libc::unshare(flags) }, "unshare: user namespaces unavailable")?;
    // Root of its user namespace, as itself outside: what mounting its
    // tmpfs and `/proc` takes, and nothing outside.
    std::fs::write("/proc/self/setgroups", "deny")?;
    std::fs::write("/proc/self/uid_map", format!("0 {uid} 1"))?;
    std::fs::write("/proc/self/gid_map", format!("0 {gid} 1"))?;
    // Its children are in the new PID namespace, not it: it goes on in
    // one, which is PID 1 there.
    // SAFETY: the process has one thread.
    match unsafe { libc::fork() } {
        -1 => return Err(last("fork")),
        0 => {}
        child => {
            // The waiter keeps nothing of the worker's: its socket closes
            // when the worker ends.
            let _ = close_all_but(&[]);
            wait_and_exit(child)
        }
    }
    // SAFETY: a system call taking integers.
    unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) };
    mounts(policy)?;
    drop_capabilities()?;
    // SAFETY: as above.
    check(unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) }, "no_new_privs")?;
    landlock(policy)?;
    seccomp()?;
    Ok(())
}

/// Close every descriptor but standard error and `keep`, standard input
/// and output to `/dev/null`'s place (closed: nothing reads or writes
/// them).
fn close_all_but(keep: &[i32]) -> io::Result<()> {
    let mut keep: Vec<u32> = keep.iter().map(|&fd| fd as u32).chain([2]).collect();
    keep.sort_unstable();
    keep.dedup();
    let mut from = 0u32;
    for &k in &keep {
        if k > from {
            // SAFETY: closing descriptors, which nothing here uses.
            check(unsafe { libc::close_range(from, k - 1, 0) }, "close_range")?;
        }
        from = k + 1;
    }
    // SAFETY: as above.
    check(unsafe { libc::close_range(from, u32::MAX, 0) }, "close_range")
}

/// Give up every capability its user namespace gave it: what it may do
/// is what any process of its user may, within its namespaces.
fn drop_capabilities() -> io::Result<()> {
    for cap in 0..=63 {
        // SAFETY: a system call taking integers; past the last capability
        // it fails, which is fine.
        unsafe { libc::prctl(libc::PR_CAPBSET_DROP, cap, 0, 0, 0) };
    }
    // SAFETY: as above.
    check(unsafe { libc::prctl(libc::PR_CAP_AMBIENT, libc::PR_CAP_AMBIENT_CLEAR_ALL, 0, 0, 0) }, "ambient capabilities")?;
    #[repr(C)]
    struct Header {
        version: u32,
        pid: i32,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Set {
        effective: u32,
        permitted: u32,
        inheritable: u32,
    }
    const VERSION_3: u32 = 0x2008_0522;
    let header = Header { version: VERSION_3, pid: 0 };
    let none = [Set { effective: 0, permitted: 0, inheritable: 0 }; 2];
    // SAFETY: capset(2) with a version 3 header and its two sets.
    check(unsafe { libc::syscall(libc::SYS_capset, &header, none.as_ptr()) } as libc::c_int, "capset")
}

/// The threads of this process.
fn threads() -> io::Result<usize> {
    let status = std::fs::read_to_string("/proc/self/status")?;
    let line = status.lines().find_map(|l| l.strip_prefix("Threads:")).ok_or_else(|| io::Error::other("no thread count"))?;
    line.trim().parse().map_err(io::Error::other)
}

/// The process outside: wait for the one inside, and end as it does.
fn wait_and_exit(child: libc::pid_t) -> ! {
    let mut status = 0;
    loop {
        // SAFETY: `status` is a live integer.
        let r = unsafe { libc::waitpid(child, &mut status, 0) };
        if r == child {
            break;
        }
        if r == -1 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            std::process::exit(1);
        }
    }
    if libc::WIFEXITED(status) {
        std::process::exit(libc::WEXITSTATUS(status));
    }
    // Killed: so is this, by the same signal (the default action, as
    // nothing here handles one).
    let signal = libc::WTERMSIG(status);
    // SAFETY: signalling itself.
    unsafe { libc::kill(libc::getpid(), signal) };
    std::process::exit(128 + signal);
}

/// Its mounts its own; `/proc` of its PID namespace; its tmpfs.
fn mounts(policy: &Policy) -> io::Result<()> {
    mount(None, Path::new("/"), None, libc::MS_REC | libc::MS_PRIVATE, None)?;
    let hidden = libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC;
    mount(Some("proc"), Path::new("/proc"), Some("proc"), hidden, None)?;
    let options = format!("size={},nr_inodes=4096,mode=0700", policy.scratch_bytes);
    mount(Some("tmpfs"), &policy.scratch, Some("tmpfs"), hidden, Some(&options))
}

fn mount(source: Option<&str>, target: &Path, kind: Option<&str>, flags: libc::c_ulong, data: Option<&str>) -> io::Result<()> {
    let c = |s: &str| CString::new(s).map_err(io::Error::other);
    let (source, kind, data) = (source.map(c).transpose()?, kind.map(c).transpose()?, data.map(c).transpose()?);
    let target = CString::new(target.as_os_str().as_bytes()).map_err(io::Error::other)?;
    let ptr = |s: &Option<CString>| s.as_ref().map_or(std::ptr::null(), |s| s.as_ptr());
    // SAFETY: NUL-terminated strings, or null, as mount(2) takes them.
    let r = unsafe { libc::mount(ptr(&source), target.as_ptr(), ptr(&kind), flags, ptr(&data).cast()) };
    check(r, &format!("mount {}", target.to_string_lossy()))
}

/// Read what the policy names and its `/proc`; write its tmpfs; no more.
fn landlock(policy: &Policy) -> io::Result<()> {
    let abi = ABI::V3;
    let read = AccessFs::ReadFile | AccessFs::ReadDir;
    let fail = |e: landlock::RulesetError| io::Error::other(format!("landlock: {e}"));
    let mut ruleset = Ruleset::default()
        .set_compatibility(CompatLevel::HardRequirement)
        .handle_access(AccessFs::from_all(abi))
        .map_err(fail)?
        .create()
        .map_err(fail)?;
    for dir in policy.read.iter().map(|p| p.as_path()).chain([Path::new("/proc")]) {
        let fd = PathFd::new(dir).map_err(|e| io::Error::other(format!("landlock: {}: {e}", dir.display())))?;
        ruleset = ruleset.add_rule(PathBeneath::new(fd, read)).map_err(fail)?;
    }
    let fd = PathFd::new(&policy.scratch).map_err(|e| io::Error::other(format!("landlock: {e}")))?;
    ruleset = ruleset.add_rule(PathBeneath::new(fd, AccessFs::from_all(abi) & !AccessFs::Execute)).map_err(fail)?;
    let status = ruleset.restrict_self().map_err(fail)?;
    match status.ruleset {
        RulesetStatus::FullyEnforced => Ok(()),
        other => Err(io::Error::other(format!("landlock: not enforced fully ({other:?})"))),
    }
}

/// The system calls a VM needs, on descriptors it has; every other
/// fails with `EPERM`. `clone` only makes threads (its flags have
/// `CLONE_THREAD`); `clone3`, whose flags a filter cannot see, fails with
/// `ENOSYS`, which has the C library use `clone`.
fn seccomp() -> io::Result<()> {
    use seccompiler::{SeccompCmpArgLen as Len, SeccompCmpOp as Op, SeccompCondition as Cond, SeccompRule};
    #[cfg(target_arch = "x86_64")]
    let arch = TargetArch::x86_64;
    #[cfg(target_arch = "aarch64")]
    let arch = TargetArch::aarch64;
    let fail = |e: &dyn std::fmt::Display| io::Error::other(format!("seccomp: {e}"));
    let rule = |conditions: Vec<Cond>| SeccompRule::new(conditions).map_err(|e| fail(&e));
    let arg = |i: u8, op: Op, value: u64| Cond::new(i, Len::Qword, op, value).map_err(|e| fail(&e));
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = ALLOWED.iter().map(|&nr| (nr, Vec::new())).collect();
    let thread = libc::CLONE_THREAD as u64;
    rules.insert(libc::SYS_clone, vec![rule(vec![arg(0, Op::MaskedEq(thread), thread)?])?]);
    // Non-blocking and close-on-exec, as Rust and its event loop set them
    // (the requests' type varies by target).
    #[allow(clippy::unnecessary_cast)]
    rules.insert(
        libc::SYS_ioctl,
        [libc::FIONBIO, libc::FIOCLEX].iter().map(|&r| rule(vec![arg(1, Op::Eq, r as u64)?])).collect::<io::Result<_>>()?,
    );
    // A pair of sockets joined to each other reaches nothing else (an
    // event loop's wake-up: tokio's signal driver).
    rules.insert(libc::SYS_socketpair, vec![rule(vec![arg(0, Op::Eq, libc::AF_UNIX as u64)?])?]);
    let program = |rules, mismatch, matched| -> io::Result<BpfProgram> {
        let filter = SeccompFilter::new(rules, mismatch, matched, arch).map_err(|e| fail(&e))?;
        filter.try_into().map_err(|e: seccompiler::BackendError| fail(&e))
    };
    let allowed = program(rules, SeccompAction::Errno(libc::EPERM as u32), SeccompAction::Allow)?;
    let clone3 =
        program(BTreeMap::from([(libc::SYS_clone3, Vec::new())]), SeccompAction::Allow, SeccompAction::Errno(libc::ENOSYS as u32))?;
    // The one installed last decides the error of a call both refuse;
    // by `prctl`, as the first refuses `seccomp`.
    seccompiler::apply_filter(&allowed).map_err(|e| fail(&e))?;
    let prog = libc::sock_fprog { len: clone3.len() as u16, filter: clone3.as_ptr() as *mut libc::sock_filter };
    // SAFETY: a filter program that lives through the call.
    check(unsafe { libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &prog) }, "seccomp")
}

/// What `seccomp` allows (and, by their arguments, `clone`, `ioctl` and
/// `socketpair`). Not here, so refused: `socket`, `connect`,
/// `bind`, `listen`, `accept`, `execve`, `ptrace`, `mount`, `unshare`,
/// `setns`, `fork`, `bpf`, `perf_event_open`, `io_uring_*`, `process_vm_*`,
/// `userfaultfd`, `kill` (but of its own threads, `tgkill`), `keyctl`...
const ALLOWED: &[i64] = &[
    // Descriptors it has.
    libc::SYS_read,
    libc::SYS_write,
    libc::SYS_readv,
    libc::SYS_writev,
    libc::SYS_pread64,
    libc::SYS_pwrite64,
    libc::SYS_close,
    libc::SYS_fstat,
    libc::SYS_lseek,
    libc::SYS_fcntl,
    libc::SYS_dup,
    libc::SYS_dup3,
    libc::SYS_pipe2,
    libc::SYS_recvfrom,
    libc::SYS_sendto,
    libc::SYS_recvmsg,
    libc::SYS_sendmsg,
    libc::SYS_shutdown,
    libc::SYS_getsockopt,
    libc::SYS_setsockopt,
    libc::SYS_fsync,
    libc::SYS_fdatasync,
    libc::SYS_ftruncate,
    // Files Landlock lets it reach.
    libc::SYS_openat,
    libc::SYS_newfstatat,
    libc::SYS_statx,
    libc::SYS_getdents64,
    libc::SYS_readlinkat,
    libc::SYS_faccessat,
    libc::SYS_faccessat2,
    libc::SYS_mkdirat,
    libc::SYS_unlinkat,
    libc::SYS_renameat2,
    libc::SYS_getcwd,
    libc::SYS_statfs,
    libc::SYS_fstatfs,
    // Memory.
    libc::SYS_mmap,
    libc::SYS_munmap,
    libc::SYS_mprotect,
    libc::SYS_mremap,
    libc::SYS_madvise,
    libc::SYS_brk,
    libc::SYS_membarrier,
    // Threads and their synchronization.
    libc::SYS_futex,
    libc::SYS_set_robust_list,
    libc::SYS_get_robust_list,
    libc::SYS_rseq,
    libc::SYS_set_tid_address,
    libc::SYS_exit,
    libc::SYS_exit_group,
    libc::SYS_sched_yield,
    libc::SYS_sched_getaffinity,
    libc::SYS_prctl,
    libc::SYS_gettid,
    libc::SYS_getpid,
    libc::SYS_getppid,
    libc::SYS_getuid,
    libc::SYS_geteuid,
    libc::SYS_getgid,
    libc::SYS_getegid,
    libc::SYS_uname,
    libc::SYS_getrusage,
    // Signals, its own.
    libc::SYS_rt_sigaction,
    libc::SYS_rt_sigprocmask,
    libc::SYS_rt_sigreturn,
    libc::SYS_sigaltstack,
    libc::SYS_tgkill,
    libc::SYS_restart_syscall,
    // Time, entropy.
    libc::SYS_clock_gettime,
    libc::SYS_clock_getres,
    libc::SYS_clock_nanosleep,
    libc::SYS_nanosleep,
    libc::SYS_gettimeofday,
    libc::SYS_getrandom,
    // Its event loop.
    libc::SYS_epoll_create1,
    libc::SYS_epoll_ctl,
    libc::SYS_epoll_pwait,
    libc::SYS_epoll_pwait2,
    libc::SYS_eventfd2,
    libc::SYS_ppoll,
    libc::SYS_pselect6,
    libc::SYS_timerfd_create,
    libc::SYS_timerfd_settime,
    // The older calls x86-64's libraries still make.
    #[cfg(target_arch = "x86_64")]
    libc::SYS_open,
    #[cfg(target_arch = "x86_64")]
    libc::SYS_stat,
    #[cfg(target_arch = "x86_64")]
    libc::SYS_lstat,
    #[cfg(target_arch = "x86_64")]
    libc::SYS_access,
    #[cfg(target_arch = "x86_64")]
    libc::SYS_readlink,
    #[cfg(target_arch = "x86_64")]
    libc::SYS_poll,
    #[cfg(target_arch = "x86_64")]
    libc::SYS_epoll_wait,
    #[cfg(target_arch = "x86_64")]
    libc::SYS_arch_prctl,
    #[cfg(target_arch = "x86_64")]
    libc::SYS_mkdir,
    #[cfg(target_arch = "x86_64")]
    libc::SYS_unlink,
];

fn check(r: libc::c_int, what: &str) -> io::Result<()> {
    if r == -1 { Err(last(what)) } else { Ok(()) }
}

fn last(what: &str) -> io::Error {
    let e = io::Error::last_os_error();
    io::Error::new(e.kind(), format!("{what}: {e}"))
}
