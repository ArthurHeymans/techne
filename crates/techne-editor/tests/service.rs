//! Services: worlds of their own, asked from the editor's world.

use std::time::{Duration, Instant};

use techne_editor::runtime::{Runtime, lisp_dir};
use techne_text::Document;

/// Run the runtime's tasks until `done`, Lisp, is true.
fn until(rt: &mut Runtime, done: &str) {
    let start = Instant::now();
    while rt.eval(done).unwrap() != "#t" {
        assert!(start.elapsed() < Duration::from_secs(20), "{done}");
        rt.run_tasks(Duration::from_millis(5));
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
    rt.run_tasks(Duration::from_millis(50));
    rt.eval("(task-kill stuck)").unwrap();
    rt.eval("(define answer #f) (spawn (lambda () (set! answer (service-call svc \"echo\" 42 '()))))").unwrap();
    until(&mut rt, "(eqv? answer 42)");
    assert_eq!(rt.eval("(service-starts svc)").unwrap(), "1", "the same world went on");
    // Kept spinning, the service queues its queue's worth, then refuses.
    rt.eval("(define spinning (map (lambda (i) (spawn (lambda () (service-call svc \"spin\" 0 '())))) (iota 20)))").unwrap();
    rt.run_tasks(Duration::from_millis(50));
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

/// What runs in a service does not stall the editor (PLAN.md, step 8's
/// acceptance): a request that loops and one that catches the stops it
/// is given end past their service's deadline, and one that floods
/// memory past another's memory limit (its deadline far off, so that the
/// limit ends it), each in an error, while the editor's world and a third
/// service go on answering. Then the services answer again.
#[test]
fn hostile_requests_do_not_stall_the_editor() {
    let dir = tempfile::tempdir().unwrap();
    let hostile = dir.path().join("hostile.scm");
    std::fs::write(
        &hostile,
        "(define (spin a s) (let l () (l)))
         (define (flood a s) (let l ((kept '())) (l (cons (make-vector 1000 0) kept))))
         (define (stubborn a s) (let l () (guard (e (#t (l))) (l))))
         (define (echo a s) a)\n",
    )
    .unwrap();
    let echo = dir.path().join("echo.scm");
    std::fs::write(&echo, "(define (echo a s) a)\n").unwrap();
    let mut rt = Runtime::with_document(Document::new(""), "emacs").unwrap();
    rt.eval(&format!(
        "(define timed (make-service \"timed\" {hostile:?} 1000))
         (define bounded (make-service \"bounded\" {hostile:?} 60000 #:memory 64))
         (define other (make-service \"other\" {echo:?}))
         (define (ask service name)
           (guard (e (#t (condition/report-string e))) (service-call service name 0 '())))
         (define answered #f)
         (define warm '())
         (for-each (lambda (s) (spawn (lambda () (set! warm (cons (service-call s \"echo\" 7 '()) warm)))))
                   (list timed bounded other))",
        hostile = hostile.display().to_string(),
        echo = echo.display().to_string()
    ))
    .unwrap();
    // Every service started, loading its file within its deadline, before
    // the third's answers are timed.
    until(&mut rt, "(equal? warm '(7 7 7))");
    rt.eval(
        "(define results (map (lambda (r) (cons r #f)) (list (cons timed \"spin\") (cons bounded \"flood\") (cons timed \"stubborn\"))))
         (for-each (lambda (r) (spawn (lambda () (set-cdr! r (ask (caar r) (cdar r)))))) results)",
    )
    .unwrap();
    let start = Instant::now();
    while rt.eval("(and (every cdr results) #t)").unwrap() != "#t" {
        assert!(start.elapsed() < Duration::from_secs(30), "{}", rt.eval("(map cdr results)").unwrap());
        let asked = Instant::now();
        assert_eq!(rt.eval("(+ 1 2)").unwrap(), "3");
        rt.eval("(set! answered #f) (spawn (lambda () (set! answered (service-call other \"echo\" 7 '()))))").unwrap();
        until(&mut rt, "(eqv? answered 7)");
        assert!(asked.elapsed() < Duration::from_secs(2), "the editor waited {:?}", asked.elapsed());
    }
    let result = |rt: &mut Runtime, i: usize| rt.eval(&format!("(cdr (list-ref results {i}))")).unwrap();
    assert!(result(&mut rt, 0).contains("killed"), "{}", result(&mut rt, 0));
    assert!(result(&mut rt, 1).contains("out of memory"), "{}", result(&mut rt, 1));
    assert!(result(&mut rt, 2).contains("killed"), "{}", result(&mut rt, 2));
    rt.eval(
        "(define again '())
         (for-each (lambda (s) (spawn (lambda () (set! again (cons (service-call s \"echo\" 42 '()) again))))) (list timed bounded))",
    )
    .unwrap();
    until(&mut rt, "(equal? again '(42 42))");
    // No limit is not a limit.
    let none = rt.eval("(guard (e (#t (condition/report-string e))) (make-service \"none\" \"/\" #f #:memory 0))").unwrap();
    assert!(none.contains("no memory limit"), "{none}");
}

/// Services are listed, and each shows what it is (data the object
/// microscope shows): a looping one runs; stopped, its request fails and
/// the others answer on; restarted, a new world answers. Shutting the
/// scope that made one closes it.
#[test]
fn services_are_listed_stopped_restarted_and_closed() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("spin.scm");
    std::fs::write(&file, "(define (spin a s) (let l () (l)))\n(define (echo a s) a)\n").unwrap();
    let mut rt = Runtime::with_document(Document::new(""), "emacs").unwrap();
    rt.eval(&format!(
        "(define a (make-service \"a\" {file:?} 60000))
         (define b (make-service \"b\" {file:?}))
         (define (info s key) (dict-ref (service-info s) key))
         (define (ask s name)
           (guard (e (#t (condition/report-string e))) (service-call s name 0 '())))
         (define spun #f)
         (define queued #f)
         (spawn (lambda () (set! spun (ask a \"spin\"))))
         (spawn (lambda () (set! queued (ask a \"echo\"))))",
        file = file.display().to_string()
    ))
    .unwrap();
    assert_eq!(rt.eval("(map service-name (services))").unwrap(), "(\"a\" \"b\")");
    until(&mut rt, "(eq? (info a 'state) 'running)");

    rt.eval("(service-stop! a)").unwrap();
    until(&mut rt, "(and spun #t)");
    assert!(rt.eval("spun").unwrap().contains("killed"), "{}", rt.eval("spun").unwrap());
    // What it queued behind is not run.
    until(&mut rt, "(and queued #t)");
    assert!(rt.eval("queued").unwrap().contains("stopped"), "{}", rt.eval("queued").unwrap());
    assert_eq!(rt.eval("(list (info a 'state) (info a 'ended) (info a 'last-end))").unwrap(), "(stopped 1 \"stopped\")");
    rt.eval("(define echoed #f) (spawn (lambda () (set! echoed (ask b \"echo\"))))").unwrap();
    until(&mut rt, "(eqv? echoed 0)");
    assert!(rt.eval("(> (info b 'held) 0)").unwrap() == "#t");

    rt.eval("(service-restart! a) (set! echoed #f) (spawn (lambda () (set! echoed (ask a \"echo\"))))").unwrap();
    until(&mut rt, "(eqv? echoed 0)");
    assert_eq!(rt.eval("(list (info a 'state) (info a 'starts))").unwrap(), "(idle 2)");

    rt.eval(&format!(
        "(define package (make-scope 'package))
         (define c (with-scope package (make-service \"c\" {file:?})))
         (set! echoed #f)
         (spawn (lambda () (set! echoed (ask c \"echo\"))))",
        file = file.display().to_string()
    ))
    .unwrap();
    until(&mut rt, "(eqv? echoed 0)");
    rt.eval("(scope-shutdown! package)").unwrap();
    assert_eq!(rt.eval("(info c 'state)").unwrap(), "closed");
    rt.eval("(set! echoed #f) (spawn (lambda () (set! echoed (ask c \"echo\"))))").unwrap();
    until(&mut rt, "(and echoed #t)");
    assert!(rt.eval("echoed").unwrap().contains("closed"), "{}", rt.eval("echoed").unwrap());
    assert_eq!(rt.eval("(map service-name (services))").unwrap(), "(\"a\" \"b\")");
}
