//! Sockets in the editor's world wait without holding up its frontends.

use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    sync::mpsc,
    time::{Duration, Instant},
};

use techne_editor::{
    host::{Event, Host},
    present::{Input, Output},
    runtime::{Progress, Runtime},
};
use techne_vm::vm::{Capability, Grants};

fn exchange(port: u16) -> String {
    format!(
        "(define answer #f) (define sent #f)
         (define request
           (spawn (lambda ()
             (set! answer
               (call-with-tcp-connection \"127.0.0.1\" {port}
                 (lambda (c) (tcp-write c \"hello\\n\") (set! sent #t) (tcp-read c)))))))"
    )
}

#[test]
fn a_socket_task_yields_and_returns_a_line() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut rt = Runtime::new().unwrap();
    rt.eval(&exchange(listener.local_addr().unwrap().port())).unwrap();
    listener.set_nonblocking(true).unwrap();
    let connecting = Instant::now();
    let socket = loop {
        rt.run_tasks(Duration::from_millis(5)).unwrap();
        if let Ok((socket, _)) = listener.accept() {
            break socket;
        }
        assert!(connecting.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(1));
    };
    socket.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut peer = BufReader::new(socket);
    let start = Instant::now();
    while rt.eval("sent").unwrap() != "#t" {
        rt.run_tasks(Duration::from_millis(5)).unwrap();
        assert!(start.elapsed() < Duration::from_secs(10));
    }
    assert_eq!(rt.run_tasks(Duration::from_millis(5)).unwrap(), Progress::Blocked);
    assert_eq!(rt.eval("answer").unwrap(), "#f");
    assert_eq!(rt.eval("(+ 20 22)").unwrap(), "42", "the editor can still evaluate");
    let mut line = String::new();
    peer.read_line(&mut line).unwrap();
    assert_eq!(line, "hello\n");
    peer.get_mut().write_all(b"reply\n").unwrap();
    while rt.eval("(task-done? request)").unwrap() != "#t" {
        assert!(start.elapsed() < Duration::from_secs(10));
        rt.run_tasks(Duration::from_millis(5)).unwrap();
        std::thread::sleep(Duration::from_millis(1));
    }
    rt.eval("(task-join request)").unwrap();
    assert_eq!(rt.eval("answer").unwrap(), "\"reply\\n\"");
}

/// No timer or frontend input wakes the runtime after the peer replies.
#[test]
fn socket_readiness_wakes_the_serving_editor() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, events) = mpsc::channel();
    let mut host = Host::start(
        None,
        "emacs".into(),
        move |rt| {
            rt.eval(&exchange(port)).unwrap();
            rt.eval("(spawn (lambda () (task-join request) (message! (current-session) answer)))").unwrap();
        },
        move |e| drop(tx.send(e)),
    );
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let socket = loop {
        if let Ok((socket, _)) = listener.accept() {
            break socket;
        }
        if let Ok(Event::Failed(e)) = events.try_recv() {
            panic!("{e}");
        }
        assert!(Instant::now() < deadline, "the editor did not connect");
        std::thread::sleep(Duration::from_millis(1));
    };
    socket.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut peer = BufReader::new(socket);
    let mut line = String::new();
    peer.read_line(&mut line).unwrap();
    assert_eq!(line, "hello\n");
    host.send(Input::Key { key: "x".into(), at: Instant::now() });
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match events.recv_timeout(deadline.saturating_duration_since(Instant::now())).unwrap() {
            Event::Output(Output::Snapshot(s)) if s.pane().text == "x" => break,
            Event::Failed(e) => panic!("{e}"),
            _ => {}
        }
    }
    peer.get_mut().write_all(b"awake\n").unwrap();
    loop {
        match events.recv_timeout(deadline.saturating_duration_since(Instant::now())).unwrap() {
            Event::Output(Output::Snapshot(s)) if s.echo == "awake\n" => break,
            Event::Failed(e) => panic!("{e}"),
            _ => {}
        }
    }
    host.close();
}

#[test]
fn restricted_worlds_have_no_network_authority() {
    let mut rt = Runtime::granted(Grants::NONE.with(Capability::Loading)).unwrap();
    for form in ["(tcp-listen 0)", "(tcp-connect \"127.0.0.1\" 1)"] {
        assert!(rt.eval(form).unwrap_err().to_string().contains("not granted"));
    }
}
