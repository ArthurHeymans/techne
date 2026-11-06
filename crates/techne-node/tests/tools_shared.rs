//! The self-describing Scheme interface and the shared source machinery.
use std::{fs, path::Path};
use techne_vm::vm::{Capability, Grants, Vm};
fn world(root: &Path) -> Vm {
    let mut vm = Vm::new();
    techne_node::install(&mut vm).unwrap();
    vm.eval_source(&format!("(import (techne tools) (techne source) (techne languages)) (define p (tool-project {:?}))\n(define (check expected actual) (unless (equal? expected actual) (error \"check\" expected actual)))\n(define (fails thunk) (guard (e (#t #t)) (thunk) #f))", root.to_str().unwrap())).unwrap();
    vm
}
#[test]
fn replies_do_not_expose_mutable_cache_or_project_state() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("source"), "abc\n").unwrap();
    let mut vm = world(dir.path());
    vm.eval_source(
        r#"
      (define original-root (tool-root p))
      (define root-copy (tool-root p))
      (string-set! root-copy 0 #\X)
      (check original-root (tool-root p))
      (define files (tool-files p))
      (string-set! (car (tool-ref files 'items)) 0 #\X)
      (check '("source") (tool-ref (tool-files p) 'items))
      (define r (tool-read p "source"))
      (define revision (string-copy (tool-ref r 'revision)))
      (string-set! (tool-ref r 'path) 0 #\X)
      (string-set! (tool-ref r 'revision) 0 #\X)
      (string-set! (tool-ref r 'text) 0 #\X)
      (check revision (tool-ref (tool-read p "source") 'revision))
      (check "abc\n" (tool-ref (tool-expand p (tool-ref r 'handle)) 'text))
    "#,
    )
    .unwrap();
}

#[test]
fn help_is_bounded_live_and_available_without_file_or_process_grants() {
    let mut vm = Vm::with_grants(Grants::ALL.without(Capability::Files).without(Capability::Processes));
    techne_node::install(&mut vm).unwrap();
    vm.eval_source(
        r#"
      (import (techne tools))
      (define h (tool-help))
      (unless (< (string-length (tool-show h)) 1800) (error "default help grew unbounded"))
      (unless (string-contains (tool-show h) "\n(tool-read project PATH") (error "help signatures need separate lines"))
      (unless (not (tool-ref h 'recipes)) (error "recipes must be opt-in"))
      (unless (pair? (tool-ref h 'signatures)) (error "missing signatures"))
      (unless (= 22 (length (tool-help 'recipes))) (error "missing detailed recipes"))
      (unless (tool-ref (tool-help 'guide) 'limits) (error "missing opt-in contracts"))
      (unless (string? (tool-ref h 'purpose)) (error "missing purpose"))
      (define (help-fixture x) "A live help fixture." x)
      (unless (string=? "A live help fixture." (tool-ref (tool-help 'help-fixture) 'text)) (error "stale help"))
      (unless (pair? (tool-ref (tool-help "help fixture") 'items)) (error "missing live discovery"))
      (unless (guard (e (#t #t)) (tool-project "/") #f) (error "help must not grant filesystem access"))
    "#,
    )
    .unwrap();
}
#[test]
fn headless_queries_use_editor_language_declarations_and_unicode_coordinates() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("source.py"), "class Thing:\n    def method(self): pass\ndef outside(): pass\n").unwrap();
    fs::write(dir.path().join("unicode.rs"), "// λ\nfn café() {}\n").unwrap();
    let mut vm = world(dir.path());
    vm.eval_source(
        r#"
      (check 'python (language-value (source-language "source.py") 'grammar))
      (check 3 (tool-ref (tool-symbols p "source.py") 'total))
      (check "member" (tool-ref (car (tool-ref (tool-symbols p "source.py" #:name "method") 'items)) 'scope))
      (check "fn café() {}" (tool-ref (tool-read-symbol p "unicode.rs" "café") 'text))
      (define tree (source-parse "rust" "// λ\nfn café() {}\n"))
      (check #t (source-complete? tree))
      (define cs (tool-ref (source-query tree "(function_item) @definition") 'items))
      (check '("definition" 5 17) (car cs))
      (check "fn café() {}" (source-text tree 5 17))
      (check "source_file" (car (source-parent tree (source-node tree 5 17))))
      (check #t (fails (lambda () (source-node tree 4 1))))
      (check #t (fails (lambda () (source-text tree 0 100))))
    "#,
    )
    .unwrap();
}
#[test]
fn expansion_reports_eof_positions_and_library_names_are_canonical() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("line"), "needle\n").unwrap();
    fs::write(dir.path().join("pages"), "é one\n\nthree\nlast").unwrap();
    fs::write(dir.path().join("empty"), "").unwrap();
    fs::write(dir.path().join("blank"), "\n").unwrap();
    fs::write(dir.path().join("inline.rs"), "fn one() {} fn two() {}\n").unwrap();
    fs::write(dir.path().join("library.sld"), "(define-library (example\n  names) (export))").unwrap();
    let mut vm = world(dir.path());
    vm.eval_source(
        r#"
      (define (source-head r)
        (string-append "[snapshot file-lines: " (number->string (tool-ref r 'file-lines))
          "; " (call-with-output-string (lambda (port) (write (tool-ref r 'path) port)))
          "; revision " (tool-ref r 'revision) "]"))
      (define r (tool-read p "line"))
      (check 2 (tool-ref (tool-expand p (tool-ref r 'handle) #:offset 7) 'line))
      (check "" (tool-ref (tool-expand p (tool-ref r 'handle) #:offset 7) 'text))
      (check "(example names)" (tool-ref (car (tool-ref (tool-symbols p "library.sld") 'items)) 'name))
      ;; A complete one-line region is not a complete file.
      (define first (tool-read p "pages" #:lines 1))
      (check '(#t #f 4 2 6)
        (map (lambda (key) (tool-ref first key)) '(region-complete file-eof file-lines next-line characters)))
      (check "é one\n" (tool-ref (tool-expand p (tool-ref first 'handle)) 'text))
      (check (string-append (source-head first) "\n1: é one\n[region complete; more file lines: (tool-read project \"pages\" #:from 2)]")
        (tool-show first #:numbered #t))
      (define clipped (tool-read p "pages" #:lines 1 #:chars 1))
      (check '(#f #f #f 1)
        (map (lambda (key) (tool-ref clipped key)) '(region-complete file-eof next-line next)))
      (define rest (tool-expand p (tool-ref clipped 'handle) #:offset (tool-ref clipped 'next)))
      (check " one\n" (tool-ref rest 'text))
      (check '(#t #f 2) (map (lambda (key) (tool-ref rest key)) '(region-complete file-eof next-line)))
      (check #t (string-prefix? (string-append (source-head clipped) "\n1: é\n[truncated region:") (tool-show clipped #:numbered #t)))
      (define middle (tool-read p "pages" #:from (tool-ref rest 'next-line) #:lines 2))
      (check "\nthree\n" (tool-ref middle 'text))
      (check 4 (tool-ref middle 'next-line))
      (check #t (string-prefix? (string-append (source-head middle) "\n2: \n3: three\n") (tool-show middle #:numbered #t)))
      (define last (tool-read p "pages" #:from (tool-ref middle 'next-line)))
      (check '(#t #t #f) (map (lambda (key) (tool-ref last key)) '(region-complete file-eof next-line)))
      (check (string-append (source-head last) "\n4: last") (tool-show last))
      (check "last" (tool-show last #:numbered #f))
      (check (string-append (source-head last) "\n4: last") (tool-show (tool-batch (list (lambda () last)))))
      (check '(1 #t #t #f)
        (map (lambda (key) (tool-ref (tool-read p "empty") key)) '(file-lines region-complete file-eof next-line)))
      (define blank (tool-read p "blank"))
      (check (string-append (source-head blank) "\n1: ") (tool-show blank))
      (check (source-head r) (tool-show (tool-expand p (tool-ref r 'handle) #:offset 7)))
      (define empty (tool-read p "empty"))
      (check (source-head empty) (tool-show empty))
      ;; A definition ending mid-line must not advertise skipping to another line.
      (define definition (tool-read-symbol p "inline.rs" "one"))
      (check '(#t #f #f)
        (map (lambda (key) (tool-ref definition key)) '(region-complete file-eof next-line)))
      (define range-error (guard (e (#t (condition/report-string e))) (tool-files p #:limit 100)))
      (check #t (and (string-contains range-error "expected 1 to 50") #t))
    "#,
    )
    .unwrap();
    fs::write(dir.path().join("pages"), "changed").unwrap();
    vm.eval_source(
        r#"
      ;; Expansion metadata belongs to the immutable snapshot, not current disk.
      (define old (tool-expand p (tool-ref first 'handle)))
      (check '(4 #f 2) (map (lambda (key) (tool-ref old key)) '(file-lines file-eof next-line)))
      (check (source-head first) (source-head old))
      (check #t (and (string-contains (tool-show old) (tool-ref first 'revision)) #t))
      (check '(1 #t #f)
        (map (lambda (key) (tool-ref (tool-read p "pages") key)) '(file-lines file-eof next-line)))
    "#,
    )
    .unwrap();
    fs::write(dir.path().join("<stdout of sh>"), "actual\nfile\n").unwrap();
    vm.eval_source(
        r#"
      ;; Kept output is not a disk file; do not suggest tool-read on its label.
      (define job (tool-run p "sh" '("-c" "printf 'a\\nb\\n'") #:keep 100))
      (define output (tool-process-result job))
      (define h (tool-ref (tool-ref output 'stdout) 'handle))
      (define output-line (tool-lines p h #:from 1 #:lines 1))
      (check '(#t #f #f #f)
        (map (lambda (key) (tool-ref output-line key)) '(region-complete file-lines file-eof next-line)))
      (check "<stdout of sh>:1: a" (tool-show output-line #:numbered #t))
      ;; Origin is explicit, not guessed from a filename that looks like output.
      (check '(2 2 #f)
        (map (lambda (key) (tool-ref (tool-read p "<stdout of sh>" #:lines 1) key)) '(file-lines next-line file-eof)))
    "#,
    )
    .unwrap();
}
