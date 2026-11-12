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
//!
//! For development ([`Dev`]), a deployment is published to a dev world
//! with a REPL server, whose clients the supervisor relays: each a
//! connection of its own, as bytes both ways, over an in-memory pipe here.
//! A connection that falls behind what is sent to it is closed.

use std::{
    collections::HashMap,
    io::Read,
    os::fd::{FromRawFd, RawFd},
    path::{Component, Path, PathBuf},
    process::ExitCode,
    rc::Rc,
    time::Duration,
};

use techne_vm::data::Data;
use tokio::{
    net::UnixStream,
    sync::{Semaphore, mpsc, oneshot},
    task::AbortHandle,
};

use crate::{
    Deployment, Host, Limits,
    body::{self, Piece},
    wire::{BUNDLE_BYTES, FromWorker, REPL_CHUNK, ToWorker, WINDOW, read_frame, write_frame},
};

/// A request being served: the task answering it, the queue its body
/// arrives on while it streams, and the credits for its response's body.
struct Serving {
    task: AbortHandle,
    body: Option<mpsc::Sender<Piece>>,
    credits: Rc<Semaphore>,
}

/// The exit status of a worker whose thread a native held past a kill.
pub const STUCK: u8 = 70;

/// The exit status of a worker that could not enter its sandbox here.
pub const UNSANDBOXED: u8 = 77;

/// What a worker does for development: the application's (`techne cloud
/// dev`, with its nREPL server), as this crate knows no REPL.
pub trait Dev {
    /// Publish `deployment`, whose entry module is at `entry`, as `name`
    /// on `host` with its REPL attached: for development
    /// (`Host::publish_dev`), or served as in production with a debug
    /// instance (`Host::attach_debug`).
    fn publish(&mut self, host: &Host, name: &str, entry: &Path, deployment: Deployment) -> Result<(), techne_vm::vm::Error>;
    /// The deployment published as `name` is the one its requests go to
    /// from now on: its world is the REPL's.
    fn activate(&mut self, _host: &Host, _name: &str) {}
    /// The deployment published as `name` is served no more.
    fn retire(&mut self, _name: &str) {}
    /// A deployment served as in production was published as `name`, its
    /// entry module at `entry`: one a REPL may be attached to.
    fn loaded(&mut self, _name: &str, _entry: &Path) {}
    /// Serve REPL client `cid` on `stream`, authenticated as `principal`
    /// if as anyone, in the debug instance of the deployment published as
    /// `name` on `host` (`Host::attach_debug`), attached for it if it has
    /// none: an error if it cannot be.
    fn attach(
        &mut self,
        _host: &Host,
        _name: &str,
        _cid: u64,
        _stream: tokio::io::DuplexStream,
        _principal: Option<String>,
    ) -> Result<(), techne_vm::vm::Error> {
        Err(techne_vm::vm::Error::new("this worker attaches no REPL"))
    }
    /// REPL client `cid` is gone: a debug instance no client is attached
    /// to any more is detached.
    fn closed(&mut self, _host: &Host, _cid: u64) {}
    /// The REPL clients whose debug instance is gone (with its
    /// deployment, or poisoned), each said once: their connections are
    /// closed, so that no one is left talking to nothing.
    fn gone(&mut self, _host: &Host) -> Vec<u64> {
        Vec::new()
    }
    /// Serve a REPL client on `stream`, which the supervisor relays,
    /// authenticated as `principal` if as anyone.
    fn connect(&mut self, stream: tokio::io::DuplexStream, principal: Option<String>);
}

/// Bytes a relayed REPL connection may have waiting to be read here.
const REPL_QUEUE: usize = 64;

/// The most files of a deployment.
const BUNDLE_FILES: usize = 10_000;

/// Serve on the socket at descriptor `fd`, sandboxed unless `sandbox` is
/// false (a dev machine without user namespaces, as its supervisor
/// decides).
pub fn main(fd: RawFd, sandbox: bool) -> ExitCode {
    main_with(fd, sandbox, None)
}

/// `main`, doing for development what `dev` does.
pub fn main_with(fd: RawFd, sandbox: bool, dev: Option<Box<dyn Dev>>) -> ExitCode {
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
        local.block_on(&rt, async { serve(UnixStream::from_std(socket)?, dev).await })
    };
    match serve() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("techne worker: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn serve(socket: UnixStream, dev: Option<Box<dyn Dev>>) -> std::io::Result<()> {
    let dev = Rc::new(std::cell::RefCell::new(dev));
    let repls = Rc::new(std::cell::RefCell::new(HashMap::<u64, mpsc::Sender<Vec<u8>>>::new()));
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
    // Fetches are the supervisor's to make, each numbered here.
    let fetching = Rc::new(std::cell::RefCell::new((0u64, HashMap::<u64, oneshot::Sender<Result<Data, String>>>::new())));
    let (asking, ask) = (fetching.clone(), tx.clone());
    host.set_broker(Rc::new(move |o: crate::outbound::Outbound| {
        let (reply, rx) = oneshot::channel();
        let mut f = asking.borrow_mut();
        f.0 += 1;
        let fid = f.0;
        f.1.insert(fid, reply);
        let _ = ask.send(FromWorker::Fetch { rid: o.tag, fid, service: o.service.name().into(), request: o.request });
        rx
    }));
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
    // Close the connections of the REPL clients whose debug instance is
    // gone: here, and so for the supervisor and the client.
    let close_gone = {
        let (dev, repls, tx, host) = (dev.clone(), repls.clone(), tx.clone(), host.clone());
        move || {
            let gone = dev.borrow_mut().as_mut().map(|dev| dev.gone(&host)).unwrap_or_default();
            for cid in gone {
                if repls.borrow_mut().remove(&cid).is_some() {
                    let _ = tx.send(FromWorker::ReplClose { cid });
                }
            }
        }
    };
    // Now and then too: a debug world poisoned is gone with no message
    // to say so.
    let looking = close_gone.clone();
    tokio::task::spawn_local(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            looking();
        }
    });
    let _ = tx.send(FromWorker::Ready);
    let running = Rc::new(std::cell::RefCell::new(HashMap::<u64, Serving>::new()));
    // Where each deployment's files are, by the name it is published as.
    let unpacked = Rc::new(std::cell::RefCell::new(HashMap::<String, PathBuf>::new()));
    let driver = host.clone();
    let host_task = tokio::task::spawn_local(async move { driver.run(None).await });
    while let Some(d) = read_frame(&mut read).await? {
        match ToWorker::from_data(d)? {
            ToWorker::Deploy { id, name, entry, files, wall_ms, dev: for_dev, env } => {
                let (host, tx, dev, unpacked) = (host.clone(), tx.clone(), dev.clone(), unpacked.clone());
                tokio::task::spawn_local(async move {
                    let limits = Limits { wall: Duration::from_millis(wall_ms), ..Limits::default() };
                    // A directory of this worker's own, where it is not
                    // sandboxed too, and of this deployment's.
                    let root = std::env::temp_dir().join(format!("bundles-{}", std::process::id())).join(id.to_string());
                    let ours = !root.exists();
                    let loaded = match unpack(&root, &entry, &files) {
                        Ok(entry) => Deployment::load_async(&entry, limits).await.map(|d| (entry, d.with_env(env))),
                        Err(e) => Err(techne_vm::vm::Error::new(e)),
                    };
                    // One not loaded leaves no files (those it wrote: not
                    // what was there before it); one replaced under its
                    // name, those of the one before.
                    let before = match &loaded {
                        Ok(_) => unpacked.borrow_mut().insert(name.clone(), root),
                        Err(_) if ours => Some(root),
                        Err(_) => None,
                    };
                    if let Some(before) = before {
                        let _ = std::fs::remove_dir_all(before);
                    }
                    let published = loaded.and_then(|(entry, d)| {
                        let output = d.output().text.clone();
                        match &mut *dev.borrow_mut() {
                            Some(dev) if for_dev => dev.publish(&host, &name, &entry, d)?,
                            _ if for_dev => return Err(techne_vm::vm::Error::new("this worker has no development")),
                            dev => {
                                host.publish(&name, d);
                                if let Some(dev) = dev {
                                    dev.loaded(&name, &entry);
                                }
                            }
                        }
                        Ok(output)
                    });
                    let message = match published {
                        Ok(output) => FromWorker::Deployed { id, error: None, output },
                        Err(e) => FromWorker::Deployed { id, error: Some(e.msg.clone()), output: String::new() },
                    };
                    let _ = tx.send(message);
                });
            }
            ToWorker::Activate { name } => {
                if let Some(dev) = &mut *dev.borrow_mut() {
                    dev.activate(&host, &name);
                }
            }
            ToWorker::Deactivate { name } => {
                host.detach_debug(&name, None);
                close_gone();
            }
            ToWorker::Retire { name } => {
                host.unpublish(&name);
                if let Some(dev) = &mut *dev.borrow_mut() {
                    dev.retire(&name);
                }
                close_gone();
                if let Some(root) = unpacked.borrow_mut().remove(&name) {
                    let _ = std::fs::remove_dir_all(root);
                }
            }
            ToWorker::Request { rid, name, request, streams, debug } => {
                // Its body written to the host as it arrives, each chunk
                // credited once taken.
                let (queue, reader) = match streams {
                    true => {
                        let (writer, reader) = body::channel();
                        let (queue, pieces) = mpsc::channel(WINDOW + 1);
                        let tx = tx.clone();
                        tokio::task::spawn_local(body::relay(pieces, writer, move || {
                            let _ = tx.send(FromWorker::Credit { rid });
                        }));
                        (Some(queue), Some(reader))
                    }
                    false => (None, None),
                };
                let reply = host.fetch_routed(rid, &name, request, reader, debug);
                let credits = Rc::new(Semaphore::new(WINDOW));
                let (tx, running_, credits_) = (tx.clone(), running.clone(), credits.clone());
                let task = tokio::task::spawn_local(async move {
                    if let Ok(response) = reply.await {
                        let streams = response.body.is_some();
                        let _ = tx.send(FromWorker::Response { rid, response: response.head, streams });
                        if let Some(reader) = response.body {
                            body::pump(reader, credits_, |piece| {
                                tx.send(match piece {
                                    Piece::Bytes(bytes) => FromWorker::Body { rid, bytes },
                                    Piece::End(error) => FromWorker::BodyEnd { rid, error },
                                })
                                .is_ok()
                            })
                            .await;
                        }
                    }
                    running_.borrow_mut().remove(&rid);
                });
                running.borrow_mut().insert(rid, Serving { task: task.abort_handle(), body: queue, credits });
            }
            // Its receiver dropped: the host abandons the request. Done
            // with it, said even if it was before.
            ToWorker::Cancel { rid } => {
                if let Some(serving) = running.borrow_mut().remove(&rid) {
                    serving.task.abort();
                }
                let _ = tx.send(FromWorker::BodyEnd { rid, error: Some("cancelled".into()) });
            }
            // For a request served no longer: nothing to do. A body past
            // its credits is cut short.
            ToWorker::Body { rid, bytes } => {
                if let Some(s) = running.borrow_mut().get_mut(&rid)
                    && s.body.as_ref().is_some_and(|q| q.try_send(Piece::Bytes(bytes)).is_err())
                {
                    s.body = None;
                }
            }
            ToWorker::BodyEnd { rid, error } => {
                if let Some(q) = running.borrow_mut().get_mut(&rid).and_then(|s| s.body.take()) {
                    let _ = q.try_send(Piece::End(error));
                }
            }
            ToWorker::Credit { rid } => {
                if let Some(s) = running.borrow().get(&rid) {
                    s.credits.add_permits(1);
                }
            }
            ToWorker::Fetched { fid, response } => {
                if let Some(reply) = fetching.borrow_mut().1.remove(&fid) {
                    let _ = reply.send(response);
                }
            }
            ToWorker::ReplOpen { cid, principal, target } => {
                let (ours, theirs) = tokio::io::duplex(REPL_CHUNK);
                let connected = match (&mut *dev.borrow_mut(), target) {
                    (Some(dev), Some(target)) => dev.attach(&host, &target, cid, theirs, principal).is_ok(),
                    (Some(dev), None) => {
                        dev.connect(theirs, principal);
                        true
                    }
                    (None, _) => false,
                };
                // Those of a debug instance found gone meanwhile.
                close_gone();
                if !connected {
                    let _ = tx.send(FromWorker::ReplClose { cid });
                    continue;
                }
                let (mut from, mut to) = tokio::io::split(ours);
                let (queue, mut queued) = mpsc::channel::<Vec<u8>>(REPL_QUEUE);
                repls.borrow_mut().insert(cid, queue);
                tokio::task::spawn_local(async move {
                    use tokio::io::AsyncWriteExt;
                    while let Some(bytes) = queued.recv().await {
                        if to.write_all(&bytes).await.is_err() {
                            break;
                        }
                    }
                    let _ = to.shutdown().await;
                });
                let (tx, repls, dev, host) = (tx.clone(), repls.clone(), dev.clone(), host.clone());
                tokio::task::spawn_local(async move {
                    use tokio::io::AsyncReadExt;
                    let mut chunk = vec![0; REPL_CHUNK];
                    while let Ok(n @ 1..) = from.read(&mut chunk).await {
                        if tx.send(FromWorker::ReplData { cid, bytes: chunk[..n].to_vec() }).is_err() {
                            break;
                        }
                    }
                    // Closed here: closed for both.
                    if repls.borrow_mut().remove(&cid).is_some() {
                        let _ = tx.send(FromWorker::ReplClose { cid });
                        if let Some(dev) = &mut *dev.borrow_mut() {
                            dev.closed(&host, cid);
                        }
                    }
                });
            }
            ToWorker::ReplData { cid, bytes } => {
                // A connection that cannot keep up is closed.
                let slow = repls.borrow().get(&cid).is_some_and(|q| q.try_send(bytes).is_err());
                if slow {
                    repls.borrow_mut().remove(&cid);
                    let _ = tx.send(FromWorker::ReplClose { cid });
                    if let Some(dev) = &mut *dev.borrow_mut() {
                        dev.closed(&host, cid);
                    }
                }
            }
            ToWorker::ReplClose { cid } => {
                if repls.borrow_mut().remove(&cid).is_some()
                    && let Some(dev) = &mut *dev.borrow_mut()
                {
                    dev.closed(&host, cid);
                }
            }
        }
    }
    host_task.abort();
    Ok(())
}

/// Write a deployment's files to `root`, a directory of their own in the
/// tmpfs, each path relative and staying in it, each written once, a
/// bounded number and size: the entry's path there.
fn unpack(root: &Path, entry: &str, files: &[(String, Vec<u8>)]) -> Result<PathBuf, String> {
    use std::io::Write;
    if files.len() > BUNDLE_FILES || files.iter().map(|(_, b)| b.len()).sum::<usize>() > BUNDLE_BYTES {
        return Err("the deployment has too many files, or too large".into());
    }
    if root.exists() {
        return Err(format!("{}: a deployment was sent as that before", root.display()));
    }
    let io = |e: std::io::Error| e.to_string();
    std::fs::create_dir_all(root).map_err(io)?;
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
