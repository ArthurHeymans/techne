//! Operations (doc/operations.md, "Order of work", step 1): every command
//! run is one, its changes name it, journal and all, and it holds the
//! work it starts (a request, a minibuffer) until that has settled.

use std::time::{Duration, Instant};

use techne_editor::{present::Input, runtime::Runtime};
use techne_text::Document;

fn runtime(text: &str) -> Runtime {
    let mut rt = Runtime::with_document(Document::new(text), "emacs").unwrap();
    watch(&mut rt);
    rt
}

/// Record each operation that ends, newest first, in `ended`:
/// (serial command outcome parent committed?).
fn watch(rt: &mut Runtime) {
    rt.eval(
        "(define ended '())
         (add-hook! 'operation-ended 'test
           (lambda (s op)
             (set! ended (cons (list (operation-serial op) (operation-command op) (operation-outcome op)
                                     (operation-parent-serial op) (operation-committed? op))
                               ended))))
         (define (ended-of name) (filter (lambda (e) (eq? (cadr e) name)) ended))
         ;; The operation the last change of the session's document was made in.
         (define (last-change-operation)
           (let ((d (session-document (current-session))))
             (document-operation d (document-revision d))))",
    )
    .unwrap();
}

fn keys(rt: &mut Runtime, keys: &str) {
    for k in keys.split(' ') {
        rt.handle(Input::Key { key: k.to_string(), at: Instant::now() });
    }
}

fn eval(rt: &mut Runtime, source: &str) -> String {
    rt.eval(source).unwrap_or_else(|e| panic!("{source}: {e}"))
}

/// Run the editor's tasks until `done` holds of SOURCE's value.
fn until(rt: &mut Runtime, source: &str, done: impl Fn(&str) -> bool) -> String {
    let start = Instant::now();
    loop {
        let v = eval(rt, source);
        if done(&v) {
            return v;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "timed out: {source} is {v}");
        rt.run_tasks(Duration::from_millis(5)).unwrap();
    }
}

#[test]
fn every_command_run_is_an_operation_and_its_changes_name_it() {
    let mut rt = runtime("abc");
    keys(&mut rt, "x");
    // The key's operation: done, with a change, which names it.
    let typed = eval(&mut rt, "(car (ended-of 'self-insert))");
    assert!(typed.ends_with("self-insert done #f #t)"), "{typed}");
    let serial = typed.trim_start_matches('(').split(' ').next().unwrap().to_string();
    assert_eq!(eval(&mut rt, "(last-change-operation)"), format!("({} {serial})", eval(&mut rt, "(operation-runtime)")));
    // A move changes nothing; *Messages* changes in no operation.
    keys(&mut rt, "C-f");
    assert!(eval(&mut rt, "(car (ended-of 'forward-char))").ends_with("forward-char done #f #f)"));
    eval(&mut rt, "(define-command (say s n) \"Say hi.\" (message! s \"hi\"))");
    eval(&mut rt, "(run-command (current-session) 'say 1)");
    assert!(eval(&mut rt, "(car (ended-of 'say))").ends_with("say done #f #f)"));
    assert_eq!(
        eval(&mut rt, "(let ((d (buffer-document (buffer-named \"*Messages*\")))) (document-operation d (document-revision d)))"),
        "#f"
    );
}

#[test]
fn a_command_run_by_a_command_is_its_child() {
    let mut rt = runtime("abc");
    eval(&mut rt, "(define-command (twice s n) \"Move twice.\" (run-command s 'forward-char 1) (run-command s 'forward-char 1))");
    eval(&mut rt, "(run-command (current-session) 'twice 1)");
    let twice = eval(&mut rt, "(car (car (ended-of 'twice)))");
    let moves = eval(&mut rt, "(map cadddr (ended-of 'forward-char))");
    assert_eq!(moves, format!("({twice} {twice})"));
    // Run from M-x, a command is a child of M-x's operation, which ends
    // when it has.
    keys(&mut rt, "M-x f o r w a r d - c h a r RET");
    let mx = eval(&mut rt, "(car (ended-of 'execute-extended-command))");
    assert!(mx.contains("execute-extended-command done"), "{mx}");
    let parent = eval(&mut rt, "(cadddr (car (ended-of 'forward-char)))");
    assert_eq!(parent, mx.trim_start_matches('(').split(' ').next().unwrap());
}

#[test]
fn a_failure_ends_failed_or_partial() {
    let mut rt = runtime("abc");
    eval(&mut rt, "(define-command (boom s n) \"Fail.\" (error \"boom\"))");
    eval(&mut rt, "(define-command (half s n) \"Change, then fail.\" (insert-text! s \"!\" \"new\") (error \"half\"))");
    eval(&mut rt, "(define-command (raise-false s n) \"Raise #f.\" (raise #f))");
    eval(&mut rt, "(run-command (current-session) 'raise-false 1)");
    assert!(eval(&mut rt, "(car (ended-of 'raise-false))").contains("raise-false failed"), "#f raised is a failure too");
    eval(&mut rt, "(run-command (current-session) 'boom 1)");
    eval(&mut rt, "(run-command (current-session) 'half 1)");
    assert!(eval(&mut rt, "(car (ended-of 'boom))").contains("boom failed"));
    assert!(eval(&mut rt, "(car (ended-of 'half))").contains("half partial"));
    // The change made before it failed stays, and names it.
    assert_eq!(eval(&mut rt, "(document-string (session-document (current-session)))"), "\"!abc\"");
    let half = eval(&mut rt, "(car (car (ended-of 'half)))");
    assert!(eval(&mut rt, "(last-change-operation)").ends_with(&format!(" {half})")));
}

/// A command whose change comes later, from a request answered when the
/// test puts a value in `answers`.
const LATER: &str = "
  (define answers (make-channel 4))
  (define slot (make-request-slot))
  (define (later s n)
    (request! slot (lambda () (let ((v (channel-recv answers))) (if (eq? v 'fail) (error \"no answer\") v)))
              (lambda (text) (insert-text! s text \"new\"))
              #:fail (lambda (e) (message! s (error-text e)))))
  (register-command! 'later \"Insert what comes, later.\" later)";

#[test]
fn a_request_holds_its_operation_and_its_change_names_it() {
    let mut rt = runtime("abc");
    eval(&mut rt, LATER);
    eval(&mut rt, "(run-command (current-session) 'later 1)");
    // Pending while the request waits; keys meanwhile are their own.
    assert_eq!(eval(&mut rt, "(ended-of 'later)"), "()");
    keys(&mut rt, "y");
    assert!(eval(&mut rt, "(car (ended-of 'self-insert))").contains("done"));
    eval(&mut rt, "(channel-send answers \"!\")");
    let later = until(&mut rt, "(ended-of 'later)", |v| v != "()");
    assert!(later.contains("later done #f #t"), "{later}");
    assert_eq!(eval(&mut rt, "(document-string (session-document (current-session)))"), "\"y!abc\"");
    let serial = eval(&mut rt, "(car (car (ended-of 'later)))");
    assert!(eval(&mut rt, "(last-change-operation)").ends_with(&format!(" {serial})")));
}

#[test]
fn a_superseded_request_cancels_and_a_failed_one_fails() {
    let mut rt = runtime("abc");
    eval(&mut rt, LATER);
    eval(&mut rt, "(run-command (current-session) 'later 1)");
    eval(&mut rt, "(run-command (current-session) 'later 1)");
    // The first, superseded before it changed anything, is cancelled.
    let first = until(&mut rt, "(ended-of 'later)", |v| v != "()");
    assert!(first.contains("later cancelled #f #f"), "{first}");
    eval(&mut rt, "(channel-send answers 'fail)");
    let both = until(&mut rt, "(length (ended-of 'later))", |v| v == "2");
    assert_eq!(both, "2");
    assert!(eval(&mut rt, "(car (ended-of 'later))").contains("later failed #f #f"));
}

#[test]
fn shutting_its_scope_cancels_it_and_nothing_commits_while_it_ends() {
    let mut rt = runtime("abc");
    eval(&mut rt, LATER);
    // A command of a scope of its own, as a package's is.
    eval(&mut rt, "(define sc (make-scope 'test)) (with-scope sc (register-command! 'scoped \"Later, in a scope.\" later))");
    eval(&mut rt, "(run-command (current-session) 'scoped 1)");
    eval(&mut rt, "(scope-shutdown! sc)");
    let ended = until(&mut rt, "(ended-of 'scoped)", |v| v != "()");
    assert!(ended.contains("scoped cancelled"), "{ended}");
    // An operation failing: what it still holds is stopped, and nothing
    // commits in it until it has ended.
    let refused = eval(
        &mut rt,
        "(let* ((op (start-operation! 'held (current-session))) (hold (operation-hold! op)))
           (operation-failed! op 'test)
           (let ((refused (guard (e (#t (error-text e)))
                            (call-in-operation op (lambda () (insert-text! (current-session) \"no\" \"new\") \"made\")))))
             (operation-release! op hold)
             refused))",
    );
    assert!(refused.contains("refused: its operation (held"), "{refused}");
    assert!(eval(&mut rt, "(car (ended-of 'held))").contains("held failed"));
    assert_eq!(eval(&mut rt, "(document-string (session-document (current-session)))"), "\"abc\"");
}

#[test]
fn an_aborted_prompt_cancels_its_command_and_an_accepted_one_runs_in_it() {
    let mut rt = runtime("abc");
    eval(
        &mut rt,
        "(define-command (ask s n) \"Insert what is typed.\"
           (completing-read s \"Text: \" '() #:require-match #f #:accept (lambda (s c) (insert-text! s (candidate-text c) \"new\"))))",
    );
    eval(&mut rt, "(run-command (current-session) 'ask 1)");
    assert_eq!(eval(&mut rt, "(ended-of 'ask)"), "()");
    keys(&mut rt, "h i RET");
    let ask = eval(&mut rt, "(car (ended-of 'ask))");
    assert!(ask.contains("ask done #f #t"), "{ask}");
    let serial = ask.trim_start_matches('(').split(' ').next().unwrap().to_string();
    assert!(eval(&mut rt, "(last-change-operation)").ends_with(&format!(" {serial})")));
    // The minibuffer's own keys are no operations.
    assert_eq!(eval(&mut rt, "(ended-of 'minibuffer-accept)"), "()");
    eval(&mut rt, "(run-command (current-session) 'ask 1)");
    keys(&mut rt, "C-g");
    assert!(eval(&mut rt, "(car (ended-of 'ask))").contains("ask cancelled"));
}

#[test]
fn changes_name_their_operation_after_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (path, journal) = (dir.path().join("f.txt"), dir.path().join("f.journal"));
    std::fs::write(&path, "x").unwrap();
    let (mut rt, _) = Runtime::open(&path, &journal, "emacs").unwrap();
    watch(&mut rt);
    keys(&mut rt, "y");
    let named = eval(&mut rt, "(last-change-operation)");
    assert!(named.starts_with('('), "{named}");
    drop(rt);
    let (mut rt, _) = Runtime::open(&path, &journal, "emacs").unwrap();
    watch(&mut rt);
    assert_eq!(eval(&mut rt, "(document-string (session-document (current-session)))"), "\"yx\"");
    assert_eq!(eval(&mut rt, "(last-change-operation)"), named);
}

#[test]
fn a_started_request_holds_its_operation_until_it_has_unwound() {
    let mut rt = runtime("abc");
    // A request whose task waits, and whose cleanup would change the text.
    eval(
        &mut rt,
        "(define gate (make-channel 1))
         (define slow-slot (make-request-slot))
         (define slow-op #f)
         (define late #f)
         (define-command (slowly s n) \"Wait, changing the text as it stops.\"
           (set! slow-op (current-operation))
           (request! slow-slot
                     (lambda ()
                       (dynamic-wind (lambda () #f)
                                     (lambda () (channel-recv gate))
                                     (lambda () (set! late (guard (e (#t (error-text e))) (insert-text! s \"late\" \"new\") 'made)))))
                     (lambda (r) #f)))",
    );
    eval(&mut rt, "(run-command (current-session) 'slowly 1)");
    // Its task starts, and waits.
    rt.run_tasks(Duration::from_millis(20)).unwrap();
    eval(&mut rt, "(operation-cancelled! slow-op)");
    assert_eq!(eval(&mut rt, "(operation-phase slow-op)"), "ending", "held until its task has unwound");
    let ended = until(&mut rt, "(ended-of 'slowly)", |v| v != "()");
    assert!(ended.contains("slowly cancelled #f #f"), "{ended}");
    assert!(eval(&mut rt, "late").contains("refused"), "its cleanup committed nothing");
    assert_eq!(eval(&mut rt, "(document-string (session-document (current-session)))"), "\"abc\"");
}

#[test]
fn a_prompt_ends_its_operation_however_it_closes() {
    let mut rt = runtime("abc");
    // Undoing its previews raises: the operation is cancelled all the same.
    eval(&mut rt, "(define-command (ask s n) \"Ask.\" (completing-read s \"? \" '(\"a\") #:abort (lambda (s) (error \"abort failed\"))))");
    eval(&mut rt, "(run-command (current-session) 'ask 1)");
    keys(&mut rt, "C-g");
    assert!(eval(&mut rt, "(car (ended-of 'ask))").contains("ask cancelled"));
    // Shutting the scope of the command reading aborts the reading.
    eval(&mut rt, "(define sc (make-scope 'test)) (with-scope sc (register-command! 'scoped-ask \"Ask, in a scope.\" (command 'ask)))");
    eval(&mut rt, "(run-command (current-session) 'scoped-ask 1)");
    assert_eq!(eval(&mut rt, "(and (sget (current-session) (quote minibuffer)) #t)"), "#t");
    let _ = rt.eval("(scope-shutdown! sc)");
    assert_eq!(eval(&mut rt, "(and (sget (current-session) (quote minibuffer)) #t)"), "#f");
    assert!(eval(&mut rt, "(car (ended-of 'scoped-ask))").contains("scoped-ask cancelled"));
}

#[test]
fn hooks_run_in_no_operation() {
    let mut rt = runtime("abc");
    eval(
        &mut rt,
        "(define-hook test-hook \"For a test: (session).\")
         (add-hook! 'test-hook 'insert (lambda (s) (insert-text! s \"!\" \"new\")))
         (define-command (hooked s n) \"Run the test hook.\" (run-hook! s 'test-hook))",
    );
    eval(&mut rt, "(run-command (current-session) 'hooked 1)");
    assert!(eval(&mut rt, "(car (ended-of 'hooked))").contains("hooked done #f #f"));
    assert_eq!(eval(&mut rt, "(last-change-operation)"), "#f");
}

#[test]
fn saving_is_an_effect_of_its_operation() {
    let dir = tempfile::tempdir().unwrap();
    let (path, journal) = (dir.path().join("f.txt"), dir.path().join("f.journal"));
    std::fs::write(&path, "x").unwrap();
    let (mut rt, _) = Runtime::open(&path, &journal, "emacs").unwrap();
    watch(&mut rt);
    keys(&mut rt, "y C-x C-s");
    assert!(eval(&mut rt, "(car (ended-of 'save-buffer))").contains("save-buffer done #f #t"), "a write is a change it made");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "yx");
    // While an operation ends, it writes nothing either.
    keys(&mut rt, "z");
    let refused = eval(
        &mut rt,
        "(let* ((op (start-operation! 'held (current-session))) (hold (operation-hold! op)))
           (operation-failed! op 'test)
           (let ((refused (guard (e (#t (error-text e)))
                            (call-in-operation op (lambda () (document-save! (session-document (current-session))) \"saved\")))))
             (operation-release! op hold)
             refused))",
    );
    assert!(refused.contains("refused"), "{refused}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "yx");
}

#[test]
fn an_action_chosen_in_the_minibuffer_runs_in_an_operation() {
    let mut rt = runtime("abc");
    eval(
        &mut rt,
        "(define-action test-kind (put-it s v) \"Insert V.\" (insert-text! s v \"new\"))
         (define-command (choose s n) \"Choose a thing.\"
           (completing-read s \"Thing: \" (list (candidate \"x\" #:target (target 'test-kind \"!\")))))",
    );
    eval(&mut rt, "(run-command (current-session) 'choose 1)");
    keys(&mut rt, "C-; p u t - i t RET");
    assert_eq!(eval(&mut rt, "(document-string (session-document (current-session)))"), "\"!abc\"");
    let act = eval(&mut rt, "(car (car (ended-of 'minibuffer-act)))");
    assert!(eval(&mut rt, "(last-change-operation)").ends_with(&format!(" {act})")), "its change names it");
}
