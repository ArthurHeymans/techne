//! A sandboxed process reaches only what its policy gives it, and its
//! cgroup bounds it. Where the kernel does not let a process sandbox
//! itself (unprivileged user namespaces disabled), or there is no cgroup
//! delegated to test with, a test says so and passes.

use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const PROBE: &str = env!("CARGO_BIN_EXE_techne-sandbox-probe");

/// Run the probe trying `tries`, reading `read`, its tmpfs on `scratch`:
/// its exit status and its lines, or `None` where it cannot sandbox.
fn probe(read: &Path, scratch: &Path, tries: &[String]) -> Option<(std::process::ExitStatus, String)> {
    let out = Command::new(PROBE).arg("--read").arg(read).arg("--scratch").arg(scratch).args(tries).output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    if out.status.code() == Some(77) {
        eprintln!("skipped: {err}");
        return None;
    }
    Some((out.status, err))
}

#[test]
fn a_sandboxed_process_reaches_only_what_it_is_given() {
    let dir = tempfile::tempdir().unwrap();
    let (read, scratch) = (dir.path().join("read"), dir.path().join("scratch"));
    std::fs::create_dir_all(&read).unwrap();
    std::fs::create_dir_all(&scratch).unwrap();
    std::fs::write(read.join("f"), "hi").unwrap();
    let outside = dir.path().join("outside");
    std::fs::write(&outside, "secret").unwrap();
    let path = |p: &Path| p.display().to_string();
    let tries = [
        format!("read={}", path(&read.join("f"))),
        format!("read={}", path(&outside)),
        "read=/etc/hostname".into(),
        format!("list={}", path(dir.path())),
        format!("write={}", path(&scratch.join("x"))),
        format!("read={}", path(&scratch.join("x"))),
        format!("write={}", path(&read.join("g"))),
        "pid".into(),
        "processes".into(),
        "socket".into(),
        "unix-socket=/run/dbus/system_bus_socket".into(),
        "exec=/bin/sh".into(),
        "fork".into(),
        "thread".into(),
        "fd=0".into(),
        "fd=1".into(),
    ];
    let Some((status, out)) = probe(&read, &scratch, &tries) else { return };
    assert!(status.success(), "{status}: {out}");
    let said = |t: &str| out.lines().find_map(|l| l.strip_prefix(&format!("{t}: "))).unwrap_or_else(|| panic!("{t}: not tried: {out}"));
    let ok = |i: usize| assert!(said(&tries[i]).starts_with("ok"), "{}: {out}", tries[i]);
    let denied = |i: usize| assert!(said(&tries[i]).starts_with("denied"), "{}: {out}", tries[i]);
    ok(0);
    denied(1);
    denied(2);
    denied(3);
    ok(4);
    ok(5);
    denied(6);
    assert_eq!(said("pid"), "ok 1", "PID 1 of its own namespace");
    assert_eq!(said("processes"), "ok 1", "no process of the host's");
    denied(9);
    denied(10);
    denied(11);
    denied(12);
    ok(13);
    denied(14);
    denied(15);
}

/// What starting a sandboxed process costs, exec included.
#[test]
fn a_sandboxed_process_starts_in_milliseconds() {
    let dir = tempfile::tempdir().unwrap();
    let runs = 20;
    let started = Instant::now();
    for _ in 0..runs {
        if probe(dir.path(), dir.path(), &[]).is_none() {
            return;
        }
    }
    let each = started.elapsed() / runs;
    eprintln!("exec and sandbox: {each:?} a process");
    assert!(each < Duration::from_millis(200), "{each:?}");
}

/// A worker past its cgroup's memory is killed, alone. Run in a scope of
/// its own (`systemd-run --user --scope`), its cgroup delegated, where
/// there is one.
#[test]
fn a_worker_cgroup_bounds_its_memory() {
    const CHILD: &str = "TECHNE_SANDBOX_CGROUP_TEST";
    if std::env::var_os(CHILD).is_some() {
        let cgroups = match techne_sandbox::cgroup::Cgroups::adopt() {
            Ok(c) => c,
            Err(e) => {
                println!("skipped: {e}");
                return;
            }
        };
        let limits = techne_sandbox::cgroup::Limits { memory: 32 << 20, pids: 64, cpu: Some((50_000, 100_000)) };
        let worker = cgroups.worker("w1", &limits).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut child = Command::new(PROBE)
            .arg("--gate")
            .arg("--read")
            .arg(dir.path())
            .arg("--scratch")
            .arg(dir.path())
            .arg("alloc=256")
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        worker.add(child.id()).unwrap();
        child.stdin.take().unwrap().write_all(b"g").unwrap();
        let out = child.wait_with_output().unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        if out.status.code() == Some(77) {
            println!("skipped: {err}");
            return;
        }
        assert!(!out.status.success() && !err.contains("alloc=256: ok"), "{:?}: {err}", out.status);
        assert!(worker.oom_kills() > 0, "killed for its memory");
        println!("contained");
        return;
    }
    let ran = Command::new("systemd-run")
        .args(["--user", "--scope", "--quiet", "-p", "Delegate=yes", "--"])
        .arg(std::env::current_exe().unwrap())
        .args(["--exact", "a_worker_cgroup_bounds_its_memory", "--nocapture"])
        .env(CHILD, "1")
        .output();
    let Ok(out) = ran else {
        eprintln!("skipped: no systemd-run");
        return;
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() && !stdout.contains("test result") {
        eprintln!("skipped: systemd-run: {stderr}");
        return;
    }
    assert!(out.status.success(), "{stdout}\n{stderr}");
    eprintln!("{}", stdout.lines().find(|l| l.starts_with("skipped") || l.starts_with("contained")).unwrap_or("?"));
}
