//! Compact defaults retain original coordinates, budgets and continuation.
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
fn definition_pages_follow_character_budget_and_explicit_item_ceiling() {
    let dir = tempfile::tempdir().unwrap();
    let source = (0..35).map(|i| format!("(define (λ{i}) {i})\n")).collect::<String>();
    fs::write(dir.path().join("defs.scm"), source).unwrap();
    fs::write(dir.path().join("broken.rs"), "fn good() {}\nfn broken(\n").unwrap();
    let mut vm = world(dir.path());
    vm.eval_source(
        r#"
      (define all (tool-symbols p "defs.scm"))
      (check 35 (length (tool-ref all 'items)))
      (check '(35 #t #f #f #f)
        (map (lambda (k) (tool-ref all k)) '(total complete truncated next budget-exceeded)))
      (define three (tool-symbols p "defs.scm" #:limit 3))
      (check 3 (tool-ref three 'next))
      (check 3 (length (tool-ref three 'items)))
      (check 35 (tool-ref three 'total))
      (define tiny (tool-symbols p "defs.scm" #:chars 1))
      (check 1 (length (tool-ref tiny 'items)))
      (check '(1 #t #t) (map (lambda (k) (tool-ref tiny k)) '(next truncated budget-exceeded)))
      (define second (tool-symbols p "defs.scm" #:chars 1 #:offset (tool-ref tiny 'next)))
      (check "λ1" (tool-ref (car (tool-ref second 'items)) 'name))
      (define expected (map (lambda (i) (tool-ref i 'name)) (tool-ref all 'items)))
      (define paged
        (let loop ((offset 0) (names '()))
          (define r (tool-symbols p "defs.scm" #:chars 100 #:offset offset))
          (check #t (or (<= (tool-ref r 'characters) 100) (tool-ref r 'budget-exceeded)))
          (define more (append names (map (lambda (i) (tool-ref i 'name)) (tool-ref r 'items))))
          (if (tool-ref r 'next) (loop (tool-ref r 'next) more) more)))
      (check expected paged)
      (define end (tool-symbols p "defs.scm" #:offset 100))
      (check '() (tool-ref end 'items))
      (check '(#f #f 0 #f) (map (lambda (k) (tool-ref end k)) '(next truncated characters budget-exceeded)))
      ;; Incomplete syntax remains truncated even after every KNOWN item.
      (define partial (tool-symbols p "broken.rs"))
      (check #f (tool-ref partial 'complete))
      (check #t (tool-ref partial 'truncated))
      (define partial-end (tool-symbols p "broken.rs" #:offset 100))
      (check '(#f #t #f)
        (map (lambda (k) (tool-ref partial-end k)) '(complete truncated next)))
      (for-each (lambda (query) (check #t (fails query)))
        (list (lambda () (tool-symbols p "defs.scm" #:chars 0))
              (lambda () (tool-symbols p "defs.scm" #:chars 16001))
              (lambda () (tool-symbols p "defs.scm" #:limit 51))))
    "#,
    )
    .unwrap();
}

#[test]
fn known_file_defaults_fill_the_bounded_read_without_guessing_eof() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("small"), "é\n".repeat(150)).unwrap();
    fs::write(dir.path().join("long"), "x\n".repeat(250)).unwrap();
    fs::write(dir.path().join("wide"), format!("{}\n", "λ".repeat(100)).repeat(200)).unwrap();
    let mut vm = world(dir.path());
    vm.eval_source(
        r#"
      (define small (tool-read p "small"))
      (check '(150 #t #t #f #f) (map (lambda (k) (tool-ref small k))
        '(file-lines region-complete file-eof next next-line)))
      (check 300 (string-length (tool-ref small 'text)))
      (define long (tool-read p "long"))
      (check '(#t #f 201) (map (lambda (k) (tool-ref long k)) '(region-complete file-eof next-line)))
      (define wide (tool-read p "wide"))
      (check 16000 (string-length (tool-ref wide 'text)))
      (check '(#f #f 16000 #f) (map (lambda (k) (tool-ref wide k)) '(region-complete file-eof next next-line)))
      (check "é\n" (tool-ref (tool-read p "small" #:lines 1) 'text))
    "#,
    )
    .unwrap();
}
