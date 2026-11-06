//! Search ownership shares the syntax enumeration and immutable snapshots.
use std::{fs, path::Path};
use techne_vm::vm::Vm;
fn world(root: &Path) -> Vm {
    let mut vm = Vm::new();
    techne_node::install(&mut vm).unwrap();
    vm.eval_source(&format!("(import (techne tools)) (define p (tool-project {:?})) (define (check a b) (unless (equal? a b) (error \"check\" a b))) (define (fails f) (guard (e (#t #t)) (f) #f))",root.to_str().unwrap())).unwrap();
    vm
}
#[test]
fn rust_hits_find_inner_definitions_and_keep_snapshot_handles() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("code.rs"), "// needle outside\nmod tests {\n #[test]\n fn alpha() { let _ = \"é needle alpha\"; fn inner() { let _ = \"nested-mark\"; } }\n #[test]\n fn beta() { let _ = \"needle beta\"; }\n}\n").unwrap();
    let mut vm = world(dir.path());
    vm.eval_source(
        r##"
      (define r (tool-search p "needle" #:enclosing #t #:limit 50))
      (define hits (tool-ref r 'items))
      (check '(none found found) (map (lambda (i) (tool-ref i 'enclosing-status)) hits))
      (check #t (tool-ref r 'enclosing-complete))
      (check '("alpha" "beta") (map (lambda (i) (tool-ref (tool-ref i 'enclosing) 'name)) (cdr hits)))
      (define owner (tool-ref (cadr hits) 'enclosing))
      (define h (tool-ref owner 'handle))
      (check (tool-ref (cadr hits) 'revision) (tool-ref owner 'revision))
      (check #t (string-prefix? "#[test]" (tool-ref (tool-expand p h) 'text)))
      (check h (tool-ref (car (tool-ref (tool-symbols p "code.rs" #:name "alpha") 'items)) 'handle))
      (define inner (tool-ref (car (tool-ref (tool-search p "nested-mark" #:enclosing #t #:after 1) 'items)) 'enclosing))
      (check '("inner" "local") (list (tool-ref inner 'name) (tool-ref inner 'scope)))
      (check #t (and (string-contains (tool-show r) "enclosing alpha") #t))
      (check #t (and (string-contains (tool-show r) "tool-expand project") #t))
      (string-set! (tool-ref owner 'name) 0 #\X)
      (check #t (and (string-contains (tool-ref (tool-expand p h) 'text) "fn alpha") #t))
    "##,
    )
    .unwrap();
    fs::write(dir.path().join("code.rs"), "changed").unwrap();
    vm.eval_source(r#"(check #t (and (string-contains (tool-ref (tool-expand p h) 'text) "needle alpha") #t))"#).unwrap();
}
#[test]
fn scheme_ownership_is_innermost_and_uses_character_position_not_line() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("code.scm"),
        "(define (outer) (define (inner) \"needle\") (inner))\n(define (a) \"mark-a\") (define (b) \"é mark-b\")\n\"top-level\"\n",
    )
    .unwrap();
    let mut vm = world(dir.path());
    vm.eval_source(
        r#"
      (define (owner query) (tool-ref (car (tool-ref (tool-search p query #:enclosing #t) 'items)) 'enclosing))
      (check '("inner" "local") (let ((x (owner "needle"))) (list (tool-ref x 'name) (tool-ref x 'scope))))
      (check "b" (tool-ref (owner "mark-b") 'name))
      (check #f (owner "top-level"))
      (check "inner" (tool-ref (tool-ref (car (tool-ref (tool-search p "needle") 'items)) 'enclosing) 'name))
      (check #f (tool-ref (car (tool-ref (tool-search p "needle" #:enclosing #f) 'items)) 'enclosing-status))
    "#,
    )
    .unwrap();
}
#[test]
fn unavailable_and_partial_syntax_do_not_claim_no_owner() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("notes.txt"), "needle\n").unwrap();
    fs::write(dir.path().join("broken.scm"), "(define (x) \"needle\"\n").unwrap();
    fs::write(dir.path().join("broken.rs"), "fn x() { let x = \"needle\";\n").unwrap();
    let mut vm = world(dir.path());
    vm.eval_source(
        r#"
      (for-each (lambda (prefix)
        (let* ((r (tool-search p "needle" #:prefix prefix #:enclosing #t)) (i (car (tool-ref r 'items))))
          (check #t (tool-ref r 'complete))
          (check #f (tool-ref r 'enclosing-complete))
          (check #f (tool-ref i 'enclosing-complete)))) '("notes" "broken.scm" "broken.rs"))
      (check 'unavailable (tool-ref (car (tool-ref (tool-search p "needle" #:prefix "notes" #:enclosing #t) 'items)) 'enclosing-status))
      (check #f (tool-ref (car (tool-ref (tool-search p "needle" #:prefix "notes") 'items)) 'enclosing-status))
      (check #t (fails (lambda () (tool-search p "needle" #:group-by 'path #:enclosing #t))))
      (check #t (fails (lambda () (tool-search p "needle" #:enclosing 'bad))))
    "#,
    )
    .unwrap();
}
