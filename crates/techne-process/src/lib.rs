//! Child processes for techne Lisp: the Stage 0B process probe.
//!
//! A process runs with pipes (separate stdout and stderr) or on a pty (one
//! output stream, a controlling terminal). Every wait happens in the calling
//! task only: reads, writes and `process-wait` are async natives, so other
//! tasks and the host keep running.
//!
//! - Output is pumped into a bounded queue per stream (`QUEUE_CHUNKS` chunks
//!   of at most `CHUNK_BYTES`). A slow reader fills it, the pump stops
//!   reading, and the child blocks on write: memory stays bounded.
//! - Each child leads its own process group; signals go to the group, and
//!   dropping the process (its Lisp object becoming garbage, or
//!   `process-kill`) kills the group. `call-with-process` kills it when the
//!   body returns, fails or its task is cancelled.
//! - Output arrives as strings. A multi-byte UTF-8 sequence split between two
//!   reads is joined; invalid bytes become U+FFFD.
//!
//! The I/O runs on a small tokio runtime owned by this crate. A remote node
//! would implement the same Lisp interface over a transport.
//!
//! ```scheme
//! (call-with-process "sh" '("-c" "echo hi; echo oops >&2")
//!   (lambda (p)
//!     (list (process-read p 'stdout) (process-read p 'stderr) (process-wait p))))
//! ;; => ("hi\n" "oops\n" 0)
//! ```

use std::{
    os::unix::process::ExitStatusExt,
    process::{ExitStatus, Stdio},
    sync::{Arc, Mutex, OnceLock},
};

use rustix::process::{Pid, Signal, kill_process_group};
use techne_vm::{
    api::{Foreign, IntoValue},
    value::Value,
    vm::{Error, Vm},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, watch},
};

/// Largest chunk one read returns.
pub const CHUNK_BYTES: usize = 16 * 1024;
/// Chunks buffered per output stream before the child is made to wait.
pub const QUEUE_CHUNKS: usize = 16;

fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("techne-process")
            .enable_all()
            .build()
            .expect("tokio runtime")
    })
}

type Output = Arc<tokio::sync::Mutex<mpsc::Receiver<String>>>;

/// A running (or finished) child process.
pub struct Process {
    pid: u32,
    stdout: Output,
    /// `None` on a pty, where stderr is the terminal too.
    stderr: Option<Output>,
    stdin: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
    pty: bool,
    status: watch::Receiver<Option<ExitStatus>>,
}

impl Drop for Process {
    fn drop(&mut self) {
        if self.status.borrow().is_none() {
            let _ = self.signal(Signal::KILL);
        }
    }
}

/// How a child ended: its exit code, or the signal that killed it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    Code(i32),
    Signal(i32),
}

impl From<ExitStatus> for Exit {
    fn from(s: ExitStatus) -> Exit {
        match (s.code(), s.signal()) {
            (Some(c), _) => Exit::Code(c),
            (None, Some(sig)) => Exit::Signal(sig),
            (None, None) => Exit::Code(-1),
        }
    }
}

impl IntoValue for Exit {
    /// An exit code, or `(signal N)`.
    fn into_value(self, vm: &mut Vm) -> Result<Value, Error> {
        match self {
            Exit::Code(c) => (c as i64).into_value(vm),
            Exit::Signal(s) => {
                let items = [Value::symbol(techne_vm::reader::intern("signal")), Value::int_unchecked(s as i64)];
                Ok(vm.make_list(&items))
            }
        }
    }
}

/// A chunk of output, or the end of the stream.
enum Chunk {
    Data(String),
    Eof,
}

impl IntoValue for Chunk {
    fn into_value(self, vm: &mut Vm) -> Result<Value, Error> {
        match self {
            Chunk::Data(s) => s.into_value(vm),
            Chunk::Eof => Ok(Value::EOF),
        }
    }
}

/// Bytes at the end of `data` that start an incomplete UTF-8 sequence.
fn incomplete_tail(data: &[u8]) -> usize {
    for back in 1..=3.min(data.len()) {
        let b = data[data.len() - back];
        if b & 0xC0 != 0x80 {
            // A lead byte: how long its sequence is.
            let len = match b {
                b if b >= 0xF0 => 4,
                b if b >= 0xE0 => 3,
                b if b >= 0xC0 => 2,
                _ => 1,
            };
            return if len > back { back } else { 0 };
        }
    }
    0
}

/// Read `reader` into `tx` until EOF (or an error, e.g. EIO from a pty whose
/// child has gone), waiting whenever the queue is full.
async fn pump(mut reader: impl AsyncRead + Unpin, tx: mpsc::Sender<String>) {
    let mut carry = Vec::new();
    let mut buf = vec![0u8; CHUNK_BYTES];
    loop {
        let n = match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        carry.extend_from_slice(&buf[..n]);
        let keep = incomplete_tail(&carry);
        let tail = carry.split_off(carry.len() - keep);
        let text = String::from_utf8_lossy(&carry).into_owned();
        carry = tail;
        if tx.send(text).await.is_err() {
            return;
        }
    }
    if !carry.is_empty() {
        let _ = tx.send(String::from_utf8_lossy(&carry).into_owned()).await;
    }
}

/// Write what arrives on `rx` to `writer`; closing the channel closes the
/// child's input. On a pty it sends end-of-file instead, and keeps the
/// terminal open until the child exits: closing the last handle to the
/// master hangs up the session, and the child would die of SIGHUP when it
/// closes its own end before exiting (as `cat` does).
async fn feed(mut writer: impl AsyncWrite + Unpin, mut rx: mpsc::Receiver<Vec<u8>>, exit: Option<watch::Receiver<Option<ExitStatus>>>) {
    while let Some(data) = rx.recv().await {
        if writer.write_all(&data).await.is_err() {
            break;
        }
    }
    match exit {
        Some(mut exit) => {
            let _ = writer.write_all(b"\x04").await;
            let _ = exit.wait_for(|s| s.is_some()).await;
        }
        None => {
            let _ = writer.shutdown().await;
        }
    }
}

fn output() -> (mpsc::Sender<String>, Output) {
    let (tx, rx) = mpsc::channel(QUEUE_CHUNKS);
    (tx, Arc::new(tokio::sync::Mutex::new(rx)))
}

impl Process {
    /// Start `program` with `args`, on a pty (24×80) or with pipes.
    pub fn spawn(program: &str, args: &[String], pty: bool) -> std::io::Result<Process> {
        let rt = runtime();
        let _enter = rt.enter();
        let (stdin_tx, stdin_rx) = mpsc::channel::<Vec<u8>>(QUEUE_CHUNKS);
        let (out_tx, stdout) = output();
        let (status_tx, status) = watch::channel(None);
        let (mut child, stderr) = if pty {
            let (terminal, pts) = pty_process::open().map_err(std::io::Error::other)?;
            terminal.resize(pty_process::Size::new(24, 80)).map_err(std::io::Error::other)?;
            let child = pty_process::Command::new(program).args(args).spawn(pts).map_err(std::io::Error::other)?;
            let (read, write) = terminal.into_split();
            rt.spawn(pump(read, out_tx));
            rt.spawn(feed(write, stdin_rx, Some(status.clone())));
            (child, None)
        } else {
            let mut child = tokio::process::Command::new(program)
                .args(args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .process_group(0)
                .spawn()?;
            let (err_tx, stderr) = output();
            rt.spawn(pump(child.stdout.take().unwrap(), out_tx));
            rt.spawn(pump(child.stderr.take().unwrap(), err_tx));
            rt.spawn(feed(child.stdin.take().unwrap(), stdin_rx, None));
            (child, Some(stderr))
        };
        let pid = child.id().unwrap_or(0);
        rt.spawn(async move {
            if let Ok(s) = child.wait().await {
                let _ = status_tx.send(Some(s));
            }
        });
        Ok(Process { pid, stdout, stderr, stdin: Mutex::new(Some(stdin_tx)), pty, status })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Signal the child's process group.
    pub fn signal(&self, sig: Signal) -> std::io::Result<()> {
        let pid = Pid::from_raw(self.pid as i32).ok_or_else(|| std::io::Error::other("no pid"))?;
        Ok(kill_process_group(pid, sig)?)
    }

    fn stream(&self, which: &str) -> Result<Output, String> {
        match which {
            "stdout" | "output" => Ok(self.stdout.clone()),
            "stderr" if self.pty => Err("a pty process has one output stream; read 'stdout".into()),
            "stderr" => Ok(self.stderr.clone().unwrap()),
            _ => Err(format!("unknown stream {which}; expected stdout or stderr")),
        }
    }

    fn input(&self) -> Option<mpsc::Sender<Vec<u8>>> {
        self.stdin.lock().unwrap().clone()
    }

    fn close_input(&self) {
        self.stdin.lock().unwrap().take();
    }

    /// Wait for the child to exit.
    pub async fn wait(&self) -> Exit {
        let mut status = self.status.clone();
        let s = *status.wait_for(|s| s.is_some()).await.expect("status sender lives until exit");
        s.expect("waited for Some").into()
    }
}

fn signal_named(name: &str) -> Result<Signal, String> {
    Ok(match name {
        "int" | "interrupt" => Signal::INT,
        "term" | "terminate" => Signal::TERM,
        "kill" => Signal::KILL,
        "hup" | "hangup" => Signal::HUP,
        "stop" => Signal::STOP,
        "cont" | "continue" => Signal::CONT,
        _ => return Err(format!("unknown signal {name}")),
    })
}

/// Scheme wrappers over the natives.
const PRELUDE: &str = r#"
(define (process-spawn program args #:pty [pty #f])
  "Start PROGRAM with the list of strings ARGS, with pipes or (#:pty #t) on a terminal."
  (%process-spawn program args pty))

(define (call-with-process program args f #:pty [pty #f])
  "Call F with a new process; the process is killed when F returns, fails or its task is cancelled."
  (let ((p (process-spawn program args #:pty pty)))
    (dynamic-wind (lambda () #f) (lambda () (f p)) (lambda () (process-kill p)))))

(define (process-read-all p stream)
  "Everything STREAM ('stdout or 'stderr) of P outputs until end of file."
  (let loop ((chunks '()))
    (let ((c (process-read p stream)))
      (if (eof-object? c) (apply string-append (reverse chunks)) (loop (cons c chunks))))))
"#;

fn get_process(vm: &mut Vm, v: Value) -> Result<Foreign<Process>, Error> {
    vm.get(v)
}

fn symbol(vm: &mut Vm, v: Value) -> Result<String, Error> {
    if v.is_symbol() {
        Ok(techne_vm::reader::symbol_name(v.as_symbol()).to_string())
    } else {
        vm.get(v)
    }
}

/// Define the process procedures in `vm`.
pub fn install(vm: &mut Vm) -> Result<(), Error> {
    vm.name_foreign_type::<Process>("process");
    vm.register_fn("%process-spawn", |program: String, args: Vec<String>, pty: bool| -> Result<Foreign<Process>, String> {
        Process::spawn(&program, &args, pty).map(Foreign::new).map_err(|e| format!("process-spawn: {program}: {e}"))
    });
    vm.register_fn("process-pid", |p: Foreign<Process>| p.pid() as i64);
    vm.register_fn("process-close-input", |p: Foreign<Process>| p.close_input());
    vm.register_fn_vm("process-signal", |vm: &mut Vm, p: Foreign<Process>, sig: Value| -> Result<(), String> {
        let name = symbol(vm, sig).map_err(|e| e.msg)?;
        p.signal(signal_named(&name)?).map_err(|e| format!("process-signal: {e}"))
    });
    vm.register_fn("process-kill", |p: Foreign<Process>| {
        if p.status.borrow().is_none() {
            let _ = p.signal(Signal::KILL);
        }
    });
    vm.register_fn("process-exited?", |p: Foreign<Process>| p.status.borrow().is_some());
    vm.register_async("process-read", 2, |vm: &mut Vm, args: &[Value]| {
        let stream = get_process(vm, args[0]).and_then(|p| {
            let which = symbol(vm, args[1])?;
            p.stream(&which).map_err(Error::new)
        });
        async move {
            let stream = stream.map_err(|e| e.msg)?;
            let mut rx = stream.lock().await;
            Ok::<_, String>(match rx.recv().await {
                Some(s) => Chunk::Data(s),
                None => Chunk::Eof,
            })
        }
    });
    vm.register_async("process-write", 2, |vm: &mut Vm, args: &[Value]| {
        let target = get_process(vm, args[0]).and_then(|p| Ok((p.input(), vm.get::<String>(args[1])?)));
        async move {
            let (input, text) = target.map_err(|e| e.msg)?;
            let input = input.ok_or("process-write: input is closed")?;
            input.send(text.into_bytes()).await.map_err(|_| "process-write: the process closed its input".to_string())
        }
    });
    vm.register_async("process-wait", 1, |vm: &mut Vm, args: &[Value]| {
        let p = get_process(vm, args[0]);
        async move {
            let p = p.map_err(|e| e.msg)?;
            Ok::<_, String>(p.wait().await)
        }
    });
    vm.eval_source(PRELUDE).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::incomplete_tail;

    #[test]
    fn utf8_tails() {
        let s = "aé€😀".as_bytes();
        assert_eq!(incomplete_tail(s), 0);
        assert_eq!(incomplete_tail(&s[..s.len() - 1]), 3);
        assert_eq!(incomplete_tail(&s[..s.len() - 2]), 2);
        assert_eq!(incomplete_tail(&s[..s.len() - 3]), 1);
        assert_eq!(incomplete_tail(&s[..2]), 1);
        assert_eq!(incomplete_tail(b"abc"), 0);
    }
}
