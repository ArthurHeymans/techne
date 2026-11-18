//! Argument forms and staging use the same contracts and invocation as
//! keys and callers; opening, editing and reviewing are not execution.

use std::time::Instant;

use techne_editor::runtime::Runtime;
use techne_text::Document;

fn eval(rt: &mut Runtime, source: &str) -> String {
    rt.eval(source).unwrap_or_else(|e| panic!("{source}: {e}"))
}

fn refused(rt: &mut Runtime, source: &str) -> String {
    eval(rt, &format!("(guard (e (#t (error-text e))) {source} 'ran)"))
}

const S: &str = "(current-session)";

const STAGING: &str = r#"
(define source (session-document (current-session)))
(define other (make-document "one two"))
(add-buffer! other)
(define v (session-view (current-session)))
(define w (make-view other "user"))
(define-command (rename-two s word) "Replace the first word in two documents."
  #:arguments '((word string))
  #:effects '(read edit) #:stageable #t
  (views-edit-at! (list (list v (document-revision source) (list (list 0 3 word)))
                       (list w (document-revision other) (list (list 0 3 word)))) "new"))
(invoke (current-session) 'rename-two (dict 'word "ONE") #:mode 'stage)
(define staged (car (kept-operation-records)))
(define ids (operation-proposals (record-operation staged)))
"#;

#[test]
fn staging_leaves_sources_and_selections_unchanged_and_accepts_as_one_linked_operation() {
    let mut rt = Runtime::with_document(Document::new("one two"), "emacs").unwrap();
    eval(&mut rt, STAGING);
    assert_eq!(
        eval(&mut rt, "(list (document-string source) (document-string other) (document-revision source) (view-ranges v))"),
        "(\"one two\" \"one two\" 0 ((0 0)))"
    );
    assert_eq!(eval(&mut rt, "(length ids)"), "2");
    assert_eq!(eval(&mut rt, "(record-violation staged)"), "#f");
    // Preview and acceptance are separate; a disjoint human edit rebases.
    eval(
        &mut rt,
        "(view-edit! (make-view source \"human\") '((4 7 \"TWO\")) \"new\")
       (define seen (map (lambda (id) (car (proposal-preview id))) ids))",
    );
    eval(&mut rt, &format!("(accept-operation-proposals! {S} staged ids seen)"));
    assert_eq!(eval(&mut rt, "(list (document-string source) (document-string other))"), "(\"ONE TWO\" \"ONE two\")");
    assert_eq!(
        eval(&mut rt, "(operation-parent-serial (record-operation (car (kept-operation-records))))"),
        eval(&mut rt, "(operation-serial (record-operation staged))")
    );
    assert_eq!(eval(&mut rt, "(operation-change-total (record-operation (car (kept-operation-records))))"), "2");
    eval(&mut rt, &format!("(undo-operation! {S} (car (kept-operation-records)))"));
    assert_eq!(eval(&mut rt, "(list (document-string source) (document-string other))"), "(\"one TWO\" \"one two\")");
}

#[test]
fn an_explicit_staged_parent_records_lineage_without_staging_new_work() {
    let mut rt = Runtime::with_document(Document::new("one two"), "emacs").unwrap();
    eval(&mut rt, STAGING);
    assert_eq!(eval(&mut rt, "(operation-phase (record-operation staged))"), "ended");
    eval(&mut rt, "(define child (start-operation! 'direct-edit (current-session) #:parent (record-operation staged)))");
    assert_eq!(eval(&mut rt, "(operation-staged? child)"), "#f");
    assert_eq!(eval(&mut rt, "(operation-parent-serial child)"), eval(&mut rt, "(operation-serial (record-operation staged))"));
    eval(&mut rt, "(call-in-operation child (lambda () (view-edit! v '((0 3 \"NOW\")) \"new\")))");
    assert_eq!(eval(&mut rt, "(document-string source)"), "\"NOW two\"");
    assert_eq!(eval(&mut rt, "(operation-proposals child)"), "()");
    assert_eq!(eval(&mut rt, "(current-operation)"), "#f");
}

#[test]
fn an_explicit_unstaged_parent_cannot_shed_the_running_operations_staging() {
    let mut rt = Runtime::with_document(Document::new("one two"), "emacs").unwrap();
    eval(&mut rt, STAGING);
    eval(
        &mut rt,
        "(define ancestor (start-operation! 'ancestor (current-session)))
         (call-in-operation ancestor (lambda () #f))
         (define child #f)
         (define-command (with-explicit-parent s) \"Edit with an unstaged ancestor while staging.\"
           #:arguments '() #:effects '(edit) #:stageable #t
           (set! child (start-operation! 'staged-edit s #:parent ancestor))
           (call-in-operation child (lambda () (view-edit! v '((4 7 \"TWO\")) \"new\"))))
         (invoke (current-session) 'with-explicit-parent (dict) #:mode 'stage)",
    );
    assert_eq!(eval(&mut rt, "(operation-staged? child)"), "#t");
    assert_eq!(eval(&mut rt, "(operation-parent-serial child)"), eval(&mut rt, "(operation-serial ancestor)"));
    assert_eq!(eval(&mut rt, "(document-string source)"), "\"one two\"");
    assert_eq!(eval(&mut rt, "(length (operation-proposals child))"), "1");
    assert_eq!(eval(&mut rt, "(current-operation)"), "#f");
}

#[test]
fn all_proposals_are_checked_before_any_is_applied() {
    let mut rt = Runtime::with_document(Document::new("one two"), "emacs").unwrap();
    eval(&mut rt, STAGING);
    eval(
        &mut rt,
        "(define seen (map (lambda (id) (car (proposal-preview id))) ids))
       (set-option! 'read-only #t #:buffer (document-buffer other))",
    );
    assert!(refused(&mut rt, &format!("(accept-operation-proposals! {S} staged ids seen)")).contains("read-only"));
    assert_eq!(eval(&mut rt, "(document-string source)"), "\"one two\"");
    eval(
        &mut rt,
        "(unset-option! 'read-only #:buffer (document-buffer other))
       (view-edit! (make-view other \"human\") '((4 7 \"TWO\")) \"new\")",
    );
    assert!(refused(&mut rt, &format!("(accept-operation-proposals! {S} staged ids seen)")).contains("changed since the preview"));
    assert_eq!(eval(&mut rt, "(document-string source)"), "\"one two\"", "first proposal was not applied");
    eval(&mut rt, "(view-edit! (make-view other \"human\") '((0 3 \"won\")) \"new\")");
    let why = refused(&mut rt, &format!("(accept-operation-proposals! {S} staged ids seen)"));
    assert!(why.contains("human") && why.contains("Refused"), "{why}");
    assert_eq!(eval(&mut rt, "(document-string source)"), "\"one two\"");
    assert_eq!(eval(&mut rt, "(map proposal-state ids)"), "(\"pending\" \"pending\")");
}

#[test]
fn external_effects_settings_undo_and_nested_apply_are_refused_when_staging() {
    let mut rt = Runtime::with_document(Document::new("abc"), "emacs").unwrap();
    eval(
        &mut rt,
        "(define runs 0)
       (define-command (external s) \"An external effect.\" #:arguments '() #:effects '(external) (set! runs (+ runs 1)))
       (define-command (nested s) \"Try an apply inside stage.\" #:arguments '() #:effects '(edit) #:stageable #t (invoke s 'external (dict)))
       (define-command (undoing s) \"Try undo.\" #:arguments '() #:effects '(edit) #:stageable #t (view-undo! (session-view s)))
       (define-command (setting s) \"Try an undeclared setting.\" #:arguments '() (set-option! 'read-only #t))",
    );
    for cmd in ["external", "nested", "undoing", "setting"] {
        let why = refused(&mut rt, &format!("(invoke {S} '{cmd} (dict) #:mode 'stage)"));
        assert!(why.contains("stage"), "{cmd}: {why}");
    }
    assert_eq!(eval(&mut rt, "runs"), "0");
    assert_eq!(eval(&mut rt, "(option (current-buffer (current-session)) 'read-only)"), "#f");
    // A query may discover an external step only after staging its edits,
    // as an LSP code action does. The proposals stay; no external step runs.
    eval(
        &mut rt,
        r#"
      (define-command (mixed s) "Propose an edit, then attempt an external step."
        #:arguments '() #:effects '(query edit) #:stageable #t
        (view-edit! (session-view s) '((0 3 "ABC")) "new")
        (note-effect! 'external)
        (set! runs (+ runs 1)))
    "#,
    );
    assert!(refused(&mut rt, &format!("(invoke {S} 'mixed (dict) #:mode 'stage)")).contains("stage mode"));
    assert_eq!(eval(&mut rt, "runs"), "0");
    assert_eq!(eval(&mut rt, "(operation-outcome (record-operation (car (kept-operation-records))))"), "partial");
    assert_eq!(eval(&mut rt, "(length (operation-proposals (record-operation (car (kept-operation-records)))))"), "1");
    assert_eq!(eval(&mut rt, "(document-string (session-document (current-session)))"), "\"abc\"");
}

#[test]
fn staging_an_edit_command_without_stageable_is_refused_before_it_runs() {
    let mut rt = Runtime::with_document(Document::new("abc"), "emacs").unwrap();
    eval(
        &mut rt,
        "(define runs 0)
       (define-command (edit-word s) \"Edit, reading nothing back.\" #:arguments '() #:effects '(edit)
         (set! runs (+ runs 1)) (view-edit! (session-view s) '((0 3 \"ABC\")) \"new\"))",
    );
    // Nothing marks the body as tolerating edits that have not landed, so
    // staging is refused rather than offered to every contract command.
    assert!(refused(&mut rt, &format!("(invoke {S} 'edit-word (dict) #:mode 'stage)")).contains("#:stageable"));
    assert_eq!(eval(&mut rt, "runs"), "0", "the body did not run");
    assert_eq!(eval(&mut rt, "(operation-proposals (record-operation (car (kept-operation-records))))"), "()");
    assert_eq!(eval(&mut rt, "(document-string (session-document (current-session)))"), "\"abc\"");
    eval(&mut rt, &format!("(invoke {S} 'edit-word (dict))"));
    assert_eq!(eval(&mut rt, "runs"), "1", "applying it is unaffected");
    assert_eq!(eval(&mut rt, "(document-string (session-document (current-session)))"), "\"ABC\"");
}

#[test]
fn stages_merge_disjoint_edits_but_refuse_partial_invalid_batches() {
    let mut rt = Runtime::with_document(Document::new("one two"), "emacs").unwrap();
    eval(
        &mut rt,
        "(define source (session-document (current-session)))
       (define v (session-view (current-session)))
       (define-command (twice s) \"Stage two disjoint edits.\" #:arguments '() #:effects '(edit) #:stageable #t
         (view-edit! v '((0 3 \"ONE\")) \"new\")
         (view-edit! v '((4 7 \"TWO\")) \"new\"))
       (invoke (current-session) 'twice (dict) #:mode 'stage)
       (define ids (operation-proposals (record-operation (car (kept-operation-records)))))",
    );
    assert_eq!(eval(&mut rt, "(length ids)"), "1");
    assert_eq!(eval(&mut rt, "(proposal-preview (car ids))"), "(0 ((0 3 \"ONE\") (4 7 \"TWO\")))");
    assert_eq!(eval(&mut rt, "(document-string source)"), "\"one two\"");
    eval(
        &mut rt,
        "(define-command (invalid-batch s) \"An invalid second entry.\" #:arguments '() #:effects '(edit) #:stageable #t
       (views-edit-at! (list (list v 0 '((0 3 \"bad\")))
                            (list (make-view (make-document \"x\") \"user\") 0 '((0 20 \"no\")))) \"new\"))",
    );
    refused(&mut rt, &format!("(invoke {S} 'invalid-batch (dict) #:mode 'stage)"));
    assert_eq!(eval(&mut rt, "(operation-proposals (record-operation (car (kept-operation-records))))"), "()");
    assert_eq!(eval(&mut rt, "(proposals source)"), eval(&mut rt, "ids"));
}

#[test]
fn staging_never_saves_and_does_not_escape_through_an_unheld_task() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.scm");
    std::fs::write(&path, "original").unwrap();
    let mut rt = Runtime::open(&path, &dir.path().join("journal"), "emacs").unwrap().0;
    eval(
        &mut rt,
        r#"
      (define source (session-document (current-session)))
      (define v (session-view (current-session)))
      (view-edit! v '((0 8 "edited")) "new")
      (define-command (save-sneakily s) "Attempt to save without declaring it."
        #:arguments '() #:effects '(edit) #:stageable #t (document-save! source))
    "#,
    );
    assert!(refused(&mut rt, &format!("(invoke {S} 'save-sneakily (dict) #:mode 'stage)")).contains("stage mode"));
    assert_eq!(std::fs::read_to_string(path).unwrap(), "original");
    eval(
        &mut rt,
        r#"
      (define late #f)
      (define-command (deferred s) "Attempt an unheld background edit."
        #:arguments '() #:effects '(edit) #:stageable #t
        (spawn (lambda ()
          (set! late (guard (e (#t (error-text e))) (view-edit! v '((0 6 "ESCAPED")) "new") "escaped")))))
      (invoke (current-session) 'deferred (dict) #:mode 'stage)
    "#,
    );
    let start = Instant::now();
    while eval(&mut rt, "late") == "#f" {
        assert!(start.elapsed() < std::time::Duration::from_secs(10));
        rt.run_tasks(std::time::Duration::from_millis(10)).unwrap();
    }
    assert!(eval(&mut rt, "late").contains("operation has ended"));
    assert_eq!(eval(&mut rt, "(document-string source)"), "\"edited\"");
}
