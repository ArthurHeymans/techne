//! Languages and moving by their trees: Emacs's keys of its tree-sitter
//! modes, over the runtime's grammars.

use std::{path::Path, time::Instant};

use techne_editor::{present::Input, runtime::Runtime};

fn keys(rt: &mut Runtime, keys: &str) {
    for k in keys.split(' ') {
        rt.handle(Input::Key { key: k.to_string(), at: Instant::now() });
    }
}

fn file(dir: &Path, name: &str, text: &str) -> Runtime {
    std::fs::write(dir.join(name), text).unwrap();
    Runtime::open(&dir.join(name), &dir.join("journal"), "emacs").unwrap().0
}

/// The minibuffer's first column, once it opens: imenu's list comes when
/// its query, on a thread, is back.
fn listed(rt: &mut Runtime) -> Vec<String> {
    let start = std::time::Instant::now();
    loop {
        let s = rt.snapshot();
        if let Some(m) = &s.minibuffer {
            return m.rows.iter().map(|r| r.text(0)).collect();
        }
        assert!(start.elapsed() < std::time::Duration::from_secs(20), "no list: {}", s.echo);
        rt.run_tasks(std::time::Duration::from_millis(20)).unwrap();
    }
}

fn point(rt: &mut Runtime) -> usize {
    rt.eval("(point (current-session))").unwrap().parse().unwrap()
}

fn goto(rt: &mut Runtime, pos: usize) {
    rt.eval(&format!("(view-set-ranges! (session-view (current-session)) (list (list {pos} {pos})) 0)")).unwrap();
}

fn region(rt: &mut Runtime) -> (usize, usize) {
    let r = rt.eval("(let ((r (car (ranges (current-session))))) (list (min (car r) (cadr r)) (max (car r) (cadr r))))").unwrap();
    let n: Vec<usize> = r.trim_matches(['(', ')']).split(' ').map(|n| n.parse().unwrap()).collect();
    (n[0], n[1])
}

const RUST: &str = "struct Point { x: i32 }\n\nimpl Point {\n    fn norm(&self) -> i32 {\n        let v = self.x * 2;\n        v\n    }\n\n    fn zero() -> Self { Point { x: 0 } }\n}\n";

#[test]
fn moving_over_nodes() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = file(dir.path(), "a.rs", RUST);
    let at = |s: &str| RUST.find(s).unwrap();
    // C-M-f over the whole struct item from its start, and back.
    keys(&mut rt, "C-M-f");
    assert_eq!(point(&mut rt), at("\n\nimpl"));
    keys(&mut rt, "C-M-b");
    assert_eq!(point(&mut rt), 0);
    // Inside a call's operands: over `self.x * 2` as one expression.
    goto(&mut rt, at("self.x"));
    keys(&mut rt, "C-M-f");
    assert_eq!(point(&mut rt), at(";\n        v"));
    // Out to the let, out to the block.
    keys(&mut rt, "C-M-u");
    assert_eq!(point(&mut rt), at("let v"));
    keys(&mut rt, "C-M-u");
    assert_eq!(point(&mut rt), at("{\n        let"));
    // Into the parameter list of `norm`, past its parenthesis.
    goto(&mut rt, at("(&self)"));
    keys(&mut rt, "C-M-d");
    assert_eq!(point(&mut rt), at("&self"));
    // Counts, as Emacs takes them: two levels down; backward into the
    // node before point; kill two nodes back.
    goto(&mut rt, at("{ Point { x: 0 }"));
    keys(&mut rt, "C-M-d C-M-d");
    let twice = point(&mut rt);
    goto(&mut rt, at("{ Point { x: 0 }"));
    keys(&mut rt, "C-u 2 C-M-d");
    assert_eq!(point(&mut rt), twice);
    goto(&mut rt, at(" -> Self"));
    keys(&mut rt, "C-u - 1 C-M-d");
    assert_eq!(point(&mut rt), at(") -> Self"));
    let other = tempfile::tempdir().unwrap();
    let mut small = file(other.path(), "c.rs", "fn f() { a; b; c; }\n");
    goto(&mut small, "fn f() { a; b; c".len());
    keys(&mut small, "C-u - 2 C-M-k");
    assert_eq!(small.eval("(document-string (doc (current-session)))").unwrap(), "\"fn f() { a; ; }\\n\"");
}

#[test]
fn definitions() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = file(dir.path(), "a.rs", RUST);
    let at = |s: &str| RUST.find(s).unwrap();
    goto(&mut rt, at("let v"));
    keys(&mut rt, "C-M-a");
    assert_eq!(point(&mut rt), at("fn norm"));
    keys(&mut rt, "C-M-a");
    assert_eq!(point(&mut rt), at("impl Point"));
    keys(&mut rt, "C-M-e");
    assert_eq!(point(&mut rt), at("\n\n    fn zero"));
    goto(&mut rt, at("Self"));
    keys(&mut rt, "C-M-h");
    assert_eq!(region(&mut rt), (at("fn zero"), at("\n}\n")));
    // imenu lists them by name.
    keys(&mut rt, "C-g M-g i");
    assert_eq!(listed(&mut rt), ["Point", "Point", "norm", "zero"]);
}

#[test]
fn expanding_the_selection_by_nodes() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = file(dir.path(), "a.rs", RUST);
    let at = |s: &str| RUST.find(s).unwrap();
    goto(&mut rt, at("self.x") + 1);
    keys(&mut rt, "C-=");
    assert_eq!(region(&mut rt), (at("self.x"), at(".x * 2")));
    keys(&mut rt, "C-=");
    assert_eq!(region(&mut rt), (at("self.x"), at(" * 2")));
    keys(&mut rt, "C-=");
    assert_eq!(region(&mut rt), (at("self.x"), at(";\n        v")));
}

#[test]
fn scheme_definitions_and_the_tree_at_point() {
    let dir = tempfile::tempdir().unwrap();
    let text = "(define (f x) (* x 2))\n(define y 3)\n";
    let mut rt = file(dir.path(), "a.scm", text);
    keys(&mut rt, "C-M-e");
    assert_eq!(point(&mut rt), text.find("\n(define y").unwrap());
    keys(&mut rt, "M-g i");
    assert_eq!(listed(&mut rt), ["f", "y"]);
    keys(&mut rt, "C-g");
    // A call at top level is no definition.
    let other = tempfile::tempdir().unwrap();
    let mut called = file(other.path(), "b.scm", "(define x 1)\n(display x)\n(define-record-type r (r) r?)\n");
    keys(&mut called, "M-g i");
    assert_eq!(listed(&mut called), ["x", "r"]);
    goto(&mut rt, text.find("2))").unwrap());
    rt.eval("(run-command (current-session) 'describe-syntax-at-point 1)").unwrap();
    assert_eq!(rt.snapshot().echo, "program > list > list > number");
}

/// Every capture name of every language's queries is a face, or a
/// dotted specialisation of one: nothing is drawn as an unknown face.
#[test]
fn every_capture_names_a_face() {
    let mut rt = Runtime::with_document(techne_text::Document::new(""), "emacs").unwrap();
    let missing = rt
        .eval(
            "(define (known? name)
               (let loop ((parts (string-split name \".\")))
                 (and (pair? parts)
                      (or (find-face (string->symbol (string-join parts \".\")))
                          (loop (reverse (cdr (reverse parts))))))))
             (append-map (lambda (l)
                           (filter (lambda (n) (not (or (string-prefix? \"_\" n) (known? n))))
                                   (syntax-capture-names (symbol->string l))))
                         (language-names))",
        )
        .unwrap();
    assert_eq!(missing, "()");
}

/// A language is the user's to define: its own queries, its own files.
#[test]
fn a_language_defined_in_lisp() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = file(dir.path(), "a.conf", "[a]\nk = 1\n");
    assert_eq!(rt.eval("(buffer-language (current-buffer (current-session)))").unwrap(), "#f");
    rt.eval("(define-language conf \"Configuration as TOML.\" #:grammar 'toml #:files '(\".conf\") #:highlights \"(integer) @number\")")
        .unwrap();
    assert_eq!(rt.eval("(buffer-language (current-buffer (current-session)))").unwrap(), "conf");
    let s = rt.snapshot();
    assert_eq!(s.pane().layers.len(), 1, "only the number: {:?}", s.pane().layers);
    let err = rt.eval("(define-language bad \"Bad.\" #:grammar 'toml #:highlights \"(nothing) @x\")").unwrap_err();
    assert!(err.to_string().contains("toml"), "{err}");
}
