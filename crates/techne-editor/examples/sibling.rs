//! The cost of a service (`techne_editor::service`) against the same work
//! done in the editor's world, as a task between keys. The work: finding
//! the definitions of a large Scheme file (`lisp/editor/services/
//! definitions.scm`) again after every edit, while keys are typed into it
//! at a steady rate through a host (`Host`), as a frontend sends them.
//! Each key is timed from its input to the snapshot answering it.
//!
//! `sibling` runs each way and a round trip of an empty request; `sibling
//! MODE` runs one: `none` (no indexing), `in-world`, `service-copy` (every
//! definition copied back) or `service` (the index kept in the service,
//! a count copied back).

use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

use techne_editor::{
    host::{Event, File, Host},
    present::{Input, Output},
    runtime::{Runtime, lisp_dir},
};

/// Keys typed, and how often; the first `WARM` are not counted (code
/// compiled on first use).
const KEYS: usize = 300;
const WARM: usize = 20;
const EVERY: Duration = Duration::from_millis(40);

/// Definitions in the file, each a few lines.
const DEFINITIONS: usize = 20_000;

fn source() -> String {
    (0..DEFINITIONS).map(|i| format!("(define (f{i} x)\n  ;; Adds {i}.\n  (+ x {i}))\n\n(display (f{i} 1))\n")).collect()
}

/// The indexing loop, in the editor's world: after each edit of the file,
/// its definitions found again, by `find`.
fn indexer(find: &str) -> String {
    format!(
        "(import (techne editor))
         (require {defs:?})
         (define svc (make-service \"definitions\" {defs:?}))
         (define indexed 0)
         (define index #f)
         (define (file-document)
           (let ((b (find (lambda (b) (document-path (buffer-document b))) (buffer-list))))
             (and b (buffer-document b))))
         (spawn (lambda ()
           (let loop ((seen -1))
             (let* ((d (file-document)) (r (if d (document-revision d) seen)))
               (if (= r seen)
                   (begin (sleep 5) (loop seen))
                   (begin (set! index {find}) (set! indexed (+ indexed 1)) (loop r)))))))",
        defs = lisp_dir().join("services/definitions.scm").display().to_string(),
    )
}

fn run(mode: &str) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.scm");
    std::fs::write(&path, source()).unwrap();
    let setup = match mode {
        "none" => String::new(),
        "in-world" => indexer("(definitions-in (document-string d))"),
        // Every definition copied back.
        "service-copy" => indexer("(service-call svc \"definitions\" '() (list d))"),
        // The index kept in the service, a count copied back.
        _ => indexer("(service-call svc \"index\" '() (list d))"),
    };
    let (tx, events) = mpsc::channel();
    let mut host = Host::start(
        Some(File { path, journal: dir.path().join("big.journal") }),
        "emacs".into(),
        move |rt: &mut Runtime| drop(rt.eval(&setup).unwrap()),
        move |e| drop(tx.send((Instant::now(), e))),
    );
    // Shown, and indexed once.
    std::thread::sleep(Duration::from_secs(3));
    let mut latencies = Vec::new();
    let mut next = Instant::now();
    for i in 0..KEYS {
        std::thread::sleep(next.saturating_duration_since(Instant::now()));
        next += EVERY;
        host.send(Input::Key { key: ["x", "y", "SPC"][i % 3].into(), at: Instant::now() });
        latencies.extend(answered(&events, Duration::ZERO));
    }
    latencies.extend(answered(&events, Duration::from_secs(2)));
    let mut latencies = latencies.split_off(WARM.min(latencies.len()));
    latencies.sort();
    let at = |q: f64| latencies[((latencies.len() - 1) as f64 * q) as usize];
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    println!(
        "{mode:9} keys {:3}  p50 {:6.2} ms  p99 {:6.2} ms  max {:6.2} ms",
        latencies.len(),
        ms(at(0.5)),
        ms(at(0.99)),
        ms(*latencies.last().unwrap())
    );
    host.close();
}

/// How long the keys answered by the snapshots received within `wait`
/// waited for them.
fn answered(events: &mpsc::Receiver<(Instant, Event)>, wait: Duration) -> Vec<Duration> {
    let deadline = Instant::now() + wait;
    std::iter::from_fn(|| events.recv_timeout(deadline.saturating_duration_since(Instant::now())).ok())
        .flat_map(|(when, e)| match e {
            Event::Output(Output::Snapshot(s)) => s.answers.iter().map(|at| when - *at).collect(),
            _ => Vec::new(),
        })
        .collect()
}

/// An empty request's round trip, from a task of the editor's world.
fn round_trip() {
    let mut rt = Runtime::with_document(techne_text::Document::new(""), "emacs").unwrap();
    let defs = lisp_dir().join("services/definitions.scm").display().to_string();
    rt.eval(&format!("(define svc (make-service \"definitions\" {defs:?})) (define left 1000)")).unwrap();
    // The first starts the world.
    rt.eval("(define warm (spawn (lambda () (service-call svc \"echo\" 0 '()))))").unwrap();
    while rt.eval("(task-done? warm)").unwrap() != "#t" {
        rt.run_tasks(Duration::from_millis(1)).unwrap();
    }
    let start = Instant::now();
    rt.eval("(spawn (lambda () (let l () (unless (= left 0) (service-call svc \"echo\" left '()) (set! left (- left 1)) (l)))))").unwrap();
    while rt.eval("left").unwrap() != "0" {
        rt.run_tasks(Duration::from_millis(1)).unwrap();
    }
    println!("round trip of an empty request: {:.1} µs", start.elapsed().as_secs_f64() * 1e6 / 1000.0);
}

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("round-trip") => round_trip(),
        Some(mode) => run(mode),
        None => {
            round_trip();
            ["none", "in-world", "service-copy", "service"].into_iter().for_each(run);
        }
    }
}
