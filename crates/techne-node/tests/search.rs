//! Bounded Scheme search contexts and grouped matching-line counts.
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
fn grouped_counts_page_files_not_matching_lines() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a"), "top\né needle\n\nneedle tail\nlast\n").unwrap();
    fs::write(dir.path().join("b"), "needle needle\nend\n").unwrap();
    fs::write(dir.path().join("empty"), "").unwrap();
    fs::write(dir.path().join("binary"), b"\xffneedle\n").unwrap();
    let mut vm = world(dir.path());
    vm.eval_source(
        r#"
      (define counts (tool-search p "needle" #:group-by 'path #:limit 1))
      (check '(3 2 matching-lines #t #t 1 1)
        (map (lambda (k) (tool-ref counts k)) '(total files unit complete truncated next binary)))
      (check '(("a" 2))
        (map (lambda (i) (list (tool-ref i 'path) (tool-ref i 'count))) (tool-ref counts 'items)))
      (check #f (tool-ref (car (tool-ref counts 'items)) 'handle))
      (check #t (and (string-contains (tool-show counts) "1 of 2; more with #:offset 1") #t))
      (check #t (and (string-contains (tool-show counts) "3 matching lines in 2 files") #t))
      (define second (tool-search p "needle" #:group-by 'path #:offset (tool-ref counts 'next)))
      (check '(3 2 #f #f) (map (lambda (k) (tool-ref second k)) '(total files next truncated)))
      (check "b: 1 matching line\n[3 matching lines in 2 files]" (tool-show second))
      (check '() (tool-ref (tool-search p "needle" #:group-by 'path #:offset 100) 'items))
      (define none (tool-search p "absent" #:group-by 'path))
      (check '(0 0 #t #f) (map (lambda (k) (tool-ref none k)) '(total files complete next)))
      (string-set! (tool-ref (car (tool-ref counts 'items)) 'path) 0 #\X)
      (check "a" (tool-ref (car (tool-ref (tool-search p "needle" #:group-by 'path) 'items)) 'path))
    "#,
    )
    .unwrap();
}

#[test]
fn context_is_bounded_unicode_snapshot_text_and_preserves_hit_handles() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a"), "top\né needle\n\nneedle tail\nlast\n").unwrap();
    fs::write(dir.path().join("b"), "needle needle\nend\n").unwrap();
    fs::write(dir.path().join("long"), format!("{}needle\n", "é".repeat(300))).unwrap();
    let mut vm = world(dir.path());
    vm.eval_source(
        r#"
      (define plain (tool-search p "needle" #:prefix "a"))
      (check #f (tool-ref (car (tool-ref plain 'items)) 'context-handle))
      (define found (tool-search p "needle" #:prefix "a" #:before 1 #:after 1))
      (define hit (car (tool-ref found 'items)))
      (define context (tool-ref hit 'context))
      (define h (tool-ref hit 'context-handle))
      (check '(2 3 9)
        (list (tool-ref hit 'line) (tool-ref hit 'column)
              (tool-ref (tool-expand p (tool-ref hit 'handle)) 'characters)))
      (check "é needle\n" (tool-ref (tool-expand p (tool-ref hit 'handle)) 'text))
      (check "top\né needle\n\n" (tool-ref context 'text))
      (check (tool-ref hit 'revision) (tool-ref context 'revision))
      (check #f (tool-ref found 'context-truncated))
      (check #t (and (string-contains (tool-show found) "]\n1: top\n2: é needle\n3: ") #t))
      (check #t (and (string-contains (tool-show found) (tool-ref context 'revision)) #t))
      (check #t (and (string-contains (tool-show found) "; \"a\"; revision ") #t))
      (check #f (and (string-contains (tool-show found) "more file lines") #t))
      (check "" (tool-ref (tool-expand p h #:offset 14) 'text))
      (check #t (fails (lambda () (tool-expand p h #:offset 15))))
      (define clipped (tool-search p "needle" #:before 1 #:after 1 #:chars 2))
      (check 2 (tool-ref clipped 'context-characters))
      (check #t (tool-ref clipped 'context-truncated))
      (check "to" (tool-ref (tool-ref (car (tool-ref clipped 'items)) 'context) 'text))
      (check #f (tool-ref (cadr (tool-ref clipped 'items)) 'context))
      (check #t (and (string-contains (tool-show clipped) "context omitted") #t))
      (check #t (and (string-contains (tool-show clipped) "context clipped") #t))
      (define wide (tool-search p "needle" #:prefix "long" #:before 1 #:chars 1))
      (define wide-hit (car (tool-ref wide 'items)))
      (check 301 (tool-ref wide-hit 'column))
      (check "é" (tool-ref (tool-ref wide-hit 'context) 'text))
      (check 307 (tool-ref (tool-ref wide-hit 'context) 'characters))
      ;; Returned nested strings cannot mutate the cached context snapshot.
      (string-set! (tool-ref context 'text) 0 #\X)
      (check "top\né needle\n\n" (tool-ref (tool-expand p h) 'text))
    "#,
    )
    .unwrap();
    fs::write(dir.path().join("a"), "changed").unwrap();
    vm.eval_source(r#"(check "top\né needle\n\n" (tool-ref (tool-expand p h) 'text))"#).unwrap();
}

#[test]
fn invalid_search_options_are_actionable() {
    let dir = tempfile::tempdir().unwrap();
    let mut vm = world(dir.path());
    vm.eval_source(
        r#"
      (for-each (lambda (query) (check #t (fails query)))
        (list (lambda () (tool-search p "x" #:before -1))
              (lambda () (tool-search p "x" #:after 9))
              (lambda () (tool-search p "x" #:after 1.5))
              (lambda () (tool-search p "x" #:chars 0))
              (lambda () (tool-search p "x" #:group-by 'unknown))
              (lambda () (tool-search p "x" #:group-by 'path #:after 1))))
    "#,
    )
    .unwrap();
}

#[test]
fn incomplete_grouped_totals_are_not_reported_as_complete() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a"), "needle\n").unwrap();
    fs::write(dir.path().join("huge"), vec![b'x'; 1_048_577]).unwrap();
    let mut vm = world(dir.path());
    vm.eval_source(
        r#"
      (define partial (tool-search p "needle" #:group-by 'path))
      (check '(1 1 #f #t 1) (map (lambda (k) (tool-ref partial k)) '(total files complete truncated skipped)))
      (check #t (and (string-contains (tool-show partial) "incomplete: skipped 1") #t))
    "#,
    )
    .unwrap();
}

#[test]
fn zero_hits_explain_literal_semantics_and_scope_without_reinterpreting_input() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("data"), "alpha\nbeta\n").unwrap();
    fs::write(dir.path().join("pipes"), "alpha|beta\n").unwrap();
    fs::write(dir.path().join("huge"), vec![b'x'; 1_048_577]).unwrap();
    let mut vm = world(dir.path());
    vm.eval_source(
        r#"
      (define query "alpha|beta")
      (define missing (tool-search p query #:prefix "data" #:after 1))
      (check '(literal "alpha|beta" "data" 0 1 #t #f #f)
        (map (lambda (k) (tool-ref missing k)) '(match-mode query prefix total scanned complete query-truncated prefix-truncated)))
      (define shown (tool-show missing))
      (for-each (lambda (s) (check #t (and (string-contains shown s) #t)))
        '("literal query \"alpha|beta\"" "prefix \"data\"" "scanned 1 text file"
          "Regex operators are not interpreted" "batch separate literal queries"))
      ;; Pipes remain valid literal characters; successful hits do not get warnings.
      (define literal (tool-search p query #:prefix "pipes"))
      (check 1 (tool-ref literal 'total))
      (check #f (and (string-contains (tool-show literal) "Regex operators") #t))
      ;; An empty page with known hits is not a zero-hit search.
      (check #f (and (string-contains (tool-show (tool-search p "alpha" #:prefix "data" #:offset 20)) "0 matches") #t))
      (string-set! (tool-ref missing 'query) 0 #\X)
      (check "alpha|beta" query)
      ;; Echoes are bounded too, without changing the actual query or prefix.
      (define long (tool-search p (make-string 400 #\é) #:prefix (make-string 400 #\x)))
      (check '(#t #t) (map (lambda (k) (tool-ref long k)) '(query-truncated prefix-truncated)))
      (check #t (<= (string-length (tool-ref long 'query)) 240))
      (check #t (<= (string-length (tool-ref long 'prefix)) 240))
      (check #t (and (string-contains (tool-show long) "query clipped") #t))
      (check #t (and (string-contains (tool-show long) "prefix clipped") #t))
      (define partial (tool-search p query #:prefix "huge"))
      (check #f (tool-ref partial 'complete))
      (check #t (and (string-contains (tool-show partial) "incomplete: skipped 1") #t))
      (check #t (and (string-contains (tool-show partial) "scanned 0 text files") #t))
    "#,
    )
    .unwrap();
}
