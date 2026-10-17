//! `techne cloud dev FILE`: a deployment served on a local port as the
//! service serves it (doc/serverless.md, milestone 2), without the
//! sandbox, the supervisor or accounts.
//!
//! FILE is the deployment's entry module, which provides `fetch`; it is
//! loaded as `techne-host` loads a deployment, confined to FILE's
//! directory and snapshotted, and each request runs in an instance started
//! from the snapshot, within the default limits. The front is
//! `lisp/http`'s server in a world of its own, handing each request to the
//! deployment with `host-fetch`. Saving any `.scm` or `.sld` file under
//! FILE's directory (but in hidden directories, `target` and
//! `node_modules`) deploys again: requests from then on go to the new
//! deployment; one that fails to load is reported and the one before keeps
//! serving, also while the new one loads. Failures and what requests print
//! go to standard error, marked with the request.

use std::{
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

use techne_host::{Deployment, Host, Limits};
use techne_vm::vm::{Error, Vm};

const USAGE: &str = "usage: techne cloud dev FILE [--port PORT]

Serve the deployment whose entry module is FILE on 127.0.0.1:PORT (8787
by default; 0 picks one), deploying again whenever a Lisp file under
FILE's directory changes.";

/// The name the deployment is published under.
const NAME: &str = "dev";

/// How often files are looked at for changes.
const POLL: Duration = Duration::from_millis(250);

pub fn main(args: Vec<String>) -> ExitCode {
    match args.split_first() {
        Some((dev, rest)) if dev == "dev" => match parse(rest) {
            Some((file, port)) => match dev_serve(&file, port) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("techne cloud: {}", e.msg);
                    ExitCode::FAILURE
                }
            },
            None => usage(),
        },
        _ => usage(),
    }
}

fn usage() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::from(2)
}

/// FILE and the port, from `FILE [--port PORT]`.
fn parse(args: &[String]) -> Option<(PathBuf, u16)> {
    match args {
        [file] => Some((file.into(), 8787)),
        [file, flag, port] | [flag, port, file] if flag == "--port" => Some((file.into(), port.parse().ok()?)),
        _ => None,
    }
}

fn dev_serve(file: &Path, port: u16) -> Result<(), Error> {
    let entry = file.canonicalize().map_err(|e| Error::new(format!("{}: {e}", file.display())))?;
    let dir = entry.parent().unwrap_or(Path::new("/")).to_path_buf();
    let host = Host::new();
    // The files as they were loaded: a save from now on deploys again.
    let seen = lisp_files(&dir);
    host.publish(NAME, Deployment::load(&entry, Limits::default())?);
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| Error::new(e.to_string()))?;
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async {
        let mut front = Vm::new();
        techne_net::install_on(&mut front, tokio::runtime::Handle::current())?;
        host.install(&mut front);
        let http = techne_vm::library::lisp_dir().join("http");
        front.eval_source(&format!(
            r#"(require {:?} {:?})
               (let ((listener (tcp-listen {port})))
                 (display (string-append "Serving " {:?} " on http://127.0.0.1:"
                                         (number->string (cadr (tcp-local-address listener))) "\n"))
                 (flush-output-port)
                 (spawn (lambda () (http-serve listener (lambda (request) (host-fetch {NAME:?} request))))))"#,
            http.join("http.scm").display().to_string(),
            http.join("server.scm").display().to_string(),
            entry.display().to_string(),
        ))?;
        tokio::task::spawn_local(redeploy_on_change(host.clone(), entry.clone(), dir, seen));
        host.run(&mut front).await;
        Ok(())
    })
}

/// Deploy `entry` again whenever a Lisp file under `dir` changes from
/// what was `seen`.
async fn redeploy_on_change(host: Host, entry: PathBuf, dir: PathBuf, mut seen: Vec<(PathBuf, Option<u64>)>) {
    // Looked at off the serving thread: a large tree takes a while.
    let scan = |dir: PathBuf| async move { tokio::task::spawn_blocking(move || lisp_files(&dir)).await.unwrap_or_default() };
    loop {
        tokio::time::sleep(POLL).await;
        let now = scan(dir.clone()).await;
        if now == seen {
            continue;
        }
        seen = now;
        eprintln!("techne cloud: loading {}", entry.display());
        match Deployment::load_async(&entry, Limits::default()).await {
            Ok(d) => {
                host.publish(NAME, d);
                eprintln!("techne cloud: deployed {}", entry.display());
            }
            Err(e) => eprintln!("techne cloud: not deployed, the last deployment keeps serving: {}", e.msg),
        }
    }
}

/// The Lisp files under `dir` and a hash of each one's contents, in
/// order (a save need not change a file's length or, within the clock's
/// resolution, its time); not those in hidden directories, `target` or
/// `node_modules`, which hold what is generated or fetched, nor through
/// symbolic links to directories.
fn lisp_files(dir: &Path) -> Vec<(PathBuf, Option<u64>)> {
    let mut files = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(d) = dirs.pop() {
        for entry in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(t) if t.is_dir() => {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    if !(name.starts_with('.') || name == "target" || name == "node_modules") {
                        dirs.push(path);
                    }
                }
                Ok(_) if path.extension().is_some_and(|e| e == "scm" || e == "sld") => {
                    let hash = std::fs::read(&path).ok().map(|bytes| {
                        let mut h = std::hash::DefaultHasher::new();
                        std::hash::Hash::hash(&bytes, &mut h);
                        std::hash::Hasher::finish(&h)
                    });
                    files.push((path, hash));
                }
                _ => {}
            }
        }
    }
    files.sort();
    files
}
