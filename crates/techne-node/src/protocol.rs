//! The node protocol: MessagePack messages, each framed by its length as a
//! big-endian `u32`, over the node's stdin and stdout (the same framing as
//! emacs-tramp-rpc). Requests carry an id; the node answers each one,
//! possibly out of order (a `Read` waits for output while others proceed).
//! Requests with id 0 are notifications: their answers are ignored.

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use techne_process::{Exit, Stream};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Largest frame accepted; a larger length is a protocol error.
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
    /// until released; `written` is how it prints.
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

pub async fn write_frame<T: Serialize>(w: &mut (impl AsyncWrite + Unpin), msg: &T) -> std::io::Result<()> {
    let bytes = rmp_serde::to_vec_named(msg).map_err(std::io::Error::other)?;
    w.write_all(&(bytes.len() as u32).to_be_bytes()).await?;
    w.write_all(&bytes).await?;
    w.flush().await
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
