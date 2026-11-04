//! Recovery and pending decisions (doc/operations.md, "Order of work",
//! step 5): a save refused because the file changed on disk offers to
//! overwrite it, to revert the buffer and to compare them; the failed
//! operation keeps them, a pending decision until one settles it.
//! Choosing is an operation of its own, a child of the one that failed,
//! checked then: after another edit, a choice expecting the text as it
//! was is refused, and a new save offers new ones.

use std::time::Instant;

use techne_editor::{present::Input, runtime::Runtime};

const S: &str = "(current-session)";

fn keys(rt: &mut Runtime, keys: &str) {
    for k in keys.split(' ') {
        rt.handle(Input::Key { key: k.to_string(), at: Instant::now() });
    }
}

fn type_text(rt: &mut Runtime, text: &str) {
    for c in text.chars() {
        rt.handle(Input::Key { key: c.to_string(), at: Instant::now() });
    }
}

fn eval(rt: &mut Runtime, source: &str) -> String {
    rt.eval(source).unwrap_or_else(|e| panic!("{source}: {e}"))
}

fn refused(rt: &mut Runtime, source: &str) -> String {
    eval(rt, &format!("(guard (e (#t (error-text e))) {source} 'ran)"))
}

fn message(rt: &mut Runtime) -> String {
    eval(rt, &format!("(sget {S} 'message)"))
}

/// A runtime on `a.txt`, "base" on disk, with an edit not saved ("x"
/// typed before it) and the file changed since ("external"); a save,
/// refused, is the decision `refused` names.
fn refused_save(dir: &std::path::Path) -> Runtime {
    let path = dir.join("a.txt");
    std::fs::write(&path, "base").unwrap();
    let (mut rt, _) = Runtime::open(&path, &dir.join("a.journal"), "emacs").unwrap();
    type_text(&mut rt, "x");
    std::fs::write(&path, "external").unwrap();
    keys(&mut rt, "C-x C-s");
    eval(
        &mut rt,
        "(define a (session-document (current-session)))
         (define refused (car (kept-operation-records)))
         (define (choice label) (find (lambda (c) (string=? (choice-label c) label)) (record-choices refused)))
         (define (choose label) (choose-recovery! (current-session) refused (choice label)))
         (define (child-of-refused?)
           (let ((r (car (kept-operation-records))))
             (list (record-command r) (eqv? (operation-parent-serial (record-operation r)) (operation-serial (record-operation refused))))))",
    );
    rt
}

fn disk(dir: &std::path::Path) -> String {
    std::fs::read_to_string(dir.join("a.txt")).unwrap()
}

#[test]
fn a_refused_save_offers_choices_and_the_first_chosen_settles_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = refused_save(dir.path());
    let said = message(&mut rt);
    assert!(said.contains("changed on disk") && said.contains("C-c r: Overwrite, Revert, Compare"), "{said}");
    assert_eq!(eval(&mut rt, "(list (record-command refused) (operation-outcome (record-operation refused)))"), "(save-buffer failed)");
    assert_eq!(eval(&mut rt, "(map choice-label (record-choices refused))"), "(\"Overwrite\" \"Revert\" \"Compare\")");
    assert_eq!(eval(&mut rt, &format!("(equal? (pending-decisions {S}) (list refused))")), "#t");

    // Comparing shows the difference, and settles nothing.
    eval(&mut rt, "(choose \"Compare\")");
    let diff = eval(&mut rt, "(document-string (buffer-document (buffer-named \"*diff*\")))");
    // Neither has a final newline.
    assert!(diff.contains("-external\\n\\\\ No newline at end of file\\n+xbase\\n"), "{diff}");
    // Coloured, beside the file's pane: its headers alike, its hunk, what
    // it deletes and what it adds each otherwise.
    let snapshot = rt.snapshot();
    let pane = snapshot.panes.iter().find(|p| p.text.to_string().starts_with("---")).expect("the *diff* pane");
    let styles: Vec<_> = pane.layers.iter().map(|h| h.style).collect();
    assert_eq!(styles.len(), 5, "{:?}", pane.layers);
    assert_eq!(styles[0], styles[1]);
    assert!(styles[1] != styles[2] && styles[2] != styles[3] && styles[3] != styles[4] && styles[2] != styles[4], "{styles:?}");
    assert_eq!(eval(&mut rt, "(record-settled refused)"), "#f");

    eval(&mut rt, "(choose \"Overwrite\")");
    assert_eq!(disk(dir.path()), "xbase");
    assert_eq!(eval(&mut rt, "(document-dirty? a)"), "#f");
    assert_eq!(eval(&mut rt, "(child-of-refused?)"), "(overwrite-file #t)", "a child of the operation that failed");
    assert_eq!(eval(&mut rt, "(eqv? (record-settled refused) (operation-serial (record-operation (car (kept-operation-records)))))"), "#t");
    assert_eq!(eval(&mut rt, &format!("(pending-decisions {S})")), "()");
    // Settled once: another choice is refused, changing nothing.
    let again = refused(&mut rt, "(choose \"Revert\")");
    assert!(again.contains("is settled") && again.contains("settled it"), "{again}");
    assert_eq!(eval(&mut rt, "(document-string a)"), "\"xbase\"");
}

#[test]
fn after_another_edit_a_choice_is_refused_and_a_new_save_offers_new_ones() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = refused_save(dir.path());
    type_text(&mut rt, "y");
    let why = eval(&mut rt, &format!("(choice-applicable {S} refused (choice \"Overwrite\"))"));
    assert!(why.contains("it changed since: revision"), "{why}");
    let chosen = refused(&mut rt, "(choose \"Overwrite\")");
    assert!(chosen.contains("it changed since"), "{chosen}");
    assert_eq!(disk(dir.path()), "external", "nothing written");
    assert_eq!(eval(&mut rt, "(record-settled refused)"), "#f");
    // No choice applies any more: it is no longer pending.
    assert_eq!(eval(&mut rt, &format!("(pending-decisions {S})")), "()");

    // Saving again is refused again, offering choices for the text now.
    keys(&mut rt, "C-x C-s");
    assert_eq!(eval(&mut rt, &format!("(length (pending-decisions {S}))")), "1");
    keys(&mut rt, "C-c r");
    type_text(&mut rt, "Overwrite");
    keys(&mut rt, "RET");
    assert_eq!(disk(dir.path()), "xybase");
    assert_eq!(eval(&mut rt, &format!("(pending-decisions {S})")), "()");
}

#[test]
fn reverting_replaces_unsaved_edits_and_undo_brings_them_back() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = refused_save(dir.path());
    eval(&mut rt, "(choose \"Revert\")");
    assert_eq!(eval(&mut rt, "(list (document-string a) (document-dirty? a))"), "(\"external\" #f)");
    assert_eq!(eval(&mut rt, "(child-of-refused?)"), "(revert-buffer #t)");
    // Its edit is the user's: undo takes it back, and saving then writes
    // over what it read, not refused.
    keys(&mut rt, "C-/");
    assert_eq!(eval(&mut rt, "(document-string a)"), "\"xbase\"");
    keys(&mut rt, "C-x C-s");
    assert_eq!(disk(dir.path()), "xbase");
}

#[test]
fn only_the_user_or_its_principal_chooses() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = refused_save(dir.path());
    eval(
        &mut rt,
        "(define-command (choose-for-it s label) \"Choose LABEL for the refused save.\"
           #:arguments '((label string))
           #:effects '(external)
           #:expose '(agents)
           (choose label))",
    );
    let said = refused(&mut rt, &format!("(invoke {S} 'choose-for-it (dict 'label \"Overwrite\") #:principal \"agent:test\")"));
    assert!(said.contains("Only the user"), "{said}");
    assert_eq!(disk(dir.path()), "external");
    assert_eq!(eval(&mut rt, "(record-settled refused)"), "#f");
}

#[test]
fn decisions_are_listed_and_dismissed() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = refused_save(dir.path());
    keys(&mut rt, "M-x");
    type_text(&mut rt, "decisions");
    keys(&mut rt, "RET");
    let listed = eval(&mut rt, "(document-string (buffer-document (buffer-named \"*decisions*\")))");
    assert!(listed.contains("save-buffer") && listed.contains("Overwrite, Revert, Compare"), "{listed}");
    eval(&mut rt, "(dismiss-decision! refused)");
    assert_eq!(eval(&mut rt, &format!("(pending-decisions {S})")), "()");
    keys(&mut rt, "C-c r");
    assert!(message(&mut rt).contains("Nothing to recover"));
}

#[test]
fn a_unified_diff_as_diff_writes_it() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "").unwrap();
    let (mut rt, _) = Runtime::open(&dir.path().join("a.txt"), &dir.path().join("a.journal"), "emacs").unwrap();
    let diff = |rt: &mut Runtime, a: &str, b: &str| eval(rt, &format!("(unified-diff {a:?} {b:?})"));
    assert_eq!(diff(&mut rt, "a\nb\n", "a\nb\n"), "\"\"");
    assert_eq!(diff(&mut rt, "a\nb\nc\n", "a\nB\nc\nd\n"), "\"--- a\\n+++ b\\n@@ -1,3 +1,4 @@\\n a\\n-b\\n+B\\n c\\n+d\\n\"");
    assert_eq!(diff(&mut rt, "", "new\n"), "\"--- a\\n+++ b\\n@@ -0,0 +1 @@\\n+new\\n\"");
    // Far apart, two hunks; context lines alike around each.
    let a: String = (1..=20).map(|i| format!("{i}\n")).collect();
    let b: String = (1..=20)
        .map(|i| match i {
            2 => "two\n".to_string(),
            19 => "nineteen\n".to_string(),
            _ => format!("{i}\n"),
        })
        .collect();
    assert_eq!(
        diff(&mut rt, &a, &b),
        format!("{:?}", "--- a\n+++ b\n@@ -1,5 +1,5 @@\n 1\n-2\n+two\n 3\n 4\n 5\n@@ -16,5 +16,5 @@\n 16\n 17\n 18\n-19\n+nineteen\n 20\n")
    );
    // The fewest lines deleted and added.
    assert_eq!(
        eval(&mut rt, "(line-diff (vector \"a\" \"b\" \"c\" \"a\" \"b\" \"b\" \"a\") (vector \"c\" \"b\" \"a\" \"b\" \"a\" \"c\"))")
            .matches("(- ")
            .count()
            + eval(&mut rt, "(line-diff (vector \"a\" \"b\" \"c\" \"a\" \"b\" \"b\" \"a\") (vector \"c\" \"b\" \"a\" \"b\" \"a\" \"c\"))")
                .matches("(+ ")
                .count(),
        5
    );
}

/// A runtime whose `fail-offering` command fails offering one choice:
/// `retry` with what it was given, `choose-it` choosing that, as data.
fn offering(dir: &std::path::Path) -> Runtime {
    std::fs::write(dir.join("a.txt"), "").unwrap();
    let (mut rt, _) = Runtime::open(&dir.join("a.txt"), &dir.join("a.journal"), "emacs").unwrap();
    eval(
        &mut rt,
        "(define runs 0)
         (define gate (make-channel))
         (define-command (retry s word) \"Count a run, once GATE lets it be checked.\"
           #:arguments '((word string))
           #:available (lambda (s) (if (eq? (sget s 'gated) #t) (channel-recv gate) #t))
           (set! runs (+ runs 1)))
         (define-command (fail-offering s word password) \"Fail, offering to retry.\"
           #:arguments '((word string) (password string #:sensitive))
           (error-with-choices (list (make-choice \"Retry\" 'retry (dict \"word\" word))
                                     (make-choice \"Retry with it\" 'retry (dict \"word\" password)))
                               \"it failed\"))
         (guard (e (#t #f)) (invoke (current-session) 'fail-offering (dict 'word \"w\" 'password \"top-secret\")))
         (define failed (car (kept-operation-records)))
         (define (choose label)
           (choose-recovery! (current-session) failed
                             (find (lambda (c) (string=? (choice-label c) label)) (record-choices failed))))",
    );
    rt
}

#[test]
fn a_choice_holding_a_sensitive_value_is_not_kept() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = offering(dir.path());
    assert_eq!(eval(&mut rt, "(map choice-label (record-choices failed))"), "(\"Retry\")");
    assert_eq!(eval(&mut rt, "(record-arguments failed)"), "((word . \"w\") (password . \"<redacted>\"))");
}

#[test]
fn two_choosing_while_it_is_checked_run_it_once() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = offering(dir.path());
    // Both check the choice, which waits; the second is refused at once.
    let said = eval(
        &mut rt,
        "(sset! (current-session) 'gated #t)
         (let* ((try (lambda () (guard (e (#t (error-text e))) (choose \"Retry\") 'ran)))
                (a (spawn try)) (b (spawn try)))
           ;; Both run up to the check, which A waits in.
           (yield) (yield)
           (channel-send gate #t)
           (list (task-join a) (task-join b) runs))",
    );
    assert!(said.starts_with("(ran \"") && said.contains("being checked") && said.ends_with(" 1)"), "{said}");
}

#[test]
fn work_an_agent_leaves_chooses_as_the_agent() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = refused_save(dir.path());
    eval(
        &mut rt,
        "(define later (make-channel))
         (define left #f)
         (define-command (leave-work s) \"Choose Overwrite later, for whoever runs this.\"
           #:arguments '()
           #:effects '(external)
           #:expose '(agents)
           (set! left (spawn (lambda () (channel-recv later) (guard (e (#t (error-text e))) (choose \"Overwrite\") 'ran)))))
         (invoke (current-session) 'leave-work (dict) #:principal \"agent:test\")",
    );
    let said = eval(&mut rt, "(channel-send later #t) (task-join left)");
    assert!(said.contains("Only the user"), "{said}");
    assert_eq!(disk(dir.path()), "external");
}

#[test]
fn a_missing_final_newline_differs() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "").unwrap();
    let (mut rt, _) = Runtime::open(&dir.path().join("a.txt"), &dir.path().join("a.journal"), "emacs").unwrap();
    assert_eq!(
        eval(&mut rt, "(unified-diff \"a\\n\" \"a\")"),
        format!("{:?}", "--- a\n+++ b\n@@ -1 +1 @@\n-a\n+a\n\\ No newline at end of file\n")
    );
    assert_eq!(eval(&mut rt, "(unified-diff \"a\" \"a\")"), "\"\"");
}
