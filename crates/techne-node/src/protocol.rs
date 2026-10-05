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

#[derive(Serialize, Deserialize, Debug)]
pub enum Request {
    /// Evaluate source text in the node's user module.
    Eval { source: String },
    /// `help` text for a global name.
    Describe { name: String },
    /// Interrupt the evaluation in progress.
    Interrupt,
    Spawn { program: String, args: Vec<String>, pty: bool },
    Read { proc: u64, stream: Stream },
    Write { proc: u64, data: String },
    CloseInput { proc: u64 },
    Signal { proc: u64, signal: String },
    Wait { proc: u64 },
    Exited { proc: u64 },
    Kill { proc: u64 },
    /// The client dropped its handle: kill the process if it runs, forget it.
    Release { proc: u64 },
}

#[derive(Serialize, Deserialize, Debug)]
pub enum Reply {
    Unit,
    /// An evaluation's value in written form (`None` for no value) and what
    /// it printed.
    Value { written: Option<String>, output: String },
    Text(String),
    Spawned { proc: u64, pid: i64 },
    Chunk(Option<String>),
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
