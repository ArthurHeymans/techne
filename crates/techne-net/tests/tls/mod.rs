//! Local TLS tests. Fixtures are a self-signed localhost leaf and its
//! PUBLIC TEST KEY, valid 2020-2120, with no trust outside these tests.
//! Production tls-connect always uses webpki-roots, never this anchor.

use super::*;
use std::{
    io::{Read, Write},
    sync::mpsc,
    thread,
};

const CERT: &[u8] = include_bytes!("../fixtures/localhost.der");
const KEY: &[u8] = include_bytes!("../fixtures/localhost-key.der");

fn server() -> (u16, thread::JoinHandle<()>) {
    server_with_upgrade(false)
}

fn server_with_upgrade(upgrade: bool) -> (u16, thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let worker = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        socket.set_write_timeout(Some(Duration::from_secs(10))).unwrap();
        if upgrade {
            socket.write_all(b"plaintext\r\n").unwrap();
            let mut command = [0; 10];
            socket.read_exact(&mut command).unwrap();
            assert_eq!(&command, b"STARTTLS\r\n");
            socket.write_all(b"220 Begin TLS\r\n").unwrap();
        }
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![CERT.to_vec().into()], rustls::pki_types::PrivatePkcs8KeyDer::from(KEY.to_vec()).into())
            .unwrap();
        let connection = rustls::ServerConnection::new(Arc::new(config)).unwrap();
        let mut tls = rustls::StreamOwned::new(connection, socket);
        // Certificate rejection is an expected failure in verification tests.
        if tls.write_all(b"ready\n").is_err() || tls.flush().is_err() {
            return;
        }
        let mut buf = [0; 4096];
        loop {
            match tls.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => {
                    if tls.write_all(&buf[..n]).is_err() || tls.flush().is_err() {
                        return;
                    }
                }
            }
        }
    });
    (port, worker)
}

fn vm(port: u16, trust_test_anchor: bool) -> Vm {
    let mut vm = Vm::new();
    install(&mut vm).unwrap();
    vm.eval_source(
        r#"
        (define (report f) (guard (e (#t (condition/report-string e))) (f)))
        (define (read-n c n)
          (let loop ((out (bytevector)))
            (if (= (bytevector-length out) n) (utf8->string out)
                (let ((b (tcp-read-bytes c)))
                  (if (eof-object? b) (error "early EOF")
                      (loop (bytevector-append out b)))))))
    "#,
    )
    .unwrap();
    if trust_test_anchor {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(CERT.to_vec().into()).unwrap();
        let config = Arc::new(
            rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        );
        // Only this test native can trust the fixture. Exercise the same
        // Connection, handshake and scope-ownership path as tls-connect.
        let connect_config = config.clone();
        vm.register_async("%test-tls-connect", 1, move |vm: &mut Vm, args: Args<'_>| {
            let host = args.get::<String>(vm, 0).map_err(|e| e.msg.clone());
            let config = connect_config.clone();
            entered(runtime().handle(), async move { tls_connect_with(host?, port, config).await.map(Foreign::new) })
        });
        vm.register_async("test-upgrade!", 2, move |vm: &mut Vm, args: Args<'_>| {
            let c = args.get::<Socket>(vm, 0).and_then(|s| s.connection("tls-upgrade!")).map_err(|e| e.msg.clone());
            let host = args.get::<String>(vm, 1).map_err(|e| e.msg.clone());
            let config = config.clone();
            entered(runtime().handle(), async move { tls_upgrade_with(&*c?, host?, config, "tls-upgrade!").await })
        });
        vm.eval_source(
            r#"(define (test-connect host)
                            (scope-own! (%test-tls-connect host) tcp-close tcp-closed?))"#,
        )
        .unwrap();
    } else {
        vm.eval_source(&format!("(define (test-connect host) (tls-connect host {port})) (define test-upgrade! tls-upgrade!)")).unwrap();
    }
    vm
}

fn run(vm: &mut Vm, body: &str) -> String {
    let interrupt = vm.interrupt_handle();
    let (done, watch) = mpsc::channel::<()>();
    let watchdog = thread::spawn(move || {
        if watch.recv_timeout(Duration::from_secs(15)).is_err() {
            interrupt.interrupt();
        }
    });
    let result = vm.eval_source(&format!("(task-join (spawn (lambda () {body})))"));
    done.send(()).unwrap();
    watchdog.join().unwrap();
    let result = result.unwrap_or_else(|e| panic!("{body}\n{e}"));
    vm.repr(&result)
}

#[test]
fn verified_tls_uses_the_existing_socket_api() {
    let (port, worker) = server();
    let mut vm = vm(port, true);
    let result = run(
        &mut vm,
        r#"
        (define c (test-connect "localhost"))
        (define greeting (read-n c 6))
        (define text (make-string 100000 #\x))
        (tcp-write c text)
        (define echoed (read-n c 100000))
        (define reader (spawn (lambda () (tcp-read c))))
        (sleep 20)
        (task-cancel reader)
        (define stopped (report (lambda () (task-join reader))))
        (tcp-write c "λ")
        (define after-cancel (read-n c 2))
        (tcp-close c)
        (list greeting (equal? text echoed) stopped after-cancel (tcp-closed? c))
    "#,
    );
    assert_eq!(result, r#"("ready\n" #t "task cancelled" "λ" #t)"#);
    worker.join().unwrap();
}

#[test]
fn tls_read_waiters_survive_another_readers_cancellation_and_close() {
    let (port, worker) = server();
    let mut vm = vm(port, true);
    let result = run(
        &mut vm,
        r#"
        (define c (test-connect "localhost"))
        (read-n c 6)
        (define first (spawn (lambda () (tcp-read c))))
        (sleep 20)
        (define second (spawn (lambda () (tcp-read c))))
        (sleep 20)
        (task-cancel second)
        (tcp-write c "one")
        (define survived (task-join first))
        (define readers (map (lambda (_) (spawn (lambda () (report (lambda () (tcp-read c)))))) (iota 3)))
        (sleep 20)
        (tcp-close c)
        (list survived (map task-join readers))
    "#,
    );
    assert_eq!(
        result,
        r#"("one" ("tcp-read: the connection is closed" "tcp-read: the connection is closed" "tcp-read: the connection is closed"))"#
    );
    worker.join().unwrap();
}

#[test]
fn scope_shutdown_closes_tls_and_wakes_reads() {
    let (port, worker) = server();
    let mut vm = vm(port, true);
    let result = run(
        &mut vm,
        r#"
        (define scope (make-scope 'tls-test))
        (define c (with-scope scope (test-connect "localhost")))
        (read-n c 6)
        (define waiting (spawn (lambda () (report (lambda () (tcp-read c))))))
        (sleep 20)
        (scope-shutdown! scope)
        (list (tcp-closed? c) (task-join waiting))
    "#,
    );
    assert_eq!(result, r#"(#t "tcp-read: the connection is closed")"#);
    worker.join().unwrap();
}

#[test]
fn cancelling_a_blocked_tls_write_keeps_record_state_usable() {
    let (port, worker) = server();
    let mut vm = vm(port, true);
    let result = run(
        &mut vm,
        r#"
        (define c (test-connect "localhost"))
        (read-n c 6)
        ;; The echo server eventually blocks writing to our unread socket,
        ;; then our writer blocks. Cancelling it must retain TLS record state.
        (define writer (spawn (lambda () (tcp-write c (make-string (* 32 1024 1024) #\x)))))
        (sleep 100)
        (define blocked (not (task-done? writer)))
        (task-cancel writer)
        (define stopped (report (lambda () (task-join writer))))
        (define drain
          (spawn (lambda ()
                   (let loop ((tail ""))
                     (let* ((text (tcp-read c)) (joined (string-append tail text))
                            (n (string-length joined)))
                       (if (string-contains joined "END") #t
                           (loop (substring joined (max 0 (- n 2)) n))))))))
        (tcp-write c "END")
        (define survived (task-join drain))
        (tcp-close c)
        (list blocked stopped survived)
    "#,
    );
    assert_eq!(result, r#"(#t "task cancelled" #t)"#);
    worker.join().unwrap();
}

#[test]
fn tls_verifies_trust_and_host_name() {
    for (trusted, host) in [(false, "localhost"), (true, "127.0.0.1")] {
        let (port, worker) = server();
        let mut vm = vm(port, trusted);
        let result = run(&mut vm, &format!(r#"(report (lambda () (test-connect "{host}")))"#));
        assert!(result.contains("tls-connect:") && result.contains("certificate"), "{result}");
        worker.join().unwrap();
    }
}

#[test]
fn tls_upgrades_an_existing_connection_and_still_checks_identity() {
    for (trust, host, succeeds) in [(true, "localhost", true), (true, "127.0.0.1", false), (false, "localhost", false)] {
        let (port, worker) = server_with_upgrade(true);
        let mut vm = vm(port, trust);
        let result = run(
            &mut vm,
            &format!(
                r#"
                (define c (tcp-connect "127.0.0.1" {port}))
                (define plain (read-n c 11))
                (tcp-write c "STARTTLS\r\n")
                (define reply (read-n c 15))
                (define result (report (lambda ()
                  (test-upgrade! c "{host}")
                  (define greeting (read-n c 6))
                  (tcp-write c "secure")
                  (list greeting (read-n c 6)))))
                (define was-closed (tcp-closed? c))
                (tcp-close c)
                (list plain reply result was-closed)
                "#
            ),
        );
        if succeeds {
            assert_eq!(result, r#"("plaintext\r\n" "220 Begin TLS\r\n" ("ready\n" "secure") #f)"#);
        } else {
            assert!(result.contains("tls-upgrade!:") && result.contains("certificate") && result.ends_with(" #t)"), "{result}");
        }
        worker.join().unwrap();
    }
}

#[test]
fn cancelling_or_closing_during_tls_upgrade_leaves_no_plaintext_socket() {
    let mut vm = vm(0, false);
    for (stop, reason) in
        [("(task-cancel upgrading)", "task cancelled"), ("(scope-shutdown! scope)", "tls-upgrade!: the connection is closed")]
    {
        let result = run(
            &mut vm,
            &format!(
                r#"
                (define listener (tcp-listen 0))
                (define scope (make-scope 'upgrade-test))
                (define c (with-scope scope (tcp-connect "127.0.0.1" (cadr (tcp-local-address listener)))))
                (define peer (tcp-accept listener))
                (define upgrading (spawn (lambda () (tls-upgrade! c "localhost"))))
                (tcp-read-bytes peer) ;; Wait for ClientHello; the peer never replies.
                (define blocked (report (lambda () (tcp-write c "must not send"))))
                {stop}
                (list blocked (report (lambda () (task-join upgrading))) (tcp-closed? c))
                "#
            ),
        );
        assert_eq!(result, format!(r#"("tcp-write: TLS handshake in progress" "{reason}" #t)"#));
    }
}

#[test]
fn tls_native_requires_network_capability() {
    let mut vm = Vm::with_grants(techne_vm::vm::Grants::ALL.without(Capability::Network));
    install(&mut vm).unwrap();
    for source in ["(tls-connect \"localhost\" 993)", "(tls-upgrade! #f \"localhost\")"] {
        let result = vm.eval_source(source);
        assert!(result.unwrap_err().to_string().contains("not granted"));
    }
}
