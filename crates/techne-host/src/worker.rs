//! A worker process: deployments served by a [`Host`] of its own, for its
//! supervisor ([`crate::supervisor`]), over the socket it was started with
//! (`wire`), sandboxed (`techne_sandbox`) before it reads anything of its
//! tenant's.
//!
//! It waits for a byte from its supervisor (who puts it in its cgroup
//! first), enters its sandbox, reading Techne's Lisp and writing nothing
//! of a deployment's sources to disk, says it is ready, and then serves:
//! a deployment's files and packages, sent to it, are validated and loaded
//! directly from an in-memory read-only [`techne_vm::bundle::BundleVfs`];
//! requests are answered as the host answers them; failures and output are
//! reported to the supervisor. A Rust native that never looks for stops,
//! holding the worker's thread past a kill, ends the worker (`STUCK`); the
//! end of its socket, its supervisor gone, ends it too.
//!
//! For development ([`Dev`]), a deployment is published to a dev world
//! with a REPL server, whose clients the supervisor relays: each a
//! connection of its own, as bytes both ways, over an in-memory pipe here.
//! A connection that falls behind what is sent to it is closed.

use std::{
    collections::{BTreeMap, HashMap},
    io::Read,
    os::fd::{FromRawFd, RawFd},
    path::{Path, PathBuf},
    process::ExitCode,
    rc::Rc,
    sync::Arc,
    time::Duration,
};

use techne_vm::{
    bundle::{BundlePackage, BundleVfs, PackagePrefix},
    data::Data,
};
use tokio::{
    net::UnixStream,
    sync::{Semaphore, mpsc, oneshot},
    task::AbortHandle,
};

use crate::{
    Deployment, Host, Limits,
    body::{self, Piece},
    wire::{BUNDLE_BYTES, BundledPackage, FromWorker, REPL_CHUNK, ToWorker, WINDOW, read_frame, write_frame},
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
    let driver = host.clone();
    let host_task = tokio::task::spawn_local(async move { driver.run(None).await });
    while let Some(d) = read_frame(&mut read).await? {
        match ToWorker::from_data(d)? {
            ToWorker::Deploy { id, name, entry, files, wall_ms, dev: for_dev, env, root_package, packages } => {
                let (host, tx, dev) = (host.clone(), tx.clone(), dev.clone());
                tokio::task::spawn_local(async move {
                    let limits = Limits { wall: Duration::from_millis(wall_ms), ..Limits::default() };
                    let loaded = match build_bundle_vfs(&entry, files, root_package, packages) {
                        Ok((bundle, entry_vpath)) => {
                            Deployment::load_bundle_async(bundle, &entry, limits).await.map(|d| (entry_vpath, d.with_env(env)))
                        }
                        Err(e) => Err(techne_vm::vm::Error::new(e)),
                    };
                    let published = loaded.and_then(|(entry_vpath, d)| {
                        let output = d.output().text.clone();
                        match &mut *dev.borrow_mut() {
                            Some(dev) if for_dev => dev.publish(&host, &name, &entry_vpath, d)?,
                            _ if for_dev => return Err(techne_vm::vm::Error::new("this worker has no development")),
                            dev => {
                                host.publish(&name, d);
                                if let Some(dev) = dev {
                                    dev.loaded(&name, &entry_vpath);
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
                close_gone();
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
            ToWorker::PatchLibrary { id, name, library, code, check_retained } => {
                let result = host.patch_library(&name, &library, &code, check_retained);
                let dirty = host.is_dirty(&name);
                let _ = tx.send(FromWorker::PatchedLibrary { id, result, dirty });
            }
        }
    }
    host_task.abort();
    Ok(())
}

/// Build an immutable, read-only in-memory [`BundleVfs`] from a deployment's
/// root files and optional package graph, validating file counts, byte sizes,
/// uniqueness, package graph integrity, and entry existence without touching
/// the OS filesystem. Returns the bundle and the entry's deterministic virtual
/// path.
pub(crate) fn build_bundle_vfs(
    entry: &str,
    files: Vec<(String, Vec<u8>)>,
    root_package: Option<(String, String)>,
    packages: Vec<BundledPackage>,
) -> Result<(Arc<BundleVfs>, PathBuf), String> {
    if root_package.is_none() && !packages.is_empty() {
        return Err("deployment carries packages without a root package".into());
    }
    if root_package.is_some() && packages.is_empty() {
        return Err("packaged deployment carries no packages".into());
    }
    if root_package.is_some() && !files.is_empty() {
        return Err("packaged deployment cannot carry root files".into());
    }
    let mut total_files = files.len();
    let mut total_bytes: usize = files.iter().map(|(_, b)| b.len()).sum();
    for pkg in &packages {
        total_files = total_files.saturating_add(pkg.files.len());
        total_bytes = total_bytes.saturating_add(pkg.files.iter().map(|(_, b)| b.len()).sum::<usize>());
    }
    if total_files > BUNDLE_FILES || total_bytes > BUNDLE_BYTES {
        return Err("the deployment has too many files, or too large".into());
    }
    let mut root_files = BTreeMap::new();
    for (path, bytes) in files {
        if root_files.insert(path.clone(), Arc::<[u8]>::from(bytes)).is_some() {
            return Err(format!("{path}: duplicate file in deployment"));
        }
    }
    let mut pkg_map = BTreeMap::new();
    for pkg in packages {
        if !crate::package::is_sha256_hex(&pkg.tree_sha256) {
            return Err(format!("package ({} {}) has invalid tree-sha256: {}", pkg.owner, pkg.pkg, pkg.tree_sha256));
        }
        let actual_sha = crate::package::canonical_tree_sha256(&pkg.files);
        if actual_sha != pkg.tree_sha256 {
            return Err(format!(
                "package ({} {}) tree-sha256 mismatch: expected {}, got {actual_sha}",
                pkg.owner, pkg.pkg, pkg.tree_sha256
            ));
        }
        let prefix = PackagePrefix::new(&pkg.owner, &pkg.pkg);
        let mut pkg_files = BTreeMap::new();
        for (path, bytes) in pkg.files.iter() {
            if pkg_files.insert(path.clone(), Arc::<[u8]>::from(bytes.as_slice())).is_some() {
                return Err(format!("{prefix}/{path}: duplicate file in package"));
            }
        }
        let declared_deps = pkg.depends.into_iter().map(|(o, p)| PackagePrefix::new(&o, &p)).collect();
        let entry_pkg = BundlePackage { tree_sha256: pkg.tree_sha256, files: pkg_files, declared_deps };
        if pkg_map.insert(prefix.clone(), entry_pkg).is_some() {
            return Err(format!("duplicate package {prefix} in deployment"));
        }
    }
    let root_prefix = root_package.map(|(o, p)| PackagePrefix::new(&o, &p));
    let vfs = BundleVfs::new(root_files, root_prefix, pkg_map).map_err(|e| e.msg.clone())?;
    let entry_vpath = vfs.entry_virtual_path(entry).map_err(|e| e.msg.clone())?;
    Ok((Arc::new(vfs), entry_vpath))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_bundle_vfs_verifies_digests_and_consistency() {
        let files = vec![
            ("app.scm".to_string(), b"(provide fetch)".to_vec()),
            ("techne-package.scm".to_string(), b"(package (acme app) (version \"0.1.0\"))".to_vec()),
        ];
        let good_sha = crate::package::canonical_tree_sha256(&files);
        let good_pkg = BundledPackage {
            owner: "acme".into(),
            pkg: "app".into(),
            tree_sha256: good_sha.clone(),
            depends: vec![],
            files: Arc::new(files.clone()),
        };

        // Valid packaged bundle succeeds.
        let (_, vpath) = build_bundle_vfs("app.scm", vec![], Some(("acme".into(), "app".into())), vec![good_pkg.clone()]).unwrap();
        assert_eq!(vpath, PathBuf::from("/pkg/acme/app/app.scm"));

        // Tampered file byte with stale tree_sha256 is rejected.
        let mut tampered_files = files;
        tampered_files[0].1 = b"(provide fetch) ;; tampered".to_vec();
        let bad_pkg = BundledPackage { files: Arc::new(tampered_files), ..good_pkg.clone() };
        let err = build_bundle_vfs("app.scm", vec![], Some(("acme".into(), "app".into())), vec![bad_pkg]).unwrap_err();
        assert!(err.contains("tree-sha256 mismatch"), "{err}");

        // Non-hex digest rejected.
        let bad_hex = BundledPackage { tree_sha256: "not-a-sha256".into(), ..good_pkg.clone() };
        let err = build_bundle_vfs("app.scm", vec![], Some(("acme".into(), "app".into())), vec![bad_hex]).unwrap_err();
        assert!(err.contains("invalid tree-sha256"), "{err}");

        // Root files + packaged deployment rejected.
        let err = build_bundle_vfs("app.scm", vec![("x.scm".into(), vec![])], Some(("acme".into(), "app".into())), vec![good_pkg.clone()])
            .unwrap_err();
        assert!(err.contains("cannot carry root files"), "{err}");

        // Packages without root_package rejected.
        let err = build_bundle_vfs("app.scm", vec![], None, vec![good_pkg]).unwrap_err();
        assert!(err.contains("without a root package"), "{err}");
    }

    struct DummyAttached;
    impl crate::Attached for DummyAttached {
        fn poll(&mut self, _vm: &mut techne_vm::vm::Vm) -> bool {
            false
        }
        fn reap(&mut self, _vm: &mut techne_vm::vm::Vm) {}
    }

    struct DummyDev;
    impl Dev for DummyDev {
        fn publish(&mut self, host: &Host, name: &str, _entry: &Path, deployment: Deployment) -> Result<(), techne_vm::vm::Error> {
            host.publish_dev(name, deployment, |_| Ok(Box::new(DummyAttached)))
        }
        fn connect(&mut self, _stream: tokio::io::DuplexStream, _principal: Option<String>) {}
    }

    #[test]
    fn worker_ipc_roundtrips_truncated_stale_holders_and_stays_alive() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, async {
            let (mut supervisor_end, worker_end) = UnixStream::pair().unwrap();
            let server = tokio::task::spawn_local(async move { serve(worker_end, Some(Box::new(DummyDev))).await });

            // Wait for Ready.
            let ready = read_frame(&mut supervisor_end).await.unwrap().unwrap();
            assert!(matches!(FromWorker::from_data(ready).unwrap(), FromWorker::Ready));

            let mut exports = String::new();
            let mut defs_v1 = String::new();
            let mut defs_v2 = String::new();
            let mut vec_items = String::new();
            for i in 0..270 {
                exports.push_str(&format!(" f{i}"));
                defs_v1.push_str(&format!("(define (f{i}) \"v1-{i}\")\n"));
                defs_v2.push_str(&format!("(define (f{i}) \"v2-{i}\")\n"));
                vec_items.push_str(&format!(" f{i}"));
            }
            let lib_v1 = format!("(define-library (alice many util) (export{exports}) (import (techne)) (begin {defs_v1}))");
            let lib_v2 = format!("(define-library (alice many util) (export{exports}) (import (techne)) (begin {defs_v2}))");

            let mut pkg_files = vec![
                (
                    "app.scm".to_string(),
                    format!(
                        "(import (alice many util))\n(define retained (vector{vec_items}))\n(provide fetch)\n(define (fetch req env ctx) (dict 'status 200 'body (f0)))\n"
                    )
                    .into_bytes(),
                ),
                ("lib/alice/many/util.sld".to_string(), lib_v1.into_bytes()),
                ("techne-package.scm".to_string(), br#"(package (alice many) (version "0.1.0") (cloud "app.scm"))"#.to_vec()),
            ];
            pkg_files.sort_by(|a, b| a.0.cmp(&b.0));
            let tree_sha = crate::package::canonical_tree_sha256(&pkg_files);

            // Deploy dev bundle over real wire frames.
            write_frame(
                &mut supervisor_end,
                &ToWorker::Deploy {
                    id: 1,
                    name: "many@1".into(),
                    entry: "app.scm".into(),
                    files: vec![],
                    wall_ms: 5000,
                    dev: true,
                    env: Data::Dict(vec![]),
                    root_package: Some(("alice".into(), "many".into())),
                    packages: vec![BundledPackage {
                        owner: "alice".into(),
                        pkg: "many".into(),
                        tree_sha256: tree_sha,
                        depends: vec![],
                        files: Arc::new(pkg_files),
                    }],
                }
                .to_data(),
            )
            .await
            .unwrap();
            let deployed = FromWorker::from_data(read_frame(&mut supervisor_end).await.unwrap().unwrap()).unwrap();
            assert!(matches!(deployed, FromWorker::Deployed { id: 1, error: None, .. }), "{deployed:?}");

            // Patch library with 270 retained closures (> MAX_STALE_HOLDERS = 256).
            write_frame(
                &mut supervisor_end,
                &ToWorker::PatchLibrary {
                    id: 7,
                    name: "many@1".into(),
                    library: vec!["alice".into(), "many".into(), "util".into()],
                    code: lib_v2,
                    check_retained: true,
                }
                .to_data(),
            )
            .await
            .unwrap();

            let patched = FromWorker::from_data(read_frame(&mut supervisor_end).await.unwrap().unwrap()).unwrap();
            match patched {
                FromWorker::PatchedLibrary { id: 7, result: Ok(report), dirty } => {
                    assert!(dirty);
                    assert_eq!(report.patched.len(), 270);
                    assert!(report.stale_holders_truncated, "expected stale_holders_truncated=true over wire IPC");
                    assert!(report.stale_holders.len() <= techne_vm::patch::MAX_STALE_HOLDERS);
                    for h in &report.stale_holders {
                        assert!(h.chain.len() <= techne_vm::patch::MAX_STALE_CHAIN);
                    }
                }
                other => panic!("unexpected {other:?}"),
            }

            // Worker stays alive and serves subsequent requests with v2!
            write_frame(
                &mut supervisor_end,
                &ToWorker::Request {
                    rid: 1,
                    name: "many@1".into(),
                    request: Data::Dict(vec![
                        (Data::Symbol("method".into()), Data::String("GET".into())),
                        (Data::Symbol("path".into()), Data::String("/".into())),
                    ]),
                    streams: false,
                    debug: false,
                }
                .to_data(),
            )
            .await
            .unwrap();
            let resp = FromWorker::from_data(read_frame(&mut supervisor_end).await.unwrap().unwrap()).unwrap();
            match resp {
                FromWorker::Response { rid: 1, response: Data::Dict(entries), .. } => {
                    let body = entries.iter().find(|(k, _)| matches!(k, Data::Symbol(s) if s == "body")).map(|(_, v)| v);
                    assert_eq!(body, Some(&Data::String("v2-0".into())));
                }
                other => panic!("unexpected {other:?}"),
            }

            drop(supervisor_end);
            server.await.unwrap().unwrap();
        });
    }
}
