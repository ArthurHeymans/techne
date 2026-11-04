//! Undo of an operation (doc/operations.md, "Order of work", step 4): a
//! command changing two documents, invoked as data by an agent or run
//! from keys, is one operation; undoing it reverses both as one child
//! operation, refused before anything changes when it cannot be, partial
//! and saying where when a journal fails part way.

use std::time::Instant;

use techne_editor::{present::Input, runtime::Runtime};
use techne_text::journal::Fence;

const S: &str = "(current-session)";
const LAST: &str = "(car (kept-operation-records))";

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

/// A runtime on `a.txt`, its session's document `a`, and `b`, `b.txt`
/// opened with its journal under `fence`: revoked, `b`'s journal fails.
fn two_documents(dir: &std::path::Path, fence: &Fence) -> Runtime {
    std::fs::write(dir.join("a.txt"), "alpha beta").unwrap();
    std::fs::write(dir.join("b.txt"), "beta alpha").unwrap();
    let (mut rt, _) = Runtime::open(&dir.join("a.txt"), &dir.join("a.journal"), "emacs").unwrap();
    let before = Fence::default();
    fence.enter();
    eval(&mut rt, &format!("(define b (open-document {:?} {:?}))", dir.join("b.txt"), dir.join("b.journal")));
    before.enter();
    eval(
        &mut rt,
        "(define a (session-document (current-session)))
         (define ended '())
         (add-hook! 'operation-ended 'test
           (lambda (s op)
             (set! ended (cons (list (operation-serial op) (operation-command op) (operation-outcome op)
                                     (operation-parent-serial op))
                               ended))))
         (define (find-in d old)
           (let* ((t (document-string d)) (c (string-contains t old))) (and c (string-cursor->index t c))))
         (define-command (rename-both s old new) \"Rename OLD to NEW in both documents.\"
           #:arguments '((old string) (new string))
           #:effects '(edit)
           #:expose '(agents)
           (for-each (lambda (d)
                       (let ((at (find-in d old)))
                         (view-edit! (make-view d (current-principal)) (list (list at (+ at (string-length old)) new)) \"new\")))
                     (list a b)))
         (define (texts) (list (document-string a) (document-string b)))
         (define (made-in d) (document-operation d (document-revision d)))",
    );
    rt
}

#[test]
fn an_agents_operation_and_a_keyed_one_are_each_undone_as_one_child() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = two_documents(dir.path(), &Fence::default());
    eval(&mut rt, &format!("(invoke {S} 'rename-both (dict 'old \"alpha\" 'new \"ALPHA\") #:principal \"agent:test\")"));
    assert_eq!(eval(&mut rt, "(texts)"), "(\"ALPHA beta\" \"beta ALPHA\")");
    let serial = eval(&mut rt, &format!("(operation-serial (record-operation {LAST}))"));
    let id = format!("({} {serial})", eval(&mut rt, "(operation-runtime)"));
    assert_eq!(eval(&mut rt, "(list (made-in a) (made-in b))"), format!("({id} {id})"), "both name it");
    assert_eq!(eval(&mut rt, &format!("(record-principal {LAST})")), "\"agent:test\"");
    // Its changes are the agent's: the user's undo does not take them.
    let undo = eval(&mut rt, &format!("(operation-serial (undo-operation! {S} (find-operation-record {serial})))"));
    assert_eq!(eval(&mut rt, "(texts)"), "(\"alpha beta\" \"beta alpha\")");
    assert_eq!(
        eval(&mut rt, &format!("(assv {undo} ended)")),
        format!("({undo} undo-operation done {serial})"),
        "one operation, a child of the one undone"
    );
    assert_eq!(eval(&mut rt, "(list (made-in a) (made-in b))"), {
        let u = format!("({} {undo})", eval(&mut rt, "(operation-runtime)"));
        format!("({u} {u})")
    });
    // Undone, there is nothing left of it to undo.
    let why = refused(&mut rt, &format!("(undo-operation! {S} (find-operation-record {serial}))"));
    assert!(why.contains("nothing to undo"), "{why}");

    // From keys: its arguments read in the minibuffer.
    keys(&mut rt, "M-x");
    type_text(&mut rt, "rename-both");
    keys(&mut rt, "RET");
    type_text(&mut rt, "beta");
    keys(&mut rt, "RET");
    type_text(&mut rt, "BETA");
    keys(&mut rt, "RET");
    assert_eq!(eval(&mut rt, "(texts)"), "(\"alpha BETA\" \"BETA alpha\")");
    let keyed = eval(&mut rt, &format!("(operation-serial (record-operation {LAST}))"));
    assert_eq!(eval(&mut rt, &format!("(record-principal {LAST})")), "\"user\"");
    eval(&mut rt, &format!("(undo-operation! {S} (find-operation-record {keyed}))"));
    assert_eq!(eval(&mut rt, "(texts)"), "(\"alpha beta\" \"beta alpha\")");
}

#[test]
fn a_conflicting_or_later_change_refuses_it_before_anything_changes() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = two_documents(dir.path(), &Fence::default());
    eval(&mut rt, &format!("(invoke {S} 'rename-both (dict 'old \"alpha\" 'new \"ALPHA\"))"));
    let serial = eval(&mut rt, &format!("(operation-serial (record-operation {LAST}))"));
    // Someone else changes the renamed text in b, the second document.
    eval(&mut rt, "(view-edit! (make-view b \"other\") '((5 10 \"Alpha!\")) \"new\")");
    let why = refused(&mut rt, &format!("(undo-operation! {S} (find-operation-record {serial}))"));
    assert!(why.contains("b.txt") && why.contains("later edits by other"), "{why}");
    assert_eq!(eval(&mut rt, "(texts)"), "(\"ALPHA beta\" \"beta Alpha!\")", "a not undone either");
    eval(&mut rt, "(view-undo! (make-view b \"other\"))");
    // The principal's own later change, in another operation, is named.
    eval(&mut rt, &format!("(invoke {S} 'rename-both (dict 'old \"beta\" 'new \"BETA\"))"));
    let later = eval(&mut rt, &format!("(operation-serial (record-operation {LAST}))"));
    let why = refused(&mut rt, &format!("(undo-operation! {S} (find-operation-record {serial}))"));
    assert!(why.contains(&format!("changed since, in operation {later}")), "{why}");
    assert_eq!(eval(&mut rt, "(texts)"), "(\"ALPHA BETA\" \"BETA ALPHA\")");
}

#[test]
fn a_journal_failing_part_way_leaves_it_partial_saying_where() {
    let dir = tempfile::tempdir().unwrap();
    let fence = Fence::default();
    let mut rt = two_documents(dir.path(), &fence);
    eval(&mut rt, &format!("(invoke {S} 'rename-both (dict 'old \"alpha\" 'new \"ALPHA\"))"));
    let serial = eval(&mut rt, &format!("(operation-serial (record-operation {LAST}))"));
    // b's journal is taken over: it records nothing more.
    fence.revoke();
    let why = refused(&mut rt, &format!("(undo-operation! {S} (find-operation-record {serial}))"));
    assert!(why.contains("undone in a.txt, not in b.txt") && why.contains("journal"), "{why}");
    assert_eq!(eval(&mut rt, "(texts)"), "(\"alpha beta\" \"beta ALPHA\")");
    let undo = eval(&mut rt, "(car (car ended))");
    assert_eq!(eval(&mut rt, "(cadr (cdr (car ended)))"), "partial", "{undo}");
}

#[test]
fn changes_forgotten_past_the_hundred_or_made_as_two_actors_refuse_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = two_documents(dir.path(), &Fence::default());
    // A hundred changes in a, recorded one by one; two in c, only counted.
    eval(
        &mut rt,
        "(define c (make-document \"xyz\"))
         (define-command (many s n) \"Change a a hundred times, then c twice.\"
           (do ((i 0 (+ i 1))) ((= i 100)) (view-edit! (make-view a \"user\") '((0 0 \"+\")) \"new\"))
           (view-edit! (make-view c \"user\") '((0 0 \"X\")) \"new\")
           (view-edit! (make-view c \"user\") '((1 1 \"Y\")) \"new\"))",
    );
    keys(&mut rt, "M-x");
    type_text(&mut rt, "many");
    keys(&mut rt, "RET");
    let serial = eval(&mut rt, &format!("(operation-serial (record-operation {LAST}))"));
    assert_eq!(eval(&mut rt, "(document-string c)"), "\"XYxyz\"");
    // Untrimmed, the same undoes every change, the counted ones too.
    eval(&mut rt, "(define d (make-document \"xyz\")) (define c-before c) (set! c d)");
    keys(&mut rt, "M-x");
    type_text(&mut rt, "many");
    keys(&mut rt, "RET");
    let whole = eval(&mut rt, &format!("(operation-serial (record-operation {LAST}))"));
    assert_eq!(eval(&mut rt, "(string-length (document-string a))"), "210");
    eval(&mut rt, &format!("(undo-operation! {S} (find-operation-record {whole}))"));
    assert_eq!(eval(&mut rt, "(list (string-length (document-string a)) (document-string d))"), "(110 \"xyz\")");
    eval(&mut rt, "(set! c c-before)");
    eval(&mut rt, "(document-limit-history! c 1)");
    let why = refused(&mut rt, &format!("(undo-operation! {S} (find-operation-record {serial}))"));
    assert!(why.contains("no longer in the history"), "{why}");
    assert_eq!(eval(&mut rt, "(string-length (document-string a))"), "110", "a not undone either");

    // One operation, its changes made as the user in a and as another in b.
    eval(
        &mut rt,
        "(define-command (two-actors s n) \"Change a as the user, b as another.\"
           (view-edit! (make-view a \"user\") '((0 0 \"1\")) \"new\")
           (view-edit! (make-view b \"other\") '((0 0 \"2\")) \"new\"))",
    );
    keys(&mut rt, "M-x");
    type_text(&mut rt, "two-actors");
    keys(&mut rt, "RET");
    let serial = eval(&mut rt, &format!("(operation-serial (record-operation {LAST}))"));
    let undo = eval(&mut rt, &format!("(operation-serial (undo-operation! {S} (find-operation-record {serial})))"));
    // Each document's changes are its one actor's: b's undone as other's.
    assert_eq!(eval(&mut rt, "(substring (document-string b) 0 4)"), "\"beta\"", "{undo}");
    // In one document, two actors: refused.
    eval(
        &mut rt,
        "(define-command (mixed s n) \"Change b as the user and as another.\"
           (view-edit! (make-view b \"user\") '((0 0 \"1\")) \"new\")
           (view-edit! (make-view b \"other\") '((0 0 \"2\")) \"new\"))",
    );
    keys(&mut rt, "M-x");
    type_text(&mut rt, "mixed");
    keys(&mut rt, "RET");
    let serial = eval(&mut rt, &format!("(operation-serial (record-operation {LAST}))"));
    let why = refused(&mut rt, &format!("(undo-operation! {S} (find-operation-record {serial}))"));
    assert!(why.contains("made as user, other"), "{why}");
    assert_eq!(eval(&mut rt, "(substring (document-string b) 0 2)"), "\"21\"");
}

#[test]
fn an_agents_undo_is_the_agents_not_the_users() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = two_documents(dir.path(), &Fence::default());
    eval(
        &mut rt,
        "(define-command (undo-it s n) \"Undo operation N.\"
           #:arguments '((n integer))
           #:effects '(edit)
           #:expose '(agents)
           (operation-serial (undo-operation! s (find-operation-record n))))",
    );
    eval(&mut rt, &format!("(invoke {S} 'rename-both (dict 'old \"alpha\" 'new \"ALPHA\") #:principal \"agent:test\")"));
    let serial = eval(&mut rt, &format!("(operation-serial (record-operation {LAST}))"));
    eval(&mut rt, &format!("(invoke {S} 'undo-it (dict 'n {serial}) #:principal \"agent:test\")"));
    assert_eq!(eval(&mut rt, "(texts)"), "(\"alpha beta\" \"beta alpha\")");
    let undo = eval(&mut rt, "(car (car (filter (lambda (e) (eq? (cadr e) 'undo-operation)) ended)))");
    assert_eq!(eval(&mut rt, &format!("(record-principal (find-operation-record {undo}))")), "\"agent:test\"");
}

#[test]
fn a_forgotten_operation_leaves_the_logs_and_its_changes_still_name_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = two_documents(dir.path(), &Fence::default());
    eval(&mut rt, &format!("(invoke {S} 'rename-both (dict 'old \"alpha\" 'new \"ALPHA\"))"));
    let serial = eval(&mut rt, &format!("(operation-serial (record-operation {LAST}))"));
    assert_eq!(eval(&mut rt, &format!("(forget-operation! (find-operation-record {serial}))")), "#t");
    assert_eq!(eval(&mut rt, &format!("(find-operation-record {serial})")), "#f");
    assert_eq!(eval(&mut rt, "(cadr (made-in a))"), serial);
}
