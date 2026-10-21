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
}

pub type Response = Result<Reply, String>;

#[derive(Serialize, Deserialize, Debug)]
pub struct Message<T> {
    pub id: u64,
    pub body: T,
}

/// The version of the framed node protocol (not nREPL).
pub const VERSION: u32 = 1;

/// The first frame in each direction, before any request or response.
#[derive(Serialize, Deserialize, Debug)]
pub struct Hello {
    pub version: u32,
}

/// How long the version exchange may take. It includes the transport
/// starting, an ssh connecting and authenticating as much as a local node.
pub const HANDSHAKE: std::time::Duration = std::time::Duration::from_secs(60);

/// Exchange versions before interpreting requests, within `HANDSHAKE`. This
/// also bounds the wait for a transport that starts an older, unversioned
/// node.
pub async fn handshake(r: &mut (impl AsyncRead + Unpin), w: &mut (impl AsyncWrite + Unpin)) -> std::io::Result<()> {
    let exchange = async {
        write_frame(w, &Hello { version: VERSION }).await?;
        let hello = read_frame::<Hello>(r).await?.ok_or_else(|| std::io::Error::other("peer closed before sending its version"))?;
        if hello.version != VERSION {
            return Err(std::io::Error::other(format!("version mismatch: expected {VERSION}, received {}", hello.version)));
        }
        Ok(())
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

/// Send a response, replacing an oversized one with a small error reply:
/// whether it was replaced (what it held never reached the peer).
pub async fn write_response(w: &mut (impl AsyncWrite + Unpin), msg: &Message<Response>) -> std::io::Result<bool> {
    match write_frame(w, msg).await {
        Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => {
            write_frame(w, &Message::<Response> { id: msg.id, body: Err(e.to_string()) }).await.map(|()| true)
        }
        result => result.map(|()| false),
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
