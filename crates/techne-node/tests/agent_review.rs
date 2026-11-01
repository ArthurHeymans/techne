//! Regression coverage for the independent tool-use review.
use std::{
    fs,
    os::unix::{ffi::OsStrExt, fs::PermissionsExt},
    path::Path,
};
use techne_vm::vm::Vm;

fn world(root: &Path) -> Vm {
    let mut vm = Vm::new();
    techne_node::install(&mut vm).unwrap();
    let library = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lisp/techne/agent.sld");
    vm.eval_source(&format!(
        "(include {:?}) (import (techne agent)) (define p (agent-project {:?}))
         (define (check expected actual) (unless (equal? expected actual) (error \"check\" expected actual)))
         (define (fails thunk) (guard (e (#t #t)) (thunk) #f))",
        library.to_str().unwrap(),
        root.to_str().unwrap()
    ))
    .unwrap();
    vm
}
fn check(vm: &mut Vm, source: &str) {
    vm.eval_source(source).unwrap();
}

#[test]
fn discovery_tracks_module_redefinitions_not_lexical_shadows() {
    let dir = tempfile::tempdir().unwrap();
    let mut vm = world(dir.path());
    check(
        &mut vm,
        r#"
        (eval-source "(define (global-helper x) \"First doc.\" x)" "user" "first.scm")
        (check "First doc." (agent-ref (agent-describe 'global-helper) 'doc))
        (eval-source "(define (global-helper x y) \"Updated doc.\" (+ x y))" "user" "updated.scm")
        (check "(global-helper x y)" (agent-ref (agent-describe 'global-helper) 'signature))
        (check "Updated doc." (agent-ref (agent-describe 'global-helper) 'doc))
        (check 'global-helper (agent-ref (car (agent-ref (agent-discover "global-helper") 'items)) 'name))
        ;; An imported binding redefined at module level is live, too.
        (eval-source "(define (agent-root x y) \"New root doc.\" (+ x y))" "user" "shadow.scm")
        (check "(agent-root x y)" (agent-ref (agent-describe 'agent-root) 'signature))
        (check "New root doc." (agent-ref (agent-describe 'agent-root) 'doc))
        (let ((global-helper (lambda () 'local)))
          (check 'local (global-helper))
          (check "Updated doc." (agent-ref (agent-describe 'global-helper) 'doc)))
    "#,
    );
}

#[test]
fn clipped_signatures_are_explicit_and_can_be_paged() {
    let dir = tempfile::tempdir().unwrap();
    let mut vm = world(dir.path());
    check(
        &mut vm,
        r#"
        (eval-source
          (string-append "(define (wide-helper "
            (string-join (map (lambda (i) (string-append "argument" (number->string i))) (iota 80)) " ")
            ") \"A wide signature.\" #t)") "user" "wide.scm")
        (define first (agent-describe 'wide-helper #:part 'signature #:chars 37))
        (check #t (agent-ref first 'signature-truncated))
        (check #t (agent-ref first 'truncated))
        (check 37 (agent-ref first 'next))
        (check 37 (string-length (agent-ref first 'text)))
        (define actual
          (let loop ((offset 0) (parts '()))
            (let* ((r (agent-describe 'wide-helper #:part 'signature #:chars 37 #:offset offset))
                   (parts (cons (agent-ref r 'text) parts)))
              (if (agent-ref r 'next) (loop (agent-ref r 'next) parts)
                  (apply string-append (reverse parts))))))
        (check (cdr (assq 'signature (binding-description 'wide-helper "user"))) actual)
        (check #t (agent-ref (car (agent-ref (agent-discover "wide-helper") 'items)) 'signature-truncated))
        (check #t (fails (lambda () (agent-describe 'wide-helper #:part 'invalid))))
    "#,
    );
}

#[test]
fn real_lines_do_not_include_a_phantom_trailing_line() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("two"), "one\ntwo\n").unwrap();
    fs::write(dir.path().join("blank"), "one\n\n").unwrap();
    fs::write(dir.path().join("empty"), "").unwrap();
    let mut vm = world(dir.path());
    check(
        &mut vm,
        r#"
        (check "two\n" (agent-ref (agent-read p "two" #:from 2) 'text))
        (check #t (fails (lambda () (agent-read p "two" #:from 3))))
        (check "\n" (agent-ref (agent-read p "blank" #:from 2) 'text))
        (check #t (fails (lambda () (agent-read p "blank" #:from 3))))
        (check "" (agent-ref (agent-read p "empty") 'text))
        (check #t (fails (lambda () (agent-read p "empty" #:from 2))))
    "#,
    );
}

struct Restore(std::path::PathBuf);
impl Drop for Restore {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700));
    }
}

#[test]
fn unreadable_directories_yield_partial_results_instead_of_aborting() {
    if rustix::process::geteuid().is_root() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("allowed"), "needle").unwrap();
    let locked = dir.path().join("locked");
    fs::create_dir(&locked).unwrap();
    fs::write(locked.join("hidden"), "needle").unwrap();
    let _restore = Restore(locked.clone());
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let mut vm = world(dir.path());
    check(
        &mut vm,
        r#"
        (define listing (agent-files p))
        (check #f (agent-ref listing 'complete))
        (check #t (> (agent-ref listing 'skipped) 0))
        (check #t (pair? (agent-ref listing 'problems)))
        (define found (agent-search p "needle"))
        (check 1 (agent-ref found 'total))
        (check #f (agent-ref found 'complete))
        (check #t (pair? (agent-ref found 'problems)))
    "#,
    );
}

#[test]
fn unsupported_file_names_are_explicit_partial_listing_problems() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("allowed"), "text").unwrap();
    fs::write(dir.path().join(std::ffi::OsStr::from_bytes(b"invalid-\xff")), "text").unwrap();
    let mut vm = world(dir.path());
    check(
        &mut vm,
        r#"
        (define listing (agent-files p))
        (check #f (agent-ref listing 'complete))
        (check '("allowed") (agent-ref listing 'items))
        (check 1 (agent-ref listing 'skipped))
        (check #t (pair? (agent-ref listing 'problems)))
    "#,
    );
}

#[test]
fn symbol_scopes_and_parse_recovery_are_not_hidden() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("library.sld"), "(define-library (example) (export f) (import (scheme base)) (begin (define (f x) (define (local y) y) (local x)) (define-values (a b) (values 1 2))))").unwrap();
    fs::write(dir.path().join("nested.rs"), "trait Outer { fn declaration(&self); }\nfn outer() { fn inner() {} }\n").unwrap();
    fs::write(dir.path().join("broken.rs"), "fn valid() {}\nfn broken(").unwrap();
    let mut vm = world(dir.path());
    check(
        &mut vm,
        r#"
        (define (definition path name) (car (agent-ref (agent-symbols p path #:name name) 'items)))
        (check "library" (agent-ref (definition "library.sld" "(example)") 'scope))
        (check "module" (agent-ref (definition "library.sld" "f") 'scope))
        (check "local" (agent-ref (definition "library.sld" "local") 'scope))
        (check "module" (agent-ref (definition "library.sld" "b") 'scope))
        (check "member" (agent-ref (definition "nested.rs" "declaration") 'scope))
        (check "local" (agent-ref (definition "nested.rs" "inner") 'scope))
        (check 2 (agent-ref (definition "nested.rs" "outer") 'end-line))
        (check #f (agent-ref (agent-symbols p "broken.rs") 'complete))
        (check #f (agent-ref (agent-read-symbol p "broken.rs" "valid") 'complete))
    "#,
    );
}

#[test]
fn batching_keeps_successful_regions_when_another_read_fails() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("source"), "one\ntwo\n").unwrap();
    let mut vm = world(dir.path());
    check(
        &mut vm,
        r#"
        (define results (agent-batch
          (list (lambda () (agent-read p "source" #:from 1 #:lines 1))
                (lambda () (agent-read p "missing"))
                (lambda () (agent-read p "source" #:from 2 #:lines 1)))))
        (check '(#t #f #t) (map (lambda (r) (agent-ref r 'ok)) results))
        (check "one\n" (agent-ref (agent-ref (car results) 'value) 'text))
        (check #t (string? (agent-ref (cadr results) 'error)))
        (check "two\n" (agent-ref (agent-ref (caddr results) 'value) 'text))
        (check '() (agent-batch '()))
        (check #t (fails (lambda () (agent-batch (make-list 21 (lambda () #t))))))
        (check #t (eq? (agent-session-project (agent-root p)) (agent-session-project (string-append (agent-root p) "/."))))
    "#,
    );
}
