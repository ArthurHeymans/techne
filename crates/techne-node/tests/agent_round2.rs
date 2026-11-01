//! Regression cases from the independent Terra and GLM dogfooding reviews.
use std::{fs, path::Path};
use techne_vm::vm::{Capability, Grants, Vm};

fn world_with_grants(root: &Path, grants: Grants) -> Vm {
    let mut vm = Vm::with_grants(grants);
    techne_node::install(&mut vm).unwrap();
    let library = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../lisp/techne/agent.sld");
    vm.eval_source(&format!(
        "(include {:?}) (import (techne agent)) (define p (agent-project {:?}))
         (define (check expected actual) (unless (equal? expected actual) (error \"check\" expected actual)))",
        library.to_str().unwrap(),
        root.to_str().unwrap()
    ))
    .unwrap();
    vm
}
fn world(root: &Path) -> Vm {
    world_with_grants(root, Grants::ALL)
}
fn check(vm: &mut Vm, source: &str) {
    vm.eval_source(source).unwrap();
}

#[test]
fn rust_symbol_spans_include_attached_outer_attributes_and_docs() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("items.rs"),
        r#"//! Crate documentation is not an outer item attribute.
// Ordinary leading comment is not attached documentation.
/// Function docs.
#[cfg(any())]
// A comment between associated attributes.
#[inline]
pub fn attributed() {}

/** Type docs. */
#[derive(Clone)]
pub struct Documented {}

#[repr(C)]
pub union Bits { value: u32 }

// Ordinary comment alone.
fn plain() {}

impl Documented {
    /// Member docs.
    #[cfg(test)]
    pub fn method(&self) {}
}
"#,
    )
    .unwrap();
    let mut vm = world(dir.path());
    check(
        &mut vm,
        r##"
        (define f (agent-ref (agent-read-symbol p "items.rs" "attributed") 'text))
        (check #t (string-prefix? "/// Function docs." f))
        (check #t (and (string-contains f "#[cfg(any())]") #t))
        (check #t (and (string-contains f "#[inline]") #t))
        (check #f (and (string-contains f "//! Crate") #t))
        (define t (agent-ref (agent-read-symbol p "items.rs" "Documented") 'text))
        (check #t (string-prefix? "/** Type docs. */" t))
        (check #t (and (string-contains t "#[derive(Clone)]") #t))
        (check #t (string-prefix? "#[repr(C)]" (agent-ref (agent-read-symbol p "items.rs" "Bits") 'text)))
        (check "fn plain() {}" (agent-ref (agent-read-symbol p "items.rs" "plain") 'text))
        (define method (agent-read-symbol p "items.rs" "method"))
        (check #t (string-prefix? "/// Member docs." (agent-ref method 'text)))
        (check #t (and (string-contains (agent-ref method 'text) "#[cfg(test)]") #t))
        (check 3 (agent-ref (car (agent-ref (agent-symbols p "items.rs" #:name "attributed") 'items)) 'line))
    "##,
    );
}

#[test]
fn preview_clipping_and_handle_lengths_are_explicit() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("long"), format!("{}needle{}\n", "é".repeat(1000), "x".repeat(1500))).unwrap();
    fs::write(dir.path().join("short"), "needle\n").unwrap();
    let mut vm = world(dir.path());
    check(
        &mut vm,
        r#"
        (define item (car (agent-ref (agent-search p "needle" #:prefix "long") 'items)))
        (define h (agent-ref item 'handle))
        (check #t (agent-ref item 'preview-truncated))
        (check 1001 (agent-ref item 'column))
        (check #t (> (agent-ref item 'preview-offset) 0))
        (check (agent-ref item 'text)
          (agent-ref (agent-expand p h #:offset (agent-ref item 'preview-offset) #:chars 240) 'text))
        (check 2507 (agent-ref (agent-expand p h #:chars 2) 'characters))
        (check "" (agent-ref (agent-expand p h #:offset 2507) 'text))
        (define message (guard (e (#t (error-object-message e))) (agent-expand p h #:offset 2508) "missing error"))
        (check #t (and (string-contains message "body has 2507 characters") #t))
        (define short (car (agent-ref (agent-search p "needle" #:prefix "short") 'items)))
        (check #f (agent-ref short 'preview-truncated))
        (check 0 (agent-ref short 'preview-offset))
        (check 1 (agent-ref short 'column))
    "#,
    );
}

#[test]
fn process_retention_loss_and_report_clipping_are_distinct() {
    let dir = tempfile::tempdir().unwrap();
    let mut vm = world(dir.path());
    check(
        &mut vm,
        r#"
        (define (run retain chars)
          (agent-ref (agent-process-result (agent-run p "printf" '("abcdef") #:retain retain) #:chars chars) 'stdout))
        (define a (run 3 20))
        (check '(6 3 #t #f #t) (map (lambda (key) (agent-ref a key)) '(characters retained dropped clipped truncated)))
        (check "def" (agent-ref a 'text))
        (define b (run 20 3))
        (check '(6 6 #f #t #t) (map (lambda (key) (agent-ref b key)) '(characters retained dropped clipped truncated)))
        (define c (run 6 6))
        (check '(6 6 #f #f #f) (map (lambda (key) (agent-ref c key)) '(characters retained dropped clipped truncated)))
        (define d (run 3 2))
        (check '(6 3 #t #t #t) (map (lambda (key) (agent-ref d key)) '(characters retained dropped clipped truncated)))
    "#,
    );
}

#[test]
fn malformed_environments_are_rejected_without_disclosing_values() {
    let dir = tempfile::tempdir().unwrap();
    let mut vm = world(dir.path());
    check(
        &mut vm,
        r#"
        (for-each
          (lambda (environment)
            (let ((message (guard (e (#t (error-object-message e)))
                             (agent-run p "true" '() #:environment environment) "missing error")))
              (check #t (string-prefix? "agent-run: environment must be" message))
              (check #f (and (string-contains message "sensitive-value") #t))))
          (list '(("BAD")) '(("NAME" "sensitive-value")) '(("" . "sensitive-value"))
                '(("A=B" . "sensitive-value")) (list (cons "NAME" (string (integer->char 0)))) 'not-a-list))
        (check 0 (agent-ref (agent-process-result
          (agent-run p "true" '() #:environment '(("TECHNE_AGENT_ROUND2" . "yes")))) 'status))
    "#,
    );
}

#[test]
fn files_without_processes_do_not_allow_agent_run() {
    let dir = tempfile::tempdir().unwrap();
    let mut vm = world_with_grants(dir.path(), Grants::ALL.without(Capability::Processes));
    let error = vm.eval_source("(agent-run p \"true\" '())").unwrap_err();
    assert!(error.msg.contains("process") && error.msg.contains("not granted"), "{error}");
}
