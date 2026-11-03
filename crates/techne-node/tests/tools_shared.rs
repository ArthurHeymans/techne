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
      (unless (= 15 (length (tool-ref h 'recipes))) (error "recipes must be one structured value"))
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
    fs::write(dir.path().join("library.sld"), "(define-library (example\n  names) (export))").unwrap();
    let mut vm = world(dir.path());
    vm.eval_source(
        r#"
      (define r (tool-read p "line"))
      (check 2 (tool-ref (tool-expand p (tool-ref r 'handle) #:offset 7) 'line))
      (check "" (tool-ref (tool-expand p (tool-ref r 'handle) #:offset 7) 'text))
      (check "(example names)" (tool-ref (car (tool-ref (tool-symbols p "library.sld") 'items)) 'name))
    "#,
    )
    .unwrap();
}
