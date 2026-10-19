//! A worker process: deployments served by a [`Host`] of its own, for its
//! supervisor ([`crate::supervisor`]), over the socket it was started with
//! (`wire`), sandboxed (`techne_sandbox`) before it reads anything of its
//! tenant's.
//!
//! It waits for a byte from its supervisor (who puts it in its cgroup
//! first), enters its sandbox, reading Techne's Lisp and writing only its
//! tmpfs, says it is ready, and then serves: a deployment's files, sent
//! to it, are written to a directory of their own in its tmpfs, each
//! path checked to stay in it, and loaded from there; requests are
//! answered as the host answers them; failures and output are reported
//! to the supervisor. A Rust native that never looks for stops, holding
//! the worker's thread past a kill, ends the worker (`STUCK`); the end of
//! its socket, its supervisor gone, ends it too.

use std::{
    collections::HashMap,
    io::Read,
    os::fd::{FromRawFd, RawFd},
    path::{Component, Path, PathBuf},
    process::ExitCode,
    rc::Rc,
    time::Duration,
};

use tokio::{net::UnixStream, sync::mpsc, task::AbortHandle};

use crate::{
    Deployment, Host, Limits,
    wire::{BUNDLE_BYTES, FromWorker, ToWorker, read_frame, write_frame},
};

/// The exit status of a worker whose thread a native held past a kill.
pub const STUCK: u8 = 70;

/// The exit status of a worker that could not enter its sandbox here.
pub const UNSANDBOXED: u8 = 77;

/// The most files of a deployment.
const BUNDLE_FILES: usize = 10_000;

/// Serve on the socket at descriptor `fd`, sandboxed unless `sandbox` is
/// false (a dev machine without user namespaces, as its supervisor
/// decides).
pub fn main(fd: RawFd, sandbox: bool) -> ExitCode {
    techne_vm::prepare();
    // SAFETY: the descriptor the supervisor gave, this process's alone.
    let mut socket = unsafe { std::os::unix::net::UnixStream::from_raw_fd(fd) };
    let mut gate = [0];
    if socket.read_exact(&mut gate).is_err() {
        return ExitCode::FAILURE;
    }
    if sandbox {
        let policy = techne_sandbox::Policy {
            read: vec![techne_vm::library::lisp_dir().to_path_buf()],
            scratch: std::env::temp_dir(),
            scratch_bytes: BUNDLE_BYTES * 2,
            keep: vec![fd],
        };
        if let Err(e) = techne_sandbox::enter(&policy) {
            eprintln!("techne worker: no sandbox: {e}");
            return ExitCode::from(UNSANDBOXED);
        }
    }
    techne_vm::stop::on_undelivered_stop(|| {
        eprintln!("techne worker: a native ran on past a kill: ending");
        // SAFETY: ending the process from the watchdog's thread, nothing
        // of it to run.
        unsafe { libc::_exit(STUCK.into()) }
    });
    let serve = || -> std::io::Result<()> {
        socket.set_nonblocking(true)?;
        // No signal handling, which would make a socket of its own.
        let rt = tokio::runtime::Builder::new_current_thread().enable_io().enable_time().build()?;
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, async { serve(UnixStream::from_std(socket)?).await })
    };
    match serve() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("techne worker: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn serve(socket: UnixStream) -> std::io::Result<()> {
    let (mut read, mut write) = socket.into_split();
    let (tx, mut rx) = mpsc::unbounded_channel::<FromWorker>();
    tokio::task::spawn_local(async move {
        while let Some(m) = rx.recv().await {
            if write_frame(&mut write, &m.to_data()).await.is_err() {
                break;
            }
        }
    });
    let host = Host::new();
    let report = tx.clone();
    host.on_failure(move |f| {
        let _ = report.send(FromWorker::Failure {
            deployment: f.deployment.clone(),
            request: f.request,
            status: f.status,
            message: f.message.clone(),
        });
    });
    let report = tx.clone();
    host.on_output(move |p| {
        let _ = report.send(FromWorker::Output {
            deployment: p.deployment.clone(),
            request: p.request,
            text: p.output.text.clone(),
            truncated: p.output.truncated,
        });
    });
    let _ = tx.send(FromWorker::Ready);
    let running = Rc::new(std::cell::RefCell::new(HashMap::<u64, AbortHandle>::new()));
    let driver = host.clone();
    let host_task = tokio::task::spawn_local(async move { driver.run(None).await });
    while let Some(d) = read_frame(&mut read).await? {
        match ToWorker::from_data(d)? {
            ToWorker::Deploy { id, name, entry, files, wall_ms } => {
                let (host, tx) = (host.clone(), tx.clone());
                tokio::task::spawn_local(async move {
                    let limits = Limits { wall: Duration::from_millis(wall_ms), ..Limits::default() };
                    let loaded = match unpack(id, &entry, &files) {
                        Ok(entry) => Deployment::load_async(&entry, limits).await,
                        Err(e) => Err(techne_vm::vm::Error::new(e)),
                    };
                    let message = match loaded {
                        Ok(d) => {
                            let output = d.output().text.clone();
                            host.publish(&name, d);
                            FromWorker::Deployed { id, error: None, output }
                        }
                        Err(e) => FromWorker::Deployed { id, error: Some(e.msg.clone()), output: String::new() },
                    };
                    let _ = tx.send(message);
                });
            }
            ToWorker::Request { rid, name, request } => {
                let reply = host.fetch(&name, request);
                let (tx, running_) = (tx.clone(), running.clone());
                let task = tokio::task::spawn_local(async move {
                    if let Ok(response) = reply.await {
                        let _ = tx.send(FromWorker::Response { rid, response });
                    }
                    running_.borrow_mut().remove(&rid);
                });
                running.borrow_mut().insert(rid, task.abort_handle());
            }
            // Its receiver dropped: the host abandons the request.
            ToWorker::Cancel { rid } => {
                if let Some(task) = running.borrow_mut().remove(&rid) {
                    task.abort();
                }
            }
        }
    }
    host_task.abort();
    Ok(())
}

/// Write deployment `id`'s files to a directory of their own in the
/// tmpfs, each path relative and staying in it, each written once, a
/// bounded number and size: the entry's path there.
fn unpack(id: u64, entry: &str, files: &[(String, Vec<u8>)]) -> Result<PathBuf, String> {
    use std::io::Write;
    if files.len() > BUNDLE_FILES || files.iter().map(|(_, b)| b.len()).sum::<usize>() > BUNDLE_BYTES {
        return Err("the deployment has too many files, or too large".into());
    }
    let root = std::env::temp_dir().join("bundles").join(id.to_string());
    if root.exists() {
        return Err(format!("deployment {id} was sent before"));
    }
    let io = |e: std::io::Error| e.to_string();
    std::fs::create_dir_all(&root).map_err(io)?;
    for (path, bytes) in files {
        let dest = root.join(relative(path)?);
        if let Some(dir) = dest.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&dest).map_err(|e| format!("{path}: {e}"))?;
        f.write_all(bytes).map_err(io)?;
    }
    let entry = root.join(relative(entry)?);
    if !entry.is_file() {
        return Err(format!("{}: no such file in the deployment", entry.display()));
    }
    Ok(entry)
}

/// `path` if it is relative and names nothing outside where it is taken
/// from: no root, no `..`, no `.`.
fn relative(path: &str) -> Result<&Path, String> {
    let p = Path::new(path);
    match !path.is_empty() && p.components().all(|c| matches!(c, Component::Normal(_))) {
        true => Ok(p),
        false => Err(format!("{path}: not a path in the deployment")),
    }
}
