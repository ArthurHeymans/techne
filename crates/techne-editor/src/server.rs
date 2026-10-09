//! Opening files in a running editor, as `emacsclient` does: a frontend's
//! process listens on a unix socket in a private directory
//! (`Host::listen`), and `techne --open FILE` asks it to show the file
//! (`open`), with `--wait` until the file is done with
//! (`lisp/editor/server.scm`), so that the editor serves as `$EDITOR`.
//!
//! Messages are framed as the node protocol frames them (`techne-node`):
//! MessagePack, each preceded by its length as a big-endian `u32`. A
//! connection carries one request and its replies.

use std::{
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::host::Shared;
use crate::runtime::Msg;

/// Largest frame accepted; a larger length is a protocol error.
const MAX_FRAME: usize = 1 << 20;

#[derive(Serialize, Deserialize, Debug)]
pub enum Request {
    /// Show the file at `path` (absolute) in the session used last; with
    /// `wait`, reply `Done` too when it is done with.
    Open { path: PathBuf, wait: bool },
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub enum Reply {
    Opened,
    Done,
    Failed(String),
}

/// The socket a running editor listens on: `techne/editor` in
/// `$XDG_RUNTIME_DIR`, the directory made private. None without one.
pub fn socket() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join("techne");
    let mut builder = std::fs::DirBuilder::new();
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.recursive(true).create(&dir).ok()?;
    // Private also when it was made before it had to be.
    std::fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o700)).ok()?;
    Some(dir.join("editor"))
}

/// Why `open` did not open the file.
#[derive(Debug)]
pub enum OpenError {
    /// No editor listens on the socket.
    NotRunning,
    /// The editor refused, or went away before it was done.
    Failed(String),
}

/// Ask the editor listening on `socket` to show the file at `path`; with
/// `wait`, return only once it is done with.
pub fn open(socket: &Path, path: &Path, wait: bool) -> Result<(), OpenError> {
    let mut stream = UnixStream::connect(socket).map_err(|_| OpenError::NotRunning)?;
    let path = std::path::absolute(path).map_err(|e| OpenError::Failed(format!("{}: {e}", path.display())))?;
    let failed = |e: std::io::Error| OpenError::Failed(e.to_string());
    write_frame(&mut stream, &Request::Open { path, wait }).map_err(failed)?;
    loop {
        match read_frame::<Reply>(&mut stream).map_err(failed)? {
            Some(Reply::Opened) if !wait => return Ok(()),
            Some(Reply::Opened) => {}
            Some(Reply::Done) => return Ok(()),
            Some(Reply::Failed(e)) => return Err(OpenError::Failed(e)),
            None => return Err(OpenError::Failed("the editor stopped before the file was done with".into())),
        }
    }
}

/// For `--open`: show the file at `path` in the editor running, if one
/// listens on `socket()`, and with `wait` return once it is done with.
/// None when none does: the caller is to be the editor.
pub fn open_running(path: &Path, wait: bool) -> Option<Result<(), String>> {
    match open(&socket()?, path, wait) {
        Err(OpenError::NotRunning) => None,
        Err(OpenError::Failed(e)) => Some(Err(e)),
        Ok(()) => Some(Ok(())),
    }
}

/// Open files in the runtime of `host` for other programs, unless an
/// editor listens on `socket()` already. A failure to listen is reported
/// on stderr: the editor works without.
pub fn listen_for(host: &crate::host::Host) -> Option<Listener> {
    let socket = socket()?;
    host.listen(&socket).unwrap_or_else(|e| {
        eprintln!("techne: {}: {e}", socket.display());
        None
    })
}

/// The most requests served at once; more are refused.
const MAX_REQUESTS: usize = 64;

/// How long a request may take to arrive once connected.
const REQUEST_WAIT: Duration = Duration::from_secs(5);

/// How often a program waiting for a file is checked for having gone.
const GONE_CHECK: Duration = Duration::from_secs(1);

/// The socket listened on, with the lock that makes it this editor's.
/// Dropped, it stops listening and removes the socket, so that another
/// editor may listen.
pub struct Listener {
    socket: PathBuf,
    stop: Arc<AtomicBool>,
    _lock: std::fs::File,
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Wakes the thread accepting, which sees it is to stop.
        let _ = UnixStream::connect(&self.socket);
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// Listen on `socket` for requests to the runtime of `shared`, unless an
/// editor listens there already (None then). The editor holding the lock
/// beside it owns the socket: one left behind by an editor that is gone is
/// replaced, one of an editor running never is.
pub(crate) fn listen(shared: Arc<Mutex<Shared>>, socket: &Path) -> std::io::Result<Option<Listener>> {
    let lock = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(socket.with_extension("lock"))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
        Err(std::fs::TryLockError::Error(e)) => return Err(e),
    }
    let _ = std::fs::remove_file(socket);
    let listener = UnixListener::bind(socket)?;
    let stop = Arc::new(AtomicBool::new(false));
    let serving = Arc::new(AtomicUsize::new(0));
    let stopped = stop.clone();
    std::thread::Builder::new().name("editor server".into()).spawn(move || {
        for stream in listener.incoming() {
            if stopped.load(Ordering::Relaxed) {
                return;
            }
            let Ok(mut stream) = stream else { continue };
            if serving.fetch_add(1, Ordering::Relaxed) >= MAX_REQUESTS {
                serving.fetch_sub(1, Ordering::Relaxed);
                let _ = write_frame(&mut stream, &Reply::Failed("the editor is busy".into()));
                continue;
            }
            // Counted while the thread runs, or until it fails to start.
            let counted = Counted(serving.clone());
            let shared = shared.clone();
            let _ = std::thread::Builder::new().name("editor request".into()).spawn(move || {
                serve(&shared, stream);
                drop(counted);
            });
        }
    })?;
    Ok(Some(Listener { socket: socket.to_owned(), stop, _lock: lock }))
}

/// A request served, uncounted when dropped.
struct Counted(Arc<AtomicUsize>);

impl Drop for Counted {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Handle a connection's request: hand it to the runtime, and pass its
/// replies on until the last, or until the program asking is gone.
fn serve(shared: &Mutex<Shared>, mut stream: UnixStream) {
    let _ = stream.set_read_timeout(Some(REQUEST_WAIT));
    let Ok(Some(Request::Open { path, wait })) = read_frame::<Request>(&mut stream) else { return };
    static NEXT: AtomicUsize = AtomicUsize::new(1);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let (tx, replies) = mpsc::channel();
    let send = |msg| shared.lock().expect("the host").inputs.as_ref().is_some_and(|i| i.send(msg).is_ok());
    // The runtime it went to.
    let sent = {
        let s = shared.lock().expect("the host");
        s.inputs.as_ref().is_some_and(|i| i.send(Msg::Open { id, path, wait, reply: tx }).is_ok()).then_some(s.generation)
    };
    let Some(generation) = sent else {
        let _ = write_frame(&mut stream, &Reply::Failed("the editor is not running".into()));
        return;
    };
    // A runtime given up keeps the sender: it may still answer, but no
    // longer for the editor.
    let given_up = || shared.lock().expect("the host").given_up == Some(generation);
    let restarted = Reply::Failed("the editor restarted; open the file again".into());
    // Until the runtime drops the sender: done, refused, or gone.
    loop {
        match replies.recv_timeout(GONE_CHECK) {
            Ok(_) if given_up() => {
                let _ = write_frame(&mut stream, &restarted);
                return;
            }
            Ok(reply) => {
                if write_frame(&mut stream, &reply).is_err() {
                    send(Msg::Gone(id));
                    return;
                }
            }
            // The file is no longer waited for: quitting is quitting again.
            Err(mpsc::RecvTimeoutError::Timeout) if gone(&stream) => {
                send(Msg::Gone(id));
                return;
            }
            Err(mpsc::RecvTimeoutError::Timeout) if given_up() => {
                let _ = write_frame(&mut stream, &restarted);
                return;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Whether the program at the other end has closed the connection: it
/// sends nothing after its request.
fn gone(stream: &UnixStream) -> bool {
    if stream.set_nonblocking(true).is_err() {
        return true;
    }
    let gone = !matches!((&*stream).read(&mut [0]), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock);
    let _ = stream.set_nonblocking(false);
    gone
}

fn write_frame<T: Serialize>(w: &mut impl Write, msg: &T) -> std::io::Result<()> {
    let bytes = rmp_serde::to_vec_named(msg).map_err(std::io::Error::other)?;
    w.write_all(&(bytes.len() as u32).to_be_bytes())?;
    w.write_all(&bytes)?;
    w.flush()
}

/// The next message, or `None` at a clean end of stream.
fn read_frame<T: DeserializeOwned>(r: &mut impl Read) -> std::io::Result<Option<T>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(std::io::Error::other(format!("frame of {len} bytes exceeds the {MAX_FRAME}-byte limit")));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    rmp_serde::from_slice(&buf).map(Some).map_err(std::io::Error::other)
}
