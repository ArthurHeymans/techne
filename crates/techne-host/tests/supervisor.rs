//! A supervisor serving a deployment with a worker process: answered as a
//! host answers, bodies streaming both ways, a worker a native holds past
//! a kill replaced and the next request served, a worker that lies
//! killed.

use std::{
    cell::RefCell,
    path::Path,
    rc::Rc,
    time::{Duration, Instant},
};

use techne_host::{
    body::{self, Reader},
    supervisor::{Options, Sandboxing, Supervisor},
};
use techne_vm::data::Data;

const APP: &str = r#"
(provide fetch)
(define (fetch request env ctx)
  (let ((path (dict-ref request 'path)))
    (cond ((string=? path "/spin") (%spin))
          ((string=? path "/echo")
           (let ((body (dict-ref request 'body)))
             (dict 'body (lambda (write)
                           (let loop ()
                             (let ((b (body)))
                               (unless (eof-object? b) (write b) (loop))))))))
          ((string=? path "/count")
           (dict 'body (lambda (write)
                         (let loop ((i 0))
                           (when (< i 50) (write (make-string 10000 #\x)) (loop (+ i 1)))))))
          ;; Its first chunk sent before the native holds the thread.
          ((string=? path "/stream-spin") (dict 'body (lambda (write) (write "a") (sleep 100) (%spin))))
          ((string=? path "/file") (open-input-file "/etc/hostname"))
          ((string=? path "/sum") (number->string (let loop ((i 0) (s 0)) (if (= i 100000) s (loop (+ i 1) (+ s i))))))
          (else (dict 'status 200 'body "hello from a worker")))))
"#;

fn options(env: &[(&str, &str)]) -> Options {
    Options {
        program: env!("CARGO_BIN_EXE_techne-worker").into(),
        args: Vec::new(),
        env: env.iter().map(|(k, v)| (k.into(), v.into())).collect(),
        sandboxing: Sandboxing::IfAvailable,
        cgroup: techne_sandbox::cgroup::Limits { memory: 512 << 20, pids: 256, cpu: None },
        wall: Duration::from_secs(10),
        dev: false,
    }
}

fn request(path: &str) -> Data {
    Data::Dict(vec![(Data::Symbol("method".into()), Data::String("GET".into())), (Data::Symbol("path".into()), Data::String(path.into()))])
}

/// The status and body of `response`.
fn answer(response: &Data) -> (i64, String) {
    let entries = match response {
        Data::Dict(entries) => entries,
        Data::String(body) => return (200, body.clone()),
        _ => panic!("{response:?}"),
    };
    let get = |k: &str| entries.iter().find(|(key, _)| matches!(key, Data::Symbol(s) if s == k)).map(|(_, v)| v);
    let status = match get("status") {
        Some(Data::Int(s)) => *s,
        _ => 200,
    };
    let body = match get("body") {
        Some(Data::String(b)) => b.clone(),
        other => format!("{other:?}"),
    };
    (status, body)
}

/// Run `test` with a supervisor started with `options`, the deployment
/// APP published as "app" from a directory of its own.
fn supervised(options: Options, test: impl AsyncFnOnce(Supervisor, Rc<RefCell<Vec<String>>>)) {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("app.scm");
    std::fs::write(&entry, APP).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async {
        let s = Supervisor::start(options).await.unwrap();
        let failures = Rc::new(RefCell::new(Vec::new()));
        let seen = failures.clone();
        s.on_failure(move |f| seen.borrow_mut().push(format!("{}: {} {}", f.deployment, f.status, f.message)));
        s.deploy("app", Path::new(&entry)).await.unwrap();
        test(s, failures).await;
    });
}

#[test]
fn a_worker_answers_as_a_host_does() {
    // Compiled code too, in a sandbox.
    supervised(options(&[("TECHNE_JIT", "1"), ("TECHNE_JIT_SYNC", "1")]), async |s, failures| {
        let r = s.fetch("app", request("/"), None).await.unwrap();
        assert_eq!(answer(&r.head), (200, "hello from a worker".into()));
        let r = s.fetch("app", request("/sum"), None).await.unwrap();
        assert_eq!(answer(&r.head), (200, "4999950000".into()));
        // An instance reaches no files, sandbox or not.
        let r = s.fetch("app", request("/file"), None).await.unwrap();
        assert_eq!(answer(&r.head).0, 500);
        let r = s.fetch("nothing", request("/"), None).await.unwrap();
        assert_eq!(answer(&r.head).0, 404);
        assert_eq!(failures.borrow().len(), 2, "{:?}", failures.borrow());
    });
}

#[test]
fn a_worker_a_native_holds_is_replaced() {
    supervised(options(&[("TECHNE_TEST_SPIN", "1")]), async |s, failures| {
        let started = Instant::now();
        let r = s.fetch("app", request("/spin"), None).await.unwrap();
        assert_eq!(answer(&r.head).0, 500);
        // Its CPU bound, then a second's grace.
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
        assert!(failures.borrow().iter().any(|f| f.contains("past a kill")), "{:?}", failures.borrow());
        served_again(&s).await;
    });
}

/// Wait for the next worker to be given the deployment and serve it.
async fn served_again(s: &Supervisor) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let r = s.fetch("app", request("/"), None).await.unwrap();
        if answer(&r.head).0 == 200 {
            break;
        }
        assert!(Instant::now() < deadline, "not served again: {:?}", r.head);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[test]
fn a_worker_that_answers_what_it_was_not_asked_is_killed() {
    let options = options(&[("TECHNE_TEST_LIE", "1")]);
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async {
        let s = Supervisor::start(options).await.unwrap();
        let failures = Rc::new(RefCell::new(Vec::new()));
        let seen = failures.clone();
        s.on_failure(move |f| seen.borrow_mut().push(format!("{}: {} {}", f.deployment, f.status, f.message)));
        let r = s.fetch("app", request("/"), None).await.unwrap();
        assert_eq!(answer(&r.head).0, 500);
        assert!(failures.borrow().iter().any(|f| f.contains("misbehaved")), "{:?}", failures.borrow());
    });
}

/// All of `body`, or why it failed.
async fn read_all(mut body: Reader) -> Result<Vec<u8>, String> {
    let mut all = Vec::new();
    while let Some(chunk) = body.read().await? {
        all.extend(chunk);
    }
    Ok(all)
}

#[test]
fn bodies_stream_through_a_worker() {
    supervised(options(&[]), async |s, failures| {
        // A request's body, written as it is read, back as the response's.
        let (writer, reader) = body::channel();
        let r = s.fetch("app", request("/echo"), Some(reader));
        let sent: Vec<u8> = (0..100_000u32).map(|i| i as u8).collect();
        let sending = sent.clone();
        tokio::task::spawn_local(async move {
            for piece in sending.chunks(7000) {
                assert!(writer.write(piece).await);
            }
            writer.end(Ok(()));
        });
        let r = r.await.unwrap();
        assert_eq!(read_all(r.body.expect("a body streaming")).await.unwrap(), sent);
        // More than the credits and queues hold, as it is read.
        let r = s.fetch("app", request("/count"), None).await.unwrap();
        assert_eq!(read_all(r.body.unwrap()).await.unwrap().len(), 500_000);
        assert!(failures.borrow().is_empty(), "{:?}", failures.borrow());
    });
}

#[test]
fn a_body_streaming_from_a_worker_ended_is_cut_short() {
    supervised(options(&[("TECHNE_TEST_SPIN", "1")]), async |s, _| {
        let r = s.fetch("app", request("/stream-spin"), None).await.unwrap();
        let mut body = r.body.unwrap();
        assert_eq!(body.read().await.unwrap().as_deref(), Some(&b"a"[..]));
        assert!(body.read().await.is_err(), "cut short");
        served_again(&s).await;
    });
}

#[test]
fn a_body_no_one_reads_is_cut_past_the_wall_time() {
    let options = Options { wall: Duration::from_secs(1), ..options(&[]) };
    supervised(options, async |s, failures| {
        let r = s.fetch("app", request("/count"), None).await.unwrap();
        let mut body = r.body.unwrap();
        // Unread past the wall time and its grace: cut short there, the
        // worker, done with it, kept.
        tokio::time::sleep(Duration::from_secs(4)).await;
        let read = async {
            loop {
                match body.read().await {
                    Ok(Some(_)) => {}
                    other => return other,
                }
            }
        };
        assert!(read.await.is_err(), "cut short");
        let r = s.fetch("app", request("/"), None).await.unwrap();
        assert_eq!(answer(&r.head), (200, "hello from a worker".into()));
        let failures = failures.borrow();
        assert!(failures.iter().any(|f| f.contains("cut short past its wall time")), "{failures:?}");
        assert!(!failures.iter().any(|f| f.contains("did not answer")), "{failures:?}");
    });
}
