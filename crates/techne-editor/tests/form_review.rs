//! Regressions from the independent review of command forms and staging.

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

fn runtime() -> Runtime {
    let mut rt = Runtime::with_document(Document::new("one two"), "emacs").unwrap();
    // Continuation is an internal operation API, deliberately exercised
    // here to ensure it cannot bypass proposal preview integrity.
    eval(&mut rt, &format!("(require {:?})", techne_editor::runtime::lisp_dir().join("operations.scm").display().to_string()));
    rt
}

const STAGE: &str = r#"
(define source (session-document (current-session)))
(define v (session-view (current-session)))
(define-command (stage-word s word) "Replace the first word with WORD."
  #:arguments '((word string)) #:effects '(edit)
  (view-edit! v (list (list 0 3 word)) "new"))
(invoke (current-session) 'stage-word (dict 'word "ONE") #:mode 'stage)
(define staged (car (kept-operation-records)))
(define ids (operation-proposals (record-operation staged)))
"#;

#[test]
fn continuing_a_staged_operation_cannot_accept_unreviewed_content() {
    let mut rt = runtime();
    eval(&mut rt, STAGE);
    eval(
        &mut rt,
        r#"
      (define seen (map (lambda (id) (car (proposal-preview id))) ids))
      (define continued (continue-operation! staged (current-session)))
      (call-in-operation continued (lambda () (view-edit! v '((4 7 "TWO")) "new")))
    "#,
    );
    assert_eq!(eval(&mut rt, "(document-revision source)"), "0", "staging does not change the source revision");
    let why = refused(&mut rt, "(accept-operation-proposals! (current-session) staged ids seen)");
    assert!(why.contains("proposals changed"), "unreviewed replacement must require another review: {why}");
    // The native single/batch APIs must not accept an old identity either.
    assert!(refused(&mut rt, "(proposal-accept! (car ids) #:seen (car seen))").contains("decided"));
    assert!(refused(&mut rt, "(proposals-accept! ids seen)").contains("decided"));
    assert_eq!(eval(&mut rt, "(document-string source)"), "\"one two\"");
    assert_eq!(eval(&mut rt, "(proposal-state (car ids))"), "\"superseded\"");
    eval(
        &mut rt,
        r#"
      (define fresh (operation-proposals (record-operation staged)))
      (define fresh-seen (map (lambda (id) (car (proposal-preview id))) fresh))
      (accept-operation-proposals! (current-session) staged fresh fresh-seen)
    "#,
    );
    assert_eq!(eval(&mut rt, "(length fresh)"), "1", "still one current proposal per document");
    assert_eq!(eval(&mut rt, "(document-string source)"), "\"ONE TWO\"");
}

#[test]
fn forgetting_or_evicting_an_operation_discards_its_pending_proposals() {
    for policy in ["forget", "count", "bytes", "both-logs"] {
        let mut rt = runtime();
        eval(&mut rt, STAGE);
        eval(&mut rt, "(define-command (ping s) \"An invocation with no effects.\" #:arguments '() #t)");
        match policy {
            "forget" => {
                eval(&mut rt, "(forget-operation! staged)");
            }
            "count" => {
                eval(&mut rt, "(set! operations-kept-max 1) (invoke (current-session) 'ping (dict))");
            }
            "bytes" => {
                eval(&mut rt, "(set! operations-kept-bytes 0) (invoke (current-session) 'ping (dict))");
            }
            "both-logs" => {
                eval(&mut rt, "(set! operation-trace-max 1) (set! operations-kept-max 1) (invoke (current-session) 'ping (dict))");
            }
            _ => unreachable!(),
        }
        assert_eq!(eval(&mut rt, "(operation-proposals (record-operation staged))"), "()", "{policy}");
        assert_eq!(eval(&mut rt, "(proposals source)"), "()", "{policy}: pending proposals must not be orphaned");
        assert_eq!(eval(&mut rt, "(map proposal-state ids)"), "(\"rejected\")", "{policy}");
        assert_eq!(eval(&mut rt, "(document-string source)"), "\"one two\"");
        // Repeated eviction must release capacity, not exhaust the global
        // 256-proposal limit while all the corresponding records disappear.
        eval(
            &mut rt,
            r#"
          (set! operations-kept-max 1)
          (set! operations-kept-bytes 4194304)
          (do ((i 0 (+ i 1))) ((= i 260))
            (invoke (current-session) 'stage-word (dict 'word "ONE") #:mode 'stage))
        "#,
        );
        assert_eq!(eval(&mut rt, "(length (proposals source))"), "1", "{policy}: capacity is reclaimed");
    }
}

#[test]
fn form_readers_keep_the_source_directory_and_their_declared_initial_value() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let path = project.join("source.scm");
    std::fs::write(&path, "").unwrap();
    let mut rt = Runtime::open(&path, &dir.path().join("journal"), "emacs").unwrap().0;
    eval(
        &mut rt,
        r#"
      (define origin (session-document (current-session)))
      (define here (default-directory (current-session)))
      (define f (open-command-form! (current-session) 'find-file))
      (define form-doc (session-document (current-session)))
    "#,
    );
    keys(&mut rt, "1");
    assert_eq!(rt.snapshot().minibuffer.unwrap().input, format!("{}/", project.display()));
    keys(&mut rt, "C-g");
    assert_eq!(eval(&mut rt, "(document=? (session-document (current-session)) form-doc)"), "#t", "aborting keeps the form focused");
    // Test a declared initial thunk as well as the file reader's implicit
    // directory default. It must execute in the original source pane.
    eval(
        &mut rt,
        r#"
      (sset! (current-session) 'focus 0)
      (define-command (choose-name s name) "Choose NAME."
        #:arguments '((name string))
        #:read `((name prompt #:initial ,(lambda (s args) (default-directory s))))
        name)
      (define other (open-command-form! (current-session) 'choose-name))
    "#,
    );
    keys(&mut rt, "1");
    assert_eq!(rt.snapshot().minibuffer.unwrap().input, format!("{}/", project.display()));
    keys(&mut rt, "C-g");
}

#[test]
fn sensitive_reader_initials_are_never_prefilled() {
    let mut rt = runtime();
    eval(
        &mut rt,
        r#"
      (define-command (secret s password) "Take PASSWORD."
        #:arguments '((password string #:sensitive))
        #:read '((password prompt #:initial "do-not-prefill"))
        #t)
      (open-command-form! (current-session) 'secret)
    "#,
    );
    keys(&mut rt, "1");
    assert_eq!(rt.snapshot().minibuffer.unwrap().input, "");
}

#[test]
fn context_acquisition_conforms_defaults_and_context_values_before_using_them() {
    let mut rt = runtime();
    eval(
        &mut rt,
        r#"
      (define-command (capture s kind choice value) "Capture KIND, CHOICE and VALUE."
        #:arguments '((kind (one-of text number) #:default "text")
                      (choice (one-of text number)) (value integer))
        #:read `((choice context #:value ,(lambda (s args) (if (eq? (dict-ref args 'kind) 'text) "number" "text")))
                 (value context #:value ,(lambda (s args) (if (eq? (dict-ref args 'choice) 'number) 7 "wrong"))))
        (list kind choice value))
      (define f (open-command-form! (current-session) 'capture))
    "#,
    );
    assert_eq!(eval(&mut rt, "(form-problems f)"), "()");
    assert_eq!(eval(&mut rt, "(dict-ref (form-arguments f) 'kind)"), "text");
    assert_eq!(eval(&mut rt, "(dict-ref (form-arguments f) 'choice)"), "number");
    assert_eq!(eval(&mut rt, "(form-invoke! (current-session) f)"), "(text number 7)");
    // Invalid defaults stay editable, but must not be supplied raw to
    // later context readers as though they were validated predecessors.
    eval(
        &mut rt,
        r#"
      (define-command (invalid-context s kind value) "Capture KIND and VALUE."
        #:arguments '((kind (one-of text number) #:default "invalid") (value string))
        #:read `((value context #:value ,(lambda (s args) (symbol->string (dict-ref args 'kind)))))
        value)
      (define bad (open-command-form! (current-session) 'invalid-context))
    "#,
    );
    assert_eq!(eval(&mut rt, "(dict-ref (form-arguments bad) 'kind)"), "\"invalid\"");
    assert_eq!(eval(&mut rt, "(dict-contains? (form-arguments bad) 'value)"), "#f");
    assert!(eval(&mut rt, "(form-problems bad)").contains("kind"));
}
