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
    supervisor::{Bindings, Options, ResourceConfinement, Sandboxing, Supervisor},
};
use techne_vm::data::Data;

const APP: &str = r#"
(provide fetch)
(define (fetch request env ctx)
  (let ((path (dict-ref request 'path)))
    (cond ((string=? path "/spin") (%spin))
          ((string=? path "/fetch")
           (guard (e ((error-object? e) (error-object-message e)))
             (utf8->string (dict-ref (fetch-url (utf8->string (dict-ref request 'body))) 'body))))
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
        resources: ResourceConfinement::IfAvailable,
        cgroup: techne_sandbox::cgroup::Limits { memory: 512 << 20, pids: 256, cpu: None },
        wall: Duration::from_secs(10),
        dev: false,
        egress: Default::default(),
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

/// What keeps under a key, or reads it, through whatever its `env` has as
/// `store`, or through a binding of its own making.
const KEEPING: &str = r#"
(provide fetch)
(define (fetch request env ctx)
  (let ((path (dict-ref request 'path))
        (store (dict-ref env 'store #f)))
    (guard (e (#t (condition/report-string e)))
      (cond ((string=? path "/put") (kv-put! store "k" (dict-ref env 'who)) "kept")
            ((string=? path "/get") (kv-get store "k" "nothing"))
            ((string=? path "/forged") (kv-get (dict 'kind 'kv 'name "theirs") "k" "nothing"))
            ((string=? path "/late-get") (sleep 300) (kv-get store "k" "nothing"))
            ;; Data whose written form does not read back.
            ((string=? path "/odd")
             (let ((odd (list 'a-symbol (string->keyword "a b") 1.5 (bytevector 0 255) (dict "é" #t))))
               (kv-put! store "odd" odd)
               (if (equal? odd (kv-get store "odd")) "same" "changed")))

            (else "?")))))
"#;

/// Deployments reach the namespaces their own bindings stand for, by the
/// names they have for them, and no other, whatever they name; without a
/// binding, nothing.
#[test]
fn a_deployment_reaches_its_own_namespaces() {
    let dir = tempfile::tempdir().unwrap();
    let entry = |name: &str| {
        let d = dir.path().join(name);
        std::fs::create_dir(&d).unwrap();
        std::fs::write(d.join("app.scm"), KEEPING).unwrap();
        d.join("app.scm")
    };
    let bindings = |who: &str, namespace: &str| Bindings {
        env: Data::Dict(vec![
            (Data::Symbol("who".into()), Data::String(who.into())),
            (
                Data::Symbol("store".into()),
                Data::Dict(vec![
                    (Data::Symbol("kind".into()), Data::Symbol("kv".into())),
                    (Data::Symbol("name".into()), Data::String("mine".into())),
                ]),
            ),
        ]),
        stores: [("mine".to_string(), namespace.to_string())].into(),
    };
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async {
        let s = Supervisor::start(options(&[])).await.unwrap();
        s.on_failure(|_| {});
        let mut front = techne_vm::vm::Vm::new();
        s.install_store(&mut front);
        let kv = techne_vm::library::lisp_dir().join("cloud/kv.scm").display().to_string();
        let kept = dir.path().join("kept").display().to_string();
        front.eval_source(&format!("(require {kv:?}) (spawn (lambda () (kv-serve (make-kv-store {kept:?}))))")).unwrap();
        let driver = s.clone();
        tokio::task::spawn_local(async move {
            let _ = driver.run(&mut front).await;
        });
        s.deploy_with("a", &entry("a"), bindings("a", "ns-a")).await.unwrap();
        s.deploy_with("b", &entry("b"), bindings("b", "ns-b")).await.unwrap();
        s.deploy("bare", &entry("bare")).await.unwrap();
        let ask = async |name: &str, path: &str| answer(&s.fetch(name, request(path), None).await.unwrap().head).1;
        assert_eq!(ask("a", "/put").await, "kept");
        // The same binding's name, another namespace.
        assert_eq!(ask("b", "/get").await, "nothing");
        assert_eq!(ask("b", "/put").await, "kept");
        assert_eq!(ask("a", "/get").await, "a");
        assert_eq!(ask("b", "/get").await, "b");
        assert!(dir.path().join("kept/ns-a.kv").is_file() && dir.path().join("kept/ns-b.kv").is_file());
        // A binding it was not given, and a deployment given none.
        assert!(ask("a", "/forged").await.contains("no KV binding theirs"));
        assert!(ask("bare", "/forged").await.contains("no KV binding theirs"));
        assert!(ask("bare", "/get").await.contains("Not a KV binding"));
        assert_eq!(ask("a", "/odd").await, "same");
        // Deployed again with another namespace: that one from then on,
        // and for a request sent before, the one it had then.
        let before = s.fetch("a", request("/late-get"), None);
        s.deploy_with("a", &entry("a2"), bindings("a", "ns-b")).await.unwrap();
        assert_eq!(ask("a", "/get").await, "b");
        assert_eq!(answer(&before.await.unwrap().head).1, "a");
    });
}

/// What answers with what its `env` has as `who`, at once or after a
/// while.
const ROUTED: &str = r#"
(provide fetch)
(define (fetch request env ctx)
  (when (string=? (dict-ref request 'path) "/slow") (sleep 300))
  (dict-ref env 'who))
"#;

/// A deployment is prepared, which changes nothing served, then
/// published, if its name is still routed where the publisher saw it; a
/// request sent before finishes on the deployment it was sent for, which
/// is then retired; an earlier one is published again by preparing it
/// again; one that does not load changes nothing.
#[test]
fn deployments_are_prepared_published_and_rolled_back() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("app.scm");
    std::fs::write(&entry, ROUTED).unwrap();
    let who = |who: &str| Bindings { env: Data::Dict(vec![(Data::Symbol("who".into()), Data::String(who.into()))]), ..Bindings::default() };
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async {
        let s = Supervisor::start(options(&[])).await.unwrap();
        s.on_failure(|_| {});
        let ask = async |path: &str| answer(&s.fetch("app", request(path), None).await.unwrap().head);
        let one = s.prepare("app", &entry, who("one")).await.unwrap();
        // Prepared: not served yet.
        assert_eq!(ask("/").await.0, 404);
        assert_eq!(s.route("app"), None);
        assert_eq!(s.publish("app", &one.id, Some(None)).unwrap(), None);
        assert_eq!(ask("/").await, (200, "one".into()));
        // The same prepared again is the one there is.
        let same = s.prepare("app", &entry, who("one")).await.unwrap();
        assert_eq!((same.id.as_str(), same.output.as_str()), (one.id.as_str(), ""));
        let two = s.prepare("app", &entry, who("two")).await.unwrap();
        assert_ne!(two.id, one.id);
        assert_eq!(s.prepared("app").len(), 2);
        assert_eq!(ask("/").await, (200, "one".into()));
        let before = s.fetch("app", request("/slow"), None);
        // Published only where the publisher saw the name routed.
        assert!(s.publish("app", &two.id, Some(None)).is_err());
        assert!(s.publish("app", &two.id, Some(Some("0000000000000000"))).is_err());
        assert_eq!(s.route("app").as_deref(), Some(one.id.as_str()));
        assert_eq!(s.publish("app", &two.id, Some(Some(&one.id))).unwrap().as_deref(), Some(one.id.as_str()));
        assert_eq!(ask("/").await, (200, "two".into()));
        // Sent before: served by the one it was sent for.
        assert_eq!(answer(&before.await.unwrap().head), (200, "one".into()));
        // Which is retired, its requests done.
        let start = Instant::now();
        while s.prepared("app") != [two.id.clone()] {
            assert!(start.elapsed() < Duration::from_secs(5), "{:?}", s.prepared("app"));
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(s.publish("app", &one.id, None).is_err());
        // Rolled back: prepared again, and published.
        let again = s.prepare("app", &entry, who("one")).await.unwrap();
        assert_eq!(again.id, one.id);
        s.publish("app", &again.id, Some(Some(&two.id))).unwrap();
        assert_eq!(ask("/").await, (200, "one".into()));
        // One that does not load changes nothing.
        let broken = dir.path().join("broken");
        std::fs::create_dir(&broken).unwrap();
        std::fs::write(broken.join("app.scm"), "(provide fetch) (car 1)").unwrap();
        assert!(s.prepare("app", &broken.join("app.scm"), who("three")).await.is_err());
        assert_eq!(ask("/").await, (200, "one".into()));
        assert_eq!(s.route("app").as_deref(), Some(one.id.as_str()));
        assert!(s.prepare("a@b", &entry, who("x")).await.is_err());
        // Prepared twice at once: loaded once, for both.
        let (a, b) = tokio::join!(s.prepare("app", &entry, who("four")), s.prepare("app", &entry, who("four")));
        assert_eq!(a.unwrap().id, b.unwrap().id);
        assert_eq!(s.prepared("app").len(), 2);
        // A few prepared and not published are kept, the oldest going
        // once another has loaded; one that does not load costs none.
        for n in 0..6 {
            s.prepare("app", &entry, who(&format!("more {n}"))).await.unwrap();
        }
        let kept = s.prepared("app");
        assert_eq!(kept.len(), 5, "{kept:?}");
        assert!(s.prepare("app", &broken.join("app.scm"), who("five")).await.is_err());
        assert_eq!(s.prepared("app"), kept);
        assert_eq!(ask("/").await, (200, "one".into()));
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
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("app.scm");
        std::fs::write(&entry, APP).unwrap();
        s.deploy("app", &entry).await.unwrap();
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

#[test]
fn a_request_fetches_through_the_supervisor() {
    // A server on loopback the test's policy lets through, as the default
    // policy does not.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        for mut c in listener.incoming().flatten() {
            let mut buf = [0; 4096];
            let _ = c.read(&mut buf);
            let _ = c.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 7\r\nconnection: close\r\n\r\nfetched");
        }
    });
    let mut options = options(&[]);
    options.egress =
        techne_host::egress::Policy { allow: |ip| ip == std::net::IpAddr::from([127, 0, 0, 1]), ports: vec![port], ..Default::default() };
    supervised(options, async |s, _| {
        let fetch = |url: String| {
            let Data::Dict(mut d) = request("/fetch") else { unreachable!() };
            d.push((Data::Symbol("body".into()), Data::Bytes(url.into_bytes())));
            s.fetch("app", Data::Dict(d), None)
        };
        let r = fetch(format!("http://127.0.0.1:{port}/")).await.unwrap();
        assert_eq!(answer(&r.head), (200, "fetched".into()));
        let r = fetch(format!("http://10.0.0.1:{port}/")).await.unwrap();
        assert!(answer(&r.head).1.contains("may not reach"), "{:?}", r.head);
    });
}

#[test]
fn resources_fail_closed_independently_of_sandbox_entry() {
    use techne_host::supervisor::Confinement;
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    tokio::task::LocalSet::new().block_on(&rt, async {
        // Sharing our cgroup with another process prevents adopting it,
        // even when this test runs in a delegated scope of its own.
        let mut companion = tokio::process::Command::new("sleep").arg("30").kill_on_drop(true).spawn().unwrap();
        let strict = Options { sandboxing: Sandboxing::Off, resources: ResourceConfinement::Required, ..options(&[]) };
        let error = match Supervisor::start(strict).await {
            Err(e) => e,
            Ok(_) => panic!("started without required resource confinement"),
        };
        assert!(error.to_string().contains("require cgroup resource confinement"), "{error}");
        let fallback = Options { sandboxing: Sandboxing::Off, resources: ResourceConfinement::IfAvailable, ..options(&[]) };
        let s = Supervisor::start(fallback).await.unwrap();
        assert_eq!(s.confinement(), Some(Confinement { sandboxed: false, cgroup_bounded: false }));
        companion.kill().await.unwrap();
        companion.wait().await.unwrap();
    });
}
