//! Documents granted to services, and the changes they propose
//! (`access.rs`): staged, then accepted as edits by the service's actor.

use std::time::{Duration, Instant};

use techne_editor::runtime::Runtime;
use techne_text::Document;

/// Run the runtime's tasks until `done`, Lisp, is true.
fn until(rt: &mut Runtime, done: &str) {
    let start = Instant::now();
    while rt.eval(done).unwrap() != "#t" {
        assert!(start.elapsed() < Duration::from_secs(20), "{done}");
        rt.run_tasks(Duration::from_millis(5)).unwrap();
    }
}

/// A runtime with a document of `alpha beta gamma`, a human's view of it,
/// a service that proposes what it is asked to, and a grant to stage
/// changes to it: `(propose edits)` asks it, and files what it answers.
fn runtime() -> Runtime {
    let dir = tempfile::tempdir().unwrap().keep();
    let file = dir.join("propose.scm");
    std::fs::write(
        &file,
        "(define (propose edits snapshots)\n  (let ((s (car snapshots))) (list (snapshot-grant s) (snapshot-revision s) edits)))\n",
    )
    .unwrap();
    let mut rt = Runtime::with_document(Document::new(""), "emacs").unwrap();
    rt.eval(&format!(
        "(define doc (make-document \"alpha beta gamma\"))
         (define human (make-view doc \"human\"))
         (define svc (make-service \"fmt\" {:?}))
         (define grant (document-grant doc 'stage #:to svc #:actor \"service:fmt\"))
         (define (ask service edits g)
           (guard (e (#t (condition/report-string e))) (service-call service \"propose\" edits (list g))))
         (define filed #f)
         (define (propose edits)
           (set! filed #f)
           (spawn (lambda ()
             (let ((answer (ask svc edits grant)))
               (set! filed
                 (if (string? answer) answer
                     (guard (e (#t (condition/report-string e)))
                       (proposal-file! svc (car answer) (cadr answer) (caddr answer) \"test\"))))))))",
        file.display().to_string()
    ))
    .unwrap();
    rt
}

fn propose(rt: &mut Runtime, edits: &str) -> String {
    rt.eval(&format!("(propose '{edits})")).unwrap();
    until(rt, "(and filed #t)");
    rt.eval("filed").unwrap()
}

fn eval(rt: &mut Runtime, src: &str) -> String {
    rt.eval(src).unwrap_or_else(|e| format!("error: {e}"))
}

/// A proposal waits; the human edits elsewhere meanwhile; accepted, it
/// is moved past that, as an edit by the service's actor, which the
/// human's undo does not take.
#[test]
fn a_proposal_is_accepted_past_a_disjoint_edit() {
    let mut rt = runtime();
    let p = propose(&mut rt, "((11 16 \"GAMMA\"))");
    assert!(p.parse::<u64>().is_ok(), "{p}");
    assert_eq!(eval(&mut rt, "(document-string doc)"), "\"alpha beta gamma\"", "only staged");
    assert_eq!(eval(&mut rt, "(proposals doc)"), format!("({p})"));
    eval(&mut rt, "(view-edit! human '((0 5 \"ALPHA\")) \"new\")");
    assert_eq!(eval(&mut rt, &format!("(proposal-preview {p})")), "(1 ((11 16 \"GAMMA\")))");
    eval(&mut rt, &format!("(proposal-accept! {p})"));
    assert_eq!(eval(&mut rt, "(document-string doc)"), "\"ALPHA beta GAMMA\"");
    assert_eq!(eval(&mut rt, &format!("(proposal-state {p})")), "\"applied\"");
    assert_eq!(eval(&mut rt, "(proposals doc)"), "()");
    eval(&mut rt, "(view-undo! human)");
    assert_eq!(eval(&mut rt, "(document-string doc)"), "\"alpha beta GAMMA\"", "the human's undo is the human's");
    // The service's change is its actor's, to undo as any edit.
    eval(&mut rt, "(view-undo! (make-view doc \"service:fmt\"))");
    assert_eq!(eval(&mut rt, "(document-string doc)"), "\"alpha beta gamma\"");
}

/// A proposal touching what the human changed since is refused, naming
/// the human; the text is unchanged. Accepted as a preview showed it, a
/// proposal is refused once the text changed since.
#[test]
fn conflicts_and_stale_previews_are_refused() {
    let mut rt = runtime();
    let p = propose(&mut rt, "((0 5 \"ALPHA\"))");
    eval(&mut rt, "(view-edit! human '((0 5 \"Alpha\")) \"new\")");
    let refused = eval(&mut rt, &format!("(proposal-accept! {p})"));
    assert!(refused.contains("Refused") && refused.contains("human"), "{refused}");
    assert_eq!(eval(&mut rt, "(document-string doc)"), "\"Alpha beta gamma\"");

    let q = propose(&mut rt, "((6 10 \"BETA\"))");
    let seen = eval(&mut rt, &format!("(car (proposal-preview {q}))"));
    eval(&mut rt, "(view-edit! human '((11 16 \"Gamma\")) \"new\")");
    let refused = eval(&mut rt, &format!("(proposal-accept! {q} #:seen {seen})"));
    assert!(refused.contains("changed since the preview"), "{refused}");
    let seen = eval(&mut rt, &format!("(car (proposal-preview {q}))"));
    eval(&mut rt, &format!("(proposal-accept! {q} #:seen {seen})"));
    assert_eq!(eval(&mut rt, "(document-string doc)"), "\"Alpha BETA Gamma\"");
}

/// A revoked grant gives no more snapshots and files nothing; what was
/// proposed through it stays, and is accepted only to adopt it.
#[test]
fn a_revoked_grants_proposals_are_adopted_or_not_at_all() {
    let mut rt = runtime();
    let p = propose(&mut rt, "((0 5 \"ALPHA\"))");
    eval(&mut rt, "(grant-revoke! grant)");
    assert!(propose(&mut rt, "((6 10 \"BETA\"))").contains("revoked"));
    assert_eq!(eval(&mut rt, &format!("(proposal-state {p})")), "\"revoked\"");
    assert!(eval(&mut rt, &format!("(proposal-accept! {p})")).contains("adopt"));
    eval(&mut rt, &format!("(proposal-accept! {p} #:adopt #t)"));
    assert_eq!(eval(&mut rt, "(document-string doc)"), "\"ALPHA beta gamma\"");
}

/// A grant to read proposes nothing; a grant is not another service's,
/// nor that of the service's world started again.
#[test]
fn grants_hold_for_their_right_their_service_and_its_world() {
    let mut rt = runtime();
    eval(&mut rt, "(define grant-read (document-grant doc 'read #:to svc))");
    eval(&mut rt, "(set! grant grant-read)");
    assert!(propose(&mut rt, "((0 5 \"ALPHA\"))").contains("reading, not proposing"));

    eval(&mut rt, "(set! grant (document-grant doc 'stage #:to svc #:actor \"service:fmt\"))");
    eval(&mut rt, &format!("(define other (make-service \"other\" {:?}))", "/nonexistent.scm"));
    assert!(eval(&mut rt, "(proposal-file! other grant 0 '((0 5 \"x\")) \"forged\")").contains("no such grant"));

    let p = propose(&mut rt, "((0 5 \"ALPHA\"))");
    eval(&mut rt, "(service-stop! svc)");
    assert_eq!(eval(&mut rt, &format!("(proposal-state {p})")), "\"revoked\"");
    assert!(propose(&mut rt, "((6 10 \"BETA\"))").contains("revoked"));
    assert_eq!(eval(&mut rt, "(service-starts svc)"), "1", "refused before the service was asked");
}

/// A proposal is moved past text the human inserted before it, and
/// rejected or accepted, is decided: it is no longer offered.
#[test]
fn a_proposal_moves_past_an_insertion_before_it() {
    let mut rt = runtime();
    let p = propose(&mut rt, "((11 16 \"GAMMA\"))");
    eval(&mut rt, "(view-edit! human '((0 0 \"and \")) \"new\")");
    assert_eq!(eval(&mut rt, &format!("(proposal-preview {p})")), "(1 ((15 20 \"GAMMA\")))");
    eval(&mut rt, &format!("(proposal-accept! {p})"));
    assert_eq!(eval(&mut rt, "(document-string doc)"), "\"and alpha beta GAMMA\"");
    assert!(eval(&mut rt, &format!("(proposal-accept! {p})")).contains("decided"));
    let q = propose(&mut rt, "((0 3 \"AND\"))");
    eval(&mut rt, &format!("(proposal-reject! {q})"));
    assert_eq!(eval(&mut rt, &format!("(proposal-state {q})")), "\"rejected\"");
    assert!(eval(&mut rt, &format!("(proposal-preview {q})")).contains("decided"));
    assert_eq!(eval(&mut rt, "(proposals doc)"), "()");
}
