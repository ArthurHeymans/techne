//! What serving from a worker process costs: a worker's cold start (spawned,
//! sandboxed, ready, then given a deployment and answering its first
//! request), a request's latency through it beside a host in this
//! process, requests a second with several at once, and a streamed body's
//! throughput. Wall time, on this machine, as it is loaded.
//!
//! `cargo build --release -p techne-host --bins --examples &&
//! target/release/examples/supervised` (it runs `techne-worker` from
//! beside it, or TECHNE_WORKER).

use std::{
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

use techne_host::{
    Deployment, Host, Limits, Response,
    body::Reader,
    supervisor::{Options, Sandboxing, Supervisor},
};
use techne_vm::data::Data;
use tokio::sync::oneshot;

const APP: &str = r#"
(provide fetch)
(define (fetch request env ctx)
  (let ((path (dict-ref request 'path)))
    (cond ((string=? path "/stream")
           (dict 'body (lambda (write)
                         (let ((chunk (make-bytevector 65536 120)))
                           (let loop ((i 0))
                             (when (< i 256) (write chunk) (loop (+ i 1))))))))
          (else "ok"))))
"#;

/// Bytes `/stream` sends.
const STREAMED: usize = 256 * 65536;

fn worker() -> PathBuf {
    std::env::var_os("TECHNE_WORKER").map(PathBuf::from).unwrap_or_else(|| {
        let exe = std::env::current_exe().unwrap();
        exe.parent().and_then(Path::parent).unwrap().join("techne-worker")
    })
}

fn options() -> Options {
    Options {
        program: worker(),
        args: Vec::new(),
        env: Vec::new(),
        sandboxing: Sandboxing::IfAvailable,
        cgroup: techne_sandbox::cgroup::Limits { memory: 512 << 20, pids: 256, cpu: None },
        wall: Duration::from_secs(30),
        dev: false,
    }
}

fn request(path: &str) -> Data {
    Data::Dict(vec![(Data::Symbol("method".into()), Data::String("GET".into())), (Data::Symbol("path".into()), Data::String(path.into()))])
}

/// What answers requests: a supervisor's worker, or a host here.
type Fetch = Rc<dyn Fn(&str, Data, Option<Reader>) -> oneshot::Receiver<Response>>;

fn local<F: std::future::Future>(f: F) -> F::Output {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    tokio::task::LocalSet::new().block_on(&rt, f)
}

/// The median and 99th percentile of `samples`.
fn percentiles(mut samples: Vec<Duration>) -> (Duration, Duration) {
    samples.sort();
    (samples[samples.len() / 2], samples[samples.len() * 99 / 100])
}

/// Latency of one request at a time, requests a second with `at_once`
/// asking, and a streamed body's throughput.
async fn measure(what: &str, fetch: Fetch) {
    for _ in 0..200 {
        fetch("app", request("/"), None).await.unwrap();
    }
    let mut samples = Vec::new();
    for _ in 0..2000 {
        let t = Instant::now();
        fetch("app", request("/"), None).await.unwrap();
        samples.push(t.elapsed());
    }
    let (p50, p99) = percentiles(samples);
    println!("{what:<12} one at a time: p50 {p50:>9.1?}  p99 {p99:>9.1?}");
    for at_once in [4, 16] {
        let until = Instant::now() + Duration::from_secs(2);
        let tasks: Vec<_> = (0..at_once)
            .map(|_| {
                let fetch = fetch.clone();
                tokio::task::spawn_local(async move {
                    let mut n = 0u64;
                    while Instant::now() < until {
                        fetch("app", request("/"), None).await.unwrap();
                        n += 1;
                    }
                    n
                })
            })
            .collect();
        let mut n = 0;
        for t in tasks {
            n += t.await.unwrap();
        }
        println!("{what:<12} {at_once:>2} at once:    {:>8.0} requests/s", n as f64 / 2.0);
    }
    // Warmed up once, as requests are.
    let stream = async || {
        let t = Instant::now();
        let mut body = fetch("app", request("/stream"), None).await.unwrap().body.expect("a streamed body");
        let mut got = 0;
        while let Some(chunk) = body.read().await.unwrap() {
            got += chunk.len();
        }
        assert_eq!(got, STREAMED);
        t.elapsed()
    };
    stream().await;
    let mut took = Duration::ZERO;
    for _ in 0..8 {
        took += stream().await;
    }
    println!("{what:<12} streaming:     {:>8.0} MiB/s", 8.0 * STREAMED as f64 / (1 << 20) as f64 / took.as_secs_f64());
}

fn main() {
    techne_vm::alloc::contain_out_of_memory();
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("app.scm");
    std::fs::write(&entry, APP).unwrap();

    // Cold starts, each worker its own: ready, deployed, first answer.
    let mut starts = Vec::new();
    for _ in 0..5 {
        starts.push(local(async {
            let t = Instant::now();
            let s = Supervisor::start(options()).await.unwrap();
            let ready = t.elapsed();
            s.deploy("app", &entry).await.unwrap();
            let deployed = t.elapsed();
            s.fetch("app", request("/"), None).await.unwrap();
            (ready, deployed, t.elapsed())
        }));
    }
    let median = |f: fn(&(Duration, Duration, Duration)) -> Duration| percentiles(starts.iter().map(f).collect()).0;
    println!(
        "cold start (median of {}): ready {:.1?}, deployed {:.1?}, first answer {:.1?}",
        starts.len(),
        median(|s| s.0),
        median(|s| s.1),
        median(|s| s.2)
    );

    local(async {
        let s = Supervisor::start(options()).await.unwrap();
        s.deploy("app", &entry).await.unwrap();
        measure("worker", Rc::new(move |name, request, body| s.fetch(name, request, body))).await;
    });
    local(async {
        let host = Host::new();
        host.publish("app", Deployment::load(&entry, Limits::default()).unwrap());
        let driver = host.clone();
        tokio::task::spawn_local(async move { driver.run(None).await });
        measure("in-process", Rc::new(move |name, request, body| host.fetch(name, request, body))).await;
    });
}
