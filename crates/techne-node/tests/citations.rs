//! Citation bounds/revisions are Scheme policy, not semantic verification.
use std::{fs, path::Path};
use techne_vm::vm::Vm;

fn world(root: &Path) -> Vm {
    let mut vm = Vm::new();
    techne_node::install(&mut vm).unwrap();
    vm.eval_source(&format!(
        "(import (techne tools)) (define p (tool-project {:?}))\n\
         (define (check expected actual) (unless (equal? expected actual) (error \"check\" expected actual)))\n\
         (define (fails thunk) (guard (e (#t #t)) (thunk) #f))",
        root.to_str().unwrap()
    ))
    .unwrap();
    vm
}

#[test]
fn citation_bounds_are_explicit_and_not_claim_verification() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("doc"), "one\né two\n").unwrap();
    fs::write(dir.path().join("empty"), "").unwrap();
    fs::write(dir.path().join("binary"), b"\xff").unwrap();
    let mut vm = world(dir.path());
    vm.eval_source(
        r#"
      (define c (tool-cite p "doc" #:from 1 #:to 2))
      (check '(citation #t #f 2 "doc:1-2")
        (map (lambda (k) (tool-ref c k)) '(kind valid reason file-lines citation)))
      (check "doc:2" (tool-ref (tool-cite p "doc" #:from 2) 'citation))
      (check #t (and (string-contains (tool-show c) "claim support not checked") #t))
      (define bad (tool-cite p "doc" #:from 110 #:to 118))
      (check '(#f past-end 2 #f)
        (map (lambda (k) (tool-ref bad k)) '(valid reason file-lines citation)))
      (check #t (and (string-contains (tool-show bad) "snapshot file-lines 2") #t))
      (check #t (tool-ref (tool-cite p "empty") 'valid))
      (check #t (fails (lambda () (tool-cite p "doc" #:from 0))))
      (check #t (fails (lambda () (tool-cite p "doc" #:from 2 #:to 1))))
      (check #t (fails (lambda () (tool-cite p "doc" #:from 1.5))))
      (check #t (fails (lambda () (tool-cite p "doc" #:to 2147483648))))
      (check #t (fails (lambda () (tool-cite p "doc" #:revision 'bad))))
      (check #t (fails (lambda () (tool-cite p "binary"))))
      (check #t (fails (lambda () (tool-cite p "absent"))))
      (check #t (fails (lambda () (tool-cite p "../outside"))))
    "#,
    )
    .unwrap();
}

#[test]
fn citation_revision_checks_current_disk_but_old_views_keep_their_line_count() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("doc"), "one\ntwo\n").unwrap();
    let mut vm = world(dir.path());
    vm.eval_source(
        r#"
      (define old (tool-read p "doc"))
      (define rev (tool-ref old 'revision))
      (check #t (tool-ref (tool-cite p "doc" #:to 2 #:revision rev) 'valid))
      (check (string-append "[snapshot file-lines: 2; \"doc\"; revision " rev "]\n1: one\n2: two") (tool-show old))
      (check "one\ntwo" (tool-show old #:numbered #f))
    "#,
    )
    .unwrap();
    fs::write(dir.path().join("doc"), "changed").unwrap();
    vm.eval_source(
        r#"
      (define stale (tool-cite p "doc" #:to 2 #:revision rev))
      (check '(#f stale-revision 1)
        (map (lambda (k) (tool-ref stale k)) '(valid reason file-lines)))
      (check #f (tool-ref (tool-cite p "doc" #:from 2) 'valid))
      (check #t (tool-ref (tool-cite p "doc") 'valid))
      (check #t (string-prefix? "[snapshot file-lines: 2;" (tool-show old)))
      (check #t (and (string-contains (tool-show (tool-expand p (tool-ref old 'handle))) rev) #t))
      (check #t (string-prefix? "[snapshot file-lines: 1;" (tool-show (tool-read p "doc"))))
    "#,
    )
    .unwrap();
}

#[test]
fn citation_checks_do_not_evict_source_handles_and_replies_own_strings() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("doc"), "original").unwrap();
    for i in 0..130 {
        fs::write(dir.path().join(format!("c{i}")), "note\n").unwrap();
    }
    let mut vm = world(dir.path());
    vm.eval_source(
        r#"
      (define r (tool-read p "doc"))
      (let loop ((i 0))
        (when (< i 130)
          (check #t (tool-ref (tool-cite p (string-append "c" (number->string i))) 'valid))
          (loop (+ i 1))))
      (check "original" (tool-ref (tool-expand p (tool-ref r 'handle)) 'text))
      (define path "doc")
      (define c (tool-cite p path))
      (string-set! (tool-ref c 'path) 0 #\X)
      (string-set! (tool-ref c 'revision) 0 #\X)
      (check "doc" path)
      (check "doc:1" (tool-ref (tool-cite p "doc") 'citation))
      (check (tool-ref r 'revision) (tool-ref (tool-cite p "doc") 'revision))
    "#,
    )
    .unwrap();
}
