//! Services: worlds of their own, asked from the editor's world.

use std::time::{Duration, Instant};

use techne_editor::runtime::{Runtime, lisp_dir};
use techne_text::Document;

/// Run the runtime's tasks until `done`, Lisp, is true.
fn until(rt: &mut Runtime, done: &str) {
    let start = Instant::now();
    while rt.eval(done).unwrap() != "#t" {
        assert!(start.elapsed() < Duration::from_secs(20), "{done}");
        rt.run_tasks(Duration::from_millis(5)).unwrap();
    }
}

fn runtime_with(service: &std::path::Path) -> Runtime {
    let mut rt = Runtime::with_document(Document::new(""), "emacs").unwrap();
    rt.eval(&format!("(define svc (make-service \"test\" {:?}))", service.display().to_string())).unwrap();
    rt
}

/// A service answers about a snapshot as of its revision, without the
/// asking task holding up the editor's world.
#[test]
fn a_service_answers_about_a_snapshot() {
    let mut rt = runtime_with(&lisp_dir().join("services/definitions.scm"));
    rt.eval(
        "(define d (make-document \"(define (f x) x)\\n(display 1)\\n(define y 1)\\n\"))
         (define answer #f)
         (spawn (lambda () (set! answer (service-call svc \"definitions\" '() (list d)))))",
    )
    .unwrap();
    until(&mut rt, "(and answer #t)");
    let expected = "(list (list (document-id d) (document-revision d) '((\"f\" 0) (\"y\" 29))))";
    assert_eq!(rt.eval(&format!("(equal? answer {expected})")).unwrap(), "#t", "{}", rt.eval("answer").unwrap());
    // What it keeps between requests stays in its world: asked again, it
    // answers from it.
    rt.eval(
        "(set! answer #f)
         (spawn (lambda () (service-call svc \"index\" '() (list d)) (set! answer (service-call svc \"lookup\" \"y\" '()))))",
    )
    .unwrap();
    until(&mut rt, "(and answer #t)");
    assert_eq!(rt.eval("(equal? answer (list (list (document-id d) (document-revision d) 29)))").unwrap(), "#t");
}

/// Killing the task asking kills the request it waits for: the service
/// goes on to the next. Past its queue, a service refuses at once. A
/// service whose file fails says so to every request.
#[test]
fn requests_are_cancelled_bounded_and_failures_reported() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("spin.scm");
    std::fs::write(&file, "(define (spin a s) (let l () (l)))\n(define (echo a s) a)\n").unwrap();
    let mut rt = runtime_with(&file);
    rt.eval("(define stuck (spawn (lambda () (service-call svc \"spin\" 0 '()))))").unwrap();
    rt.run_tasks(Duration::from_millis(50)).unwrap();
    rt.eval("(task-kill stuck)").unwrap();
    rt.eval("(define answer #f) (spawn (lambda () (set! answer (service-call svc \"echo\" 42 '()))))").unwrap();
    until(&mut rt, "(eqv? answer 42)");
    assert_eq!(rt.eval("(service-starts svc)").unwrap(), "1", "the same world went on");
    // Kept spinning, the service queues its queue's worth, then refuses.
    rt.eval("(define spinning (map (lambda (i) (spawn (lambda () (service-call svc \"spin\" 0 '())))) (iota 20)))").unwrap();
    rt.run_tasks(Duration::from_millis(50)).unwrap();
    let refused: usize = rt.eval("(length (filter (lambda (t) (task-done? t)) spinning))").unwrap().parse().unwrap();
    // As the service took the first off its queue or not yet.
    let queue = techne_editor::service::QUEUE;
    assert!((20 - queue - 1..=20 - queue).contains(&refused), "{refused} refused");
    rt.eval("(for-each task-kill spinning)").unwrap();

    let mut broken = runtime_with(&dir.path().join("missing.scm"));
    broken.eval("(define failed #f) (spawn (lambda () (guard (e (#t (set! failed (condition/report-string e)))) (service-call svc \"echo\" 1 '()))))").unwrap();
    until(&mut broken, "(and failed #t)");
    assert!(broken.eval("failed").unwrap().contains("did not start"), "{}", broken.eval("failed").unwrap());
}

/// A service is made with the authority of the world making it: not
/// without loading, and not from a file that world may not read.
#[test]
fn a_service_has_no_more_authority_than_its_maker() {
    use techne_vm::vm::{Capability, Grants};
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("svc.scm");
    std::fs::write(&file, "(define (echo a s) a)\n").unwrap();
    let make = format!("(guard (e (#t (condition/report-string e))) (make-service \"s\" {:?}) \"made\")", file.display().to_string());
    let mut none = Runtime::with_document_granted(Document::new(""), "emacs", Grants::NONE.with(Capability::Loading)).unwrap();
    none.set_grant(Capability::Loading, false);
    assert!(none.eval(&make).unwrap().contains("not granted"), "{}", none.eval(&make).unwrap());
    let mut confined = Runtime::with_document(Document::new(""), "emacs").unwrap();
    confined.confine_reading(&tempfile::tempdir().unwrap().keep()).unwrap();
    assert!(confined.eval(&make).unwrap().contains("may not be read"), "{}", confined.eval(&make).unwrap());
}

/// Loading a service's file is bounded as a request is: a file that does
/// not end is killed, and the service says it did not start.
#[test]
fn a_service_whose_file_does_not_end() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("loop.scm");
    std::fs::write(&file, "(let loop () (loop))\n").unwrap();
    let mut rt = Runtime::with_document(Document::new(""), "emacs").unwrap();
    rt.eval(&format!(
        "(define svc (make-service \"loop\" {:?} 100))
         (define failed #f)
         (spawn (lambda () (guard (e (#t (set! failed (condition/report-string e)))) (service-call svc \"echo\" 1 '()))))",
        file.display().to_string()
    ))
    .unwrap();
    until(&mut rt, "(and failed #t)");
    assert!(rt.eval("failed").unwrap().contains("did not start"), "{}", rt.eval("failed").unwrap());
}

/// Positions are bytes, as the editor's: past text that is not ASCII
/// too. And a text's id is never another's, also once it is gone.
#[test]
fn positions_are_bytes_and_ids_are_not_reused() {
    let mut rt = runtime_with(&lisp_dir().join("services/definitions.scm"));
    rt.eval(
        "(define d (make-document \"; é\\n(define x 1)\\n\"))
         (define answer #f)
         (spawn (lambda () (set! answer (service-call svc \"definitions\" '() (list d)))))",
    )
    .unwrap();
    until(&mut rt, "(and answer #t)");
    assert_eq!(rt.eval("(caddr (car answer))").unwrap(), "((\"x\" 5))");
    let ids: Vec<String> = (0..3).map(|_| rt.eval("(begin (collect-garbage) (document-id (make-document \"\")))").unwrap()).collect();
    assert!(ids[0] != ids[1] && ids[1] != ids[2], "{ids:?}");
}
