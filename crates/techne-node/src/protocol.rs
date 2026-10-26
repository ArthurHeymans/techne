//! The node protocol: MessagePack messages, each framed by its length as a
//! big-endian `u32`, over the node's stdin and stdout (the same framing as
//! emacs-tramp-rpc). Requests carry an id; the node answers each one,
//! possibly out of order (a `Read` waits for output while others proceed).
//! Requests with id 0 are notifications: their answers are ignored.
//!
//! Each side first sends a framed `Hello` and checks the peer's version
//! before exchanging requests. Frames are limited in both directions.

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use techne_process::{Exit, Stream};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Largest frame sent or accepted; a larger length is a protocol error.
pub const MAX_FRAME: usize = 16 << 20;

/// An argument sent to a remote procedure.
#[derive(Serialize, Deserialize, Debug)]
pub enum Arg {
    /// Data in written form.
    Data(String),
    /// A value the node holds for this client.
    Handle(u64),
}

/// What an evaluation produced.
#[derive(Serialize, Deserialize, Debug)]
pub enum Outcome {
    Void,
    /// Data in written form.
    Data(String),
    /// Not data (a procedure, record, ...): the node keeps it under `id`
    /// until released; `written` is a bounded preview, or a fixed marker
    /// when a cycle, depth or size limit prevents writing it.
    Handle {
        id: u64,
        written: String,
    },
}

/// A process the node runs, for reconciling after a reconnect.
#[derive(Serialize, Deserialize, Debug)]
pub struct ProcInfo {
    pub proc: u64,
    pub pid: i64,
    pub command: Vec<String>,
    pub pty: bool,
    pub persistent: bool,
    /// `None` while running.
    pub exit: Option<Exit>,
    /// Output bytes dropped unread.
    pub dropped: u64,
}

/// What a file must be for a write to replace it.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    /// Anything, or nothing.
    Any,
    /// There must be no file.
    Absent,
    /// Its bytes must have this hash (BLAKE3).
    Hash(#[serde(with = "serde_bytes")] [u8; 32]),
}

/// A file's metadata.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct FileStat {
    pub kind: FileKind,
    pub size: u64,
    /// Seconds since the epoch.
    pub modified: i64,
    pub mode: u32,
    pub device: u64,
    pub inode: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    File,
    Directory,
    Symlink,
    Other,
}

/// A file's bytes and their hash.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct FileData {
    #[serde(with = "serde_bytes")]
    pub bytes: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub hash: [u8; 32],
}

/// An entry of a directory.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    #[serde(with = "serde_bytes")]
    pub name: Vec<u8>,
    pub directory: bool,
}

/// What a node is, as it reports in its hello (doc/remote.md, "Endpoints
/// report what they are"). A fact it cannot tell is `None`.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct Facts {
    pub machine: Option<String>,
    pub boot: Option<String>,
    pub uid: u32,
    pub user: Option<String>,
    #[serde(with = "serde_bytes")]
    pub home: Vec<u8>,
    pub shell: Option<String>,
    pub mount_namespace: Option<u64>,
    pub pid_namespace: Option<u64>,
    pub arch: String,
    pub os: String,
    pub host: Option<String>,
    /// The node's own executable: what a hop on the same machine runs,
    /// with `arguments` (`node`, for `techne node`).
    #[serde(with = "serde_bytes")]
    pub program: Vec<u8>,
    pub arguments: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum Request {
    /// Evaluate source text in a module of the node (by name or file path;
    /// `user` when `None`).
    Eval {
        source: String,
        #[serde(default)]
        module: Option<String>,
    },
    /// Call the procedure held as `f` with `args`.
    Apply {
        f: u64,
        args: Vec<Arg>,
    },
    /// `help` text for a global name.
    Describe {
        name: String,
    },
    /// `help` text for a held value.
    DescribeHandle {
        id: u64,
    },
    /// The client dropped a handle.
    ReleaseHandle {
        id: u64,
    },
    /// The node's processes (persistent ones survive disconnects).
    ListProcesses,
    /// A handle on a process the node runs (after a reconnect).
    Attach {
        proc: u64,
    },
    /// End the node (a session daemon): kill its processes and exit.
    Shutdown,
    /// Interrupt the evaluation in progress.
    Interrupt,
    Spawn {
        program: String,
        args: Vec<String>,
        pty: bool,
        persist: bool,
        /// The working directory, else the node's.
        #[serde(default, with = "serde_bytes")]
        directory: Option<Vec<u8>>,
        /// Variables set besides those the node's environment has.
        #[serde(default)]
        environment: Vec<(String, String)>,
    },
    /// What `path` is, or `Reply::Stat(None)` if there is nothing there.
    /// Paths are bytes; `~` and relative ones start at the node's home.
    Stat {
        #[serde(with = "serde_bytes")]
        path: Vec<u8>,
    },
    /// The bytes of the regular file at `path` (`Reply::File(None)`: there
    /// is none) and their hash (`Reply::File`).
    ReadFile {
        #[serde(with = "serde_bytes")]
        path: Vec<u8>,
    },
    /// Replace the file at `path` with `data`, atomically, if it is as
    /// `expect` says; else `Reply::Conflict`. A symbolic link is followed.
    WriteFile {
        #[serde(with = "serde_bytes")]
        path: Vec<u8>,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
        expect: Expect,
    },
    /// The entries of a directory (`Reply::Entries`).
    ListDirectory {
        #[serde(with = "serde_bytes")]
        path: Vec<u8>,
    },
    /// The nearest directory from `directory` up that has an entry named
    /// one of `names` (`Reply::Path`).
    FindUp {
        #[serde(with = "serde_bytes")]
        directory: Vec<u8>,
        names: Vec<String>,
    },
    Read {
        proc: u64,
        stream: Stream,
    },
    Write {
        proc: u64,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
    CloseInput {
        proc: u64,
    },
    Signal {
        proc: u64,
        signal: String,
    },
    Resize {
        proc: u64,
        rows: u16,
        cols: u16,
    },
    Dropped {
        proc: u64,
    },
    Wait {
        proc: u64,
    },
    Exited {
        proc: u64,
    },
    Kill {
        proc: u64,
    },
    /// The client dropped its handle: kill the process if it runs (unless
    /// it is persistent), forget it once it has exited.
    Release {
        proc: u64,
    },
}

#[derive(Serialize, Deserialize, Debug)]
pub enum Reply {
    Unit,
    /// An evaluation's outcome and what it printed.
    Evaluated {
        outcome: Outcome,
        output: String,
    },
    Processes(Vec<ProcInfo>),
    Int(i64),
    Text(String),
    Spawned {
        proc: u64,
        pid: i64,
    },
    Chunk(#[serde(with = "serde_bytes")] Option<Vec<u8>>),
    Exit(Exit),
    Bool(bool),
    Stat(Option<FileStat>),
    /// A file's bytes and their hash, or nothing.
    File(Option<FileData>),
    /// Written: the hash of what the file now has.
    Written(#[serde(with = "serde_bytes")] [u8; 32]),
    /// Not written: the file is not as expected. The hash of what it has,
    /// or `None` when there is none.
    Conflict(#[serde(with = "serde_bytes")] Option<[u8; 32]>),
    Entries(Vec<DirEntry>),
    Path(#[serde(with = "serde_bytes")] Option<Vec<u8>>),
}

pub type Response = Result<Reply, String>;

#[derive(Serialize, Deserialize, Debug)]
pub struct Message<T> {
    pub id: u64,
    pub body: T,
}

/// The version of the framed node protocol (not nREPL).
pub const VERSION: u32 = 2;

/// The first frame in each direction, before any request or response.
#[derive(Serialize, Deserialize, Debug)]
pub struct Hello {
    pub version: u32,
    /// The node's: what it is. A client sends none.
    #[serde(default)]
    pub facts: Option<Facts>,
}

/// How long the version exchange may take. It includes the transport
/// starting, an ssh connecting and authenticating as much as a local node.
pub const HANDSHAKE: std::time::Duration = std::time::Duration::from_secs(60);

/// Exchange versions before interpreting requests, within `HANDSHAKE`. This
/// also bounds the wait for a transport that starts an older, unversioned
/// node.
/// `facts` are this end's, a node's; the peer's are returned.
pub async fn handshake(
    r: &mut (impl AsyncRead + Unpin),
    w: &mut (impl AsyncWrite + Unpin),
    facts: Option<Facts>,
) -> std::io::Result<Option<Facts>> {
    let exchange = async {
        write_frame(w, &Hello { version: VERSION, facts }).await?;
        let hello = read_frame::<Hello>(r).await?.ok_or_else(|| std::io::Error::other("peer closed before sending its version"))?;
        if hello.version != VERSION {
            return Err(std::io::Error::other(format!("version mismatch: expected {VERSION}, received {}", hello.version)));
        }
        Ok(hello.facts)
    };
    tokio::time::timeout(HANDSHAKE, exchange)
        .await
        .unwrap_or_else(|_| Err(std::io::Error::other("timed out waiting for peer version")))
        .map_err(|e| std::io::Error::other(format!("node protocol version handshake failed: {e}")))
}

#[derive(Default)]
struct Frame {
    bytes: Vec<u8>,
    oversized: bool,
}

impl std::io::Write for Frame {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_FRAME.saturating_sub(self.bytes.len()) {
            self.oversized = true;
            return Err(frame_limit());
        }
        let needed = self.bytes.len() + bytes.len();
        if needed > self.bytes.capacity() {
            let capacity = needed.max(self.bytes.capacity().saturating_mul(2)).min(MAX_FRAME);
            self.bytes.reserve_exact(capacity - self.bytes.len());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn frame_limit() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("node frame exceeds the {MAX_FRAME}-byte limit"))
}

/// Serialize within the frame limit before writing any bytes to the peer.
/// Refusal leaves the stream untouched, so a smaller error can be sent.
pub async fn write_frame<T: Serialize>(w: &mut (impl AsyncWrite + Unpin), msg: &T) -> std::io::Result<()> {
    let mut frame = Frame::default();
    if let Err(e) = msg.serialize(&mut rmp_serde::Serializer::new(&mut frame).with_struct_map()) {
        return Err(if frame.oversized { frame_limit() } else { std::io::Error::other(e) });
    }
    w.write_all(&(frame.bytes.len() as u32).to_be_bytes()).await?;
    w.write_all(&frame.bytes).await?;
    w.flush().await
}

/// Send a response, replacing an oversized one with a small error reply,
/// calling `replaced` first: what it held never reaches the peer, and is
/// let go before the peer can ask anything more.
pub async fn write_response(
    w: &mut (impl AsyncWrite + Unpin),
    msg: &Message<Response>,
    replaced: impl FnOnce(&Message<Response>),
) -> std::io::Result<()> {
    match write_frame(w, msg).await {
        Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => {
            replaced(msg);
            write_frame(w, &Message::<Response> { id: msg.id, body: Err(e.to_string()) }).await
        }
        result => result,
    }
}

/// The next message, or `None` at a clean end of stream.
pub async fn read_frame<T: DeserializeOwned>(r: &mut (impl AsyncRead + Unpin)) -> std::io::Result<Option<T>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(std::io::Error::other(format!("frame of {len} bytes exceeds the {MAX_FRAME}-byte limit")));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    rmp_serde::from_slice(&buf).map(Some).map_err(std::io::Error::other)
}
