//! A supervisor serving a deployment with a worker process: answered as a
//! host answers, a worker a native holds past a kill replaced and the
//! next request served, a worker that lies killed.

use std::{
    cell::RefCell,
    path::Path,
    rc::Rc,
    time::{Duration, Instant},
};

use techne_host::supervisor::{Options, Sandboxing, Supervisor};
use techne_vm::data::Data;

const APP: &str = r#"
(provide fetch)
(define (fetch request env ctx)
  (let ((path (dict-ref request 'path)))
    (cond ((string=? path "/spin") (%spin))
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
        let r = s.fetch("app", request("/")).await.unwrap();
        assert_eq!(answer(&r), (200, "hello from a worker".into()));
        let r = s.fetch("app", request("/sum")).await.unwrap();
        assert_eq!(answer(&r), (200, "4999950000".into()));
        // An instance reaches no files, sandbox or not.
        let r = s.fetch("app", request("/file")).await.unwrap();
        assert_eq!(answer(&r).0, 500);
        let r = s.fetch("nothing", request("/")).await.unwrap();
        assert_eq!(answer(&r).0, 404);
        assert_eq!(failures.borrow().len(), 2, "{:?}", failures.borrow());
    });
}

#[test]
fn a_worker_a_native_holds_is_replaced() {
    supervised(options(&[("TECHNE_TEST_SPIN", "1")]), async |s, failures| {
        let started = Instant::now();
        let r = s.fetch("app", request("/spin")).await.unwrap();
        assert_eq!(answer(&r).0, 500);
        // Its CPU bound, then a second's grace.
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
        assert!(failures.borrow().iter().any(|f| f.contains("past a kill")), "{:?}", failures.borrow());
        // The next worker is given the deployment: served again.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let r = s.fetch("app", request("/")).await.unwrap();
            if answer(&r).0 == 200 {
                break;
            }
            assert!(Instant::now() < deadline, "not served again: {r:?}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });
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
        let r = s.fetch("app", request("/")).await.unwrap();
        assert_eq!(answer(&r).0, 500);
        assert!(failures.borrow().iter().any(|f| f.contains("misbehaved")), "{:?}", failures.borrow());
    });
}
