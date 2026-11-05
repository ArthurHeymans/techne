//! Argument forms and staging use the same contracts and invocation as
//! keys and callers; opening, editing and reviewing are not execution.

use std::time::Instant;

use techne_editor::{present::Input, runtime::Runtime};
use techne_text::Document;

fn eval(rt: &mut Runtime, source: &str) -> String {
    rt.eval(source).unwrap_or_else(|e| panic!("{source}: {e}"))
}

fn refused(rt: &mut Runtime, source: &str) -> String {
    eval(rt, &format!("(guard (e (#t (error-text e))) {source} 'ran)"))
}

fn keys(rt: &mut Runtime, keys: &str) {
    for key in keys.split(' ') {
        rt.handle(Input::Key { key: key.into(), at: Instant::now() });
    }
}

const S: &str = "(current-session)";

#[test]
fn a_form_rechecks_dependent_arguments_and_runs_in_its_source_in_both_profiles() {
    for profile in ["emacs", "modal"] {
        let mut rt = Runtime::with_document(Document::new("abc"), profile).unwrap();
        eval(
            &mut rt,
            "(define source (session-document (current-session)))
           (define runs 0)
           (define-command (pick s kind value) \"Pick a KIND and a VALUE of it.\"
             #:arguments `((kind (one-of number text) #:default number)
                           (value ,(lambda (args) (if (eq? (dict-ref args 'kind) 'number) 'integer 'string))))
             #:effects '(edit)
             (set! runs (+ runs 1))
             (insert-text! s (if (string? value) value (number->string value)) \"new\"))
           (define f (open-command-form! (current-session) 'pick))",
        );
        assert_eq!(eval(&mut rt, "runs"), "0");
        assert_eq!(eval(&mut rt, "(map car (form-problems f))"), "(value)");
        eval(&mut rt, "(form-set-argument! f 'value 3)");
        assert_eq!(eval(&mut rt, "(form-problems f)"), "()");
        eval(&mut rt, "(form-set-argument! f 'kind 'text)");
        assert!(eval(&mut rt, "(form-problems f)").contains("expected a string"));
        assert!(refused(&mut rt, &format!("(form-invoke! {S} f)")).contains("need attention"));
        assert_eq!(eval(&mut rt, "runs"), "0");
        // The existing schema reader, not a second form-specific parser.
        eval(&mut rt, "(form-set-argument! f 'value \"\")");
        keys(&mut rt, "2 x y RET RET");
        assert_eq!(eval(&mut rt, "runs"), "1", "{profile}: {}", rt.snapshot().echo);
        assert_eq!(eval(&mut rt, "(document-string source)"), "\"xyabc\"");
        let shown = eval(&mut rt, &format!("(document-string (session-document {S}))"));
        assert!(shown.contains("last run") && shown.contains("done"), "{shown}");
    }
}

#[test]
fn argument_names_do_not_collide_with_view_rows_and_forms_stay_independent() {
    let mut rt = Runtime::with_document(Document::new(""), "emacs").unwrap();
    eval(
        &mut rt,
        r#"
      (define-command (choose s command keys last) "Return COMMAND, KEYS and LAST."
        #:arguments '((command string #:default "run") (keys string #:default "key") (last string #:default "tail"))
        (list command keys last))
      (define first (open-command-form! (current-session) 'choose))
      (form-set-argument! first 'command "changed")
      (sset! (current-session) 'focus 0)
      (define second (open-command-form! (current-session) 'choose))
    "#,
    );
    assert_eq!(eval(&mut rt, "(length (filter (lambda (b) (string-prefix? \"*arguments: choose \" (buffer-name b))) (buffer-list)))"), "2");
    assert_eq!(eval(&mut rt, "(form-invoke! (current-session) first)"), "(\"changed\" \"key\" \"tail\")");
    assert_eq!(eval(&mut rt, "(form-invoke! (current-session) second)"), "(\"run\" \"key\" \"tail\")");
}

#[test]
fn again_revalidates_references_omits_secrets_and_links_the_new_operation() {
    let mut rt = Runtime::with_document(Document::new("abc"), "emacs").unwrap();
    eval(
        &mut rt,
        "(define source (session-document (current-session)))
       (define seen #f)
       (define-command (mark s at password text) \"Record AT, PASSWORD and TEXT.\"
         #:arguments '((at location) (password string #:sensitive) (text string))
         (set! seen (list (location-position at) password text)))
       (invoke (current-session) 'mark (dict 'at (location source 2) 'password \"hunter2\" 'text \"short\"))
       (define old (car (kept-operation-records)))
       (view-edit! (session-view (current-session)) '((0 0 \">>\")) \"new\")
       (define f (open-command-form! (current-session) 'mark #:record old))",
    );
    let args = eval(&mut rt, "(form-arguments f)");
    assert!(!args.contains("hunter2") && !args.contains("<redacted>"), "{args}");
    assert_eq!(eval(&mut rt, "(map car (form-problems f))"), "(password)");
    assert_eq!(eval(&mut rt, "(car seen)"), "2", "opening does not run it");
    eval(&mut rt, "(form-set-argument! f 'password \"new-password\")");
    eval(&mut rt, &format!("(form-invoke! {S} f)"));
    assert_eq!(eval(&mut rt, "(car seen)"), "4", "reference maps through subsequent edits");
    assert_eq!(
        eval(&mut rt, "(operation-parent-serial (record-operation (car (kept-operation-records))))"),
        eval(&mut rt, "(operation-serial (record-operation old))")
    );
    assert!(!eval(&mut rt, "(record-arguments (car (kept-operation-records)))").contains("new-password"));
}

#[test]
fn shortened_arguments_are_not_silently_replayed_and_redefinition_needs_review() {
    let mut rt = Runtime::with_document(Document::new(""), "emacs").unwrap();
    eval(
        &mut rt,
        "(define seen #f)
       (define-command (keep s text) \"Keep TEXT.\" #:arguments '((text string)) (set! seen text))
       (invoke (current-session) 'keep (dict 'text (make-string 1000 #\\x)))
       (define old (car (kept-operation-records)))
       (define f (open-command-form! (current-session) 'keep #:record old))",
    );
    assert_eq!(eval(&mut rt, "(record-unavailable-arguments old)"), "(text)");
    assert_eq!(eval(&mut rt, "(dict-size (form-arguments f))"), "0");
    eval(
        &mut rt,
        "(form-set-argument! f 'text \"new\")
       (define-command (keep s text) \"Keep TEXT differently.\" #:arguments '((text string)) (set! seen (string-append text \"!\")))",
    );
    assert!(refused(&mut rt, &format!("(form-invoke! {S} f)")).contains("command changed"));
    assert_eq!(eval(&mut rt, "(string-length seen)"), "1000");
    eval(&mut rt, &format!("(form-invoke! {S} f)"));
    assert_eq!(eval(&mut rt, "seen"), "\"new!\"");
}

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
fn optional_arguments_can_be_omitted_and_a_closed_source_is_not_replaced() {
    let mut rt = Runtime::with_document(Document::new(""), "emacs").unwrap();
    eval(
        &mut rt,
        r#"
      (define runs 0)
      (define-command (maybe s count) "Use COUNT when given."
        #:arguments '((count integer #:optional)) (set! runs (+ runs 1)))
      (define f (open-command-form! (current-session) 'maybe))
      (form-set-argument! f 'count 3)
      (form-clear-argument! f 'count)
    "#,
    );
    assert_eq!(eval(&mut rt, "(form-problems f)"), "()");
    eval(&mut rt, &format!("(form-invoke! {S} f)"));
    assert_eq!(eval(&mut rt, "runs"), "1");
    // Closing the original pane must not make the form's generated text
    // (or some other pane) the command's new context.
    eval(&mut rt, "(set-session-panes! (current-session) (cdr (session-panes (current-session))) 0)");
    assert!(refused(&mut rt, &format!("(form-invoke! {S} f)")).contains("source pane was closed"));
    assert_eq!(eval(&mut rt, "runs"), "1");
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
