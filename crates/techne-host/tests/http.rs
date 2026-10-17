//! The whole way: an HTTP request over TCP to a front world serving with
//! `lisp/http`, handed to a deployment's instance with `host-fetch`, and
//! its response back to the client.

use std::path::Path;

use techne_host::{Deployment, Host, Limits};
use techne_vm::vm::Vm;

#[test]
fn http_through_the_front_to_an_instance() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("app.scm");
    std::fs::write(
        &entry,
        r#"(provide fetch)
           (define (fetch request env ctx)
             (dict 'status 201
                   'headers (dict "x-path" (dict-ref request 'path))
                   'body (string-append "got " (utf8->string (dict-ref request 'body)))))"#,
    )
    .unwrap();
    let host = Host::new();
    host.publish("app", Deployment::load(&entry, Limits::default()).unwrap());

    let lisp = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lisp/http");
    let mut front = Vm::new();
    techne_net::install(&mut front).unwrap();
    host.install(&mut front);
    front
        .eval_source(&format!(
            r#"(require {:?} {:?})
               (define listener (tcp-listen 0))
               (spawn (lambda () (http-serve listener (lambda (request) (host-fetch "app" request)))))
               (define answer
                 (spawn (lambda ()
                          (let ((answer (call-with-tcp-connection "127.0.0.1" (cadr (tcp-local-address listener))
                                          (lambda (c)
                                            (tcp-write c "POST /notes HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Length: 2\r\n\r\nhi")
                                            (let loop ((acc ""))
                                              (let ((s (tcp-read c)))
                                                (if (eof-object? s) acc (loop (string-append acc s)))))))))
                            (tcp-close listener)
                            answer))))"#,
            lisp.join("http.scm").display().to_string(),
            lisp.join("server.scm").display().to_string()
        ))
        .unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(host.run(&mut front));
    let answer = front.eval_source("(task-join answer)").unwrap();
    let answer: String = front.get(&answer).unwrap();
    assert!(answer.starts_with("HTTP/1.1 201 Created\r\n"), "{answer}");
    assert!(answer.contains("\r\nx-path: /notes\r\n"), "{answer}");
    assert!(answer.ends_with("\r\n\r\ngot hi"), "{answer}");
}
