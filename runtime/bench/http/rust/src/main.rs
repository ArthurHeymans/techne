//! The same minimal server as ../techne/server.scm, on tokio's
//! multi-thread runtime with THREADS workers.
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

const HELLO: &[u8] = b"HTTP/1.1 200 OK\r\nServer: rust\r\nContent-Type: text/plain\r\nContent-Length: 13\r\n\r\nHello, World!";

fn work(n: u64) -> usize {
    (0..n).map(|i| (i * i).to_string().len()).sum()
}

fn answer(request: &[u8], out: &mut Vec<u8>) {
    if request.starts_with(b"GET /work ") {
        let body = work(1000).to_string();
        out.extend_from_slice(
            format!("HTTP/1.1 200 OK\r\nServer: rust\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}", body.len())
                .as_bytes(),
        );
    } else {
        out.extend_from_slice(HELLO);
    }
}

async fn handle(mut s: TcpStream) -> std::io::Result<()> {
    let (mut buf, mut out) = (Vec::with_capacity(16 * 1024), Vec::new());
    let mut chunk = vec![0u8; 16 * 1024];
    loop {
        let n = s.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
        let mut start = 0;
        while let Some(end) = memchr::memmem::find(&buf[start..], b"\r\n\r\n") {
            answer(&buf[start..], &mut out);
            start += end + 4;
        }
        buf.drain(..start);
        if !out.is_empty() {
            s.write_all(&out).await?;
            out.clear();
        }
    }
}

fn main() {
    let threads = std::env::var("THREADS").map_or(1, |t| t.parse().unwrap());
    let port = std::env::args().nth(1).unwrap_or("8080".into());
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(threads).enable_all().build().unwrap();
    rt.block_on(async {
        let l = TcpListener::bind(format!("127.0.0.1:{port}")).await.unwrap();
        loop {
            let (s, _) = l.accept().await.unwrap();
            s.set_nodelay(true).unwrap();
            tokio::spawn(handle(s));
        }
    });
}
