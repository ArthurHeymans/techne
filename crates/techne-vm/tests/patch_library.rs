//! Integration tests for Mode 2 live in-world library patching
//! (`Vm::patch_library` and Scheme `patch-library!`).

use std::{collections::BTreeMap, sync::Arc};

use techne_vm::{
    bundle::{BundlePackage, BundleVfs, PackagePrefix},
    patch::{PatchRefusal, sha256_hex},
    vm::{Capability, Grants, Vm},
};

fn parts(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

fn eval_str(vm: &mut Vm, code: &str) -> String {
    let val = vm.eval_source(code).unwrap_or_else(|e| panic!("eval failed for `{code}`: {e}"));
    vm.repr(&val)
}

#[test]
fn patch_preserves_record_rtd_and_mutable_state() {
    let mut vm = Vm::new();
    let v1 = r#"
(define-library (acme state)
  (export make-item item? item-name bump! count describe-item make-adder collect-all)
  (import (techne))
  (begin
    (define-record-type item
      (make-item name)
      item?
      (name item-name))
    (define counter (vector 0))
    (define (bump!)
      (vector-set! counter 0 (+ (vector-ref counter 0) 1))
      (vector-ref counter 0))
    (define (count)
      (vector-ref counter 0))
    (define (format-tag s)
      (string-append "v1:" s))
    (define (describe-item it)
      "Describe an item (v1)."
      (format-tag (item-name it)))
    (define ((make-adder a) b)
      (+ a b))
    (define (collect-all . xs)
      (length xs))))
"#;
    vm.eval_source(v1).unwrap();
    vm.eval_source("(import (acme state))").unwrap();
    vm.eval_source("(define before-item (make-item \"alpha\"))").unwrap();
    assert_eq!(eval_str(&mut vm, "(bump!)"), "1");
    assert_eq!(eval_str(&mut vm, "(bump!)"), "2");
    assert_eq!(eval_str(&mut vm, "(describe-item before-item)"), "\"v1:alpha\"");
    assert_eq!(eval_str(&mut vm, "((make-adder 10) 5)"), "15");
    assert_eq!(eval_str(&mut vm, "(collect-all 'a 'b 'c)"), "3");

    // No-op patch with extra whitespace and comments changes nothing.
    let v1_reformatted = format!("; comment\n\n{v1}\n");
    let noop = vm.patch_library(&parts(&["acme", "state"]), &v1_reformatted, true).unwrap();
    assert!(noop.patched.is_empty(), "{noop:?}");
    assert_eq!(noop.unchanged, 8);
    assert_eq!(noop.source_sha256, sha256_hex(v1_reformatted.as_bytes()));

    // Patch private helper, exported procedure (with updated docstring),
    // curried procedure, and dotted-rest procedure.
    let v2 = r#"
(define-library (acme state)
  (export make-item item? item-name bump! count describe-item make-adder collect-all)
  (import (techne))
  (begin
    (define-record-type item
      (make-item name)
      item?
      (name item-name))
    (define counter (vector 0))
    (define (bump!)
      (vector-set! counter 0 (+ (vector-ref counter 0) 1))
      (vector-ref counter 0))
    (define (count)
      (vector-ref counter 0))
    (define format-tag
      (lambda (s) (string-append "v2:" s)))
    (define (describe-item it)
      "Describe an item (v2)."
      (string-append (format-tag (item-name it)) "!"))
    (define ((make-adder a) b)
      (+ a b 100))
    (define (collect-all . xs)
      (* 10 (length xs)))))
"#;
    let report = vm.patch_library(&parts(&["acme", "state"]), v2, true).unwrap();
    assert_eq!(report.library, parts(&["acme", "state"]));
    assert_eq!(report.patched, vec!["format-tag", "describe-item", "make-adder", "collect-all"]);
    assert_eq!(report.unchanged, 4);
    assert!(report.stale_holders.is_empty(), "{:?}", report.stale_holders);
    assert_eq!(report.source_sha256, sha256_hex(v2.as_bytes()));

    // Pre-patch record identity and mutable counter survive untouched!
    assert_eq!(eval_str(&mut vm, "(item? before-item)"), "#t");
    assert_eq!(eval_str(&mut vm, "(item-name before-item)"), "\"alpha\"");
    assert_eq!(eval_str(&mut vm, "(count)"), "2");
    assert_eq!(eval_str(&mut vm, "(bump!)"), "3");

    // Patched procedures take effect immediately.
    assert_eq!(eval_str(&mut vm, "(describe-item before-item)"), "\"v2:alpha!\"");
    assert_eq!(eval_str(&mut vm, "((make-adder 10) 5)"), "115");
    assert_eq!(eval_str(&mut vm, "(collect-all 'a 'b 'c)"), "30");
}

#[test]
fn patch_is_immediately_visible_to_jit_compiled_callers() {
    let mut vm = Vm::new();
    vm.eval_source(
        r#"
(define-library (acme hot)
  (export step)
  (import (techne))
  (begin
    (define (step x) (+ x 1))))
(import (acme hot))
(define (sum-steps n)
  (let loop ((i 0) (acc 0))
    (if (= i n)
        acc
        (loop (+ i 1) (+ acc (step i))))))
"#,
    )
    .unwrap();

    // Warm up well past JIT compilation thresholds.
    assert_eq!(eval_str(&mut vm, "(sum-steps 5000)"), "12502500");

    // Patch `(step x)` in-place; the already-compiled caller must see it immediately.
    let v2 = r#"
(define-library (acme hot)
  (export step)
  (import (techne))
  (begin
    (define (step x) (+ x 2))))
"#;
    let report = vm.patch_library(&parts(&["acme", "hot"]), v2, true).unwrap();
    assert_eq!(report.patched, vec!["step"]);
    assert_eq!(eval_str(&mut vm, "(sum-steps 5000)"), "12507500");
}

#[test]
fn patch_refuses_every_structural_change_atomically() {
    let mut vm = Vm::new();
    vm.eval_source(
        r#"
(define-library (acme base)
  (export foreign-fn)
  (import (techne))
  (begin
    (define (foreign-fn) 'from-base)))

(define-library (acme target)
  (export foreign-fn make-box-rec box-rec? box-val m-id get-val helper-entry)
  (import (techne) (acme base))
  (begin
    (define-record-type box-rec
      (make-box-rec val)
      box-rec?
      (val box-val))
    (define-syntax m-id
      (syntax-rules () ((_ x) x)))
    (define init-state 42)
    (define (priv x) (+ x 1))
    (define (get-val) init-state)
    (define (helper-entry x) (priv x))))
(import (acme target))
"#,
    )
    .unwrap();

    assert_eq!(eval_str(&mut vm, "(helper-entry 10)"), "11");
    let lib = parts(&["acme", "target"]);

    // 1. NotLoaded (unloaded library and builtin library)
    let err =
        vm.patch_library(&parts(&["acme", "missing"]), "(define-library (acme missing) (export) (import (techne)))", true).unwrap_err();
    assert_eq!(err.reason, PatchRefusal::NotLoaded);

    let err = vm.patch_library(&parts(&["scheme", "base"]), "(define-library (scheme base) (export) (import (techne)))", true).unwrap_err();
    assert_eq!(err.reason, PatchRefusal::NotLoaded);

    // 2. SyntaxError (malformed, empty, extra form, non-define-library)
    for bad in ["(define-library (acme target", "", "(+ 1 2)", "(define-library (acme target)) (+ 1 2)"] {
        let err = vm.patch_library(&lib, bad, true).unwrap_err();
        assert_eq!(err.reason, PatchRefusal::SyntaxError, "for `{bad}`: {err:?}");
    }

    // 3. LibraryNameMismatch
    let err = vm.patch_library(&lib, "(define-library (acme other) (export) (import (techne)))", true).unwrap_err();
    assert_eq!(err.reason, PatchRefusal::LibraryNameMismatch);

    // 4. IncludeNotSupported (all three declaration forms + top-level include)
    for decl in ["(include \"x.scm\")", "(include-ci \"x.scm\")", "(include-library-declarations \"x.scm\")", "(begin (include \"x.scm\"))"]
    {
        let src = format!(
            "(define-library (acme target)
               (export foreign-fn make-box-rec box-rec? box-val m-id get-val helper-entry)
               (import (techne) (acme base))
               {decl})"
        );
        let err = vm.patch_library(&lib, &src, true).unwrap_err();
        assert_eq!(err.reason, PatchRefusal::IncludeNotSupported, "for `{decl}`: {err:?}");
    }

    // 5. UnsupportedDeclaration
    let err = vm.patch_library(&lib, "(define-library (acme target) (bogus 1) (export) (import (techne)))", true).unwrap_err();
    assert_eq!(err.reason, PatchRefusal::UnsupportedDeclaration);

    // 6. ImportsChanged
    let err = vm
        .patch_library(
            &lib,
            r#"(define-library (acme target)
                 (export foreign-fn make-box-rec box-rec? box-val m-id get-val helper-entry)
                 (import (techne) (acme base) (scheme write))
                 (begin))"#,
            true,
        )
        .unwrap_err();
    assert_eq!(err.reason, PatchRefusal::ImportsChanged);

    // 7. ExportRebound (added export)
    let err = vm
        .patch_library(
            &lib,
            r#"(define-library (acme target)
                 (export foreign-fn make-box-rec box-rec? box-val m-id get-val helper-entry priv)
                 (import (techne) (acme base))
                 (begin))"#,
            true,
        )
        .unwrap_err();
    assert_eq!(err.reason, PatchRefusal::ExportRebound);

    // 8. BindingRemoved (export removed)
    let err = vm
        .patch_library(
            &lib,
            r#"(define-library (acme target)
                 (export foreign-fn make-box-rec box-rec? box-val m-id get-val)
                 (import (techne) (acme base))
                 (begin))"#,
            true,
        )
        .unwrap_err();
    assert_eq!(err.reason, PatchRefusal::BindingRemoved);
    assert_eq!(err.binding.as_deref(), Some("helper-entry"));

    let header = r#"
(define-library (acme target)
  (export foreign-fn make-box-rec box-rec? box-val m-id get-val helper-entry)
  (import (techne) (acme base))
  (begin
"#;

    // 9. RecordLayoutChanged (changed, removed, added)
    let rec_changed = format!(
        "{header}
    (define-record-type box-rec (make-box-rec val extra) box-rec? (val box-val) (extra box-extra))
    (define-syntax m-id (syntax-rules () ((_ x) x)))
    (define init-state 42)
    (define (priv x) (+ x 1))
    (define (get-val) init-state)
    (define (helper-entry x) (priv x))))"
    );
    assert_eq!(vm.patch_library(&lib, &rec_changed, true).unwrap_err().reason, PatchRefusal::RecordLayoutChanged);

    let rec_removed = format!(
        "{header}
    (define-syntax m-id (syntax-rules () ((_ x) x)))
    (define init-state 42)
    (define (priv x) (+ x 1))
    (define (get-val) init-state)
    (define (helper-entry x) (priv x))))"
    );
    assert_eq!(vm.patch_library(&lib, &rec_removed, true).unwrap_err().reason, PatchRefusal::RecordLayoutChanged);

    let rec_added = format!(
        "{header}
    (define-record-type box-rec (make-box-rec val) box-rec? (val box-val))
    (define-record-type extra-rec (make-extra) extra-rec?)
    (define-syntax m-id (syntax-rules () ((_ x) x)))
    (define init-state 42)
    (define (priv x) (+ x 1))
    (define (get-val) init-state)
    (define (helper-entry x) (priv x))))"
    );
    assert_eq!(vm.patch_library(&lib, &rec_added, true).unwrap_err().reason, PatchRefusal::RecordLayoutChanged);

    // 10. MacroChanged (changed, removed, added)
    let macro_changed = format!(
        "{header}
    (define-record-type box-rec (make-box-rec val) box-rec? (val box-val))
    (define-syntax m-id (syntax-rules () ((_ x) (list x))))
    (define init-state 42)
    (define (priv x) (+ x 1))
    (define (get-val) init-state)
    (define (helper-entry x) (priv x))))"
    );
    assert_eq!(vm.patch_library(&lib, &macro_changed, true).unwrap_err().reason, PatchRefusal::MacroChanged);

    let macro_added = format!(
        "{header}
    (define-record-type box-rec (make-box-rec val) box-rec? (val box-val))
    (define-syntax m-id (syntax-rules () ((_ x) x)))
    (define-syntax m-new (syntax-rules () ((_ x) x)))
    (define init-state 42)
    (define (priv x) (+ x 1))
    (define (get-val) init-state)
    (define (helper-entry x) (priv x))))"
    );
    assert_eq!(vm.patch_library(&lib, &macro_added, true).unwrap_err().reason, PatchRefusal::MacroChanged);

    // 11. StateInitializerChanged (changed non-lambda define, added top-level expr)
    let state_changed = format!(
        "{header}
    (define-record-type box-rec (make-box-rec val) box-rec? (val box-val))
    (define-syntax m-id (syntax-rules () ((_ x) x)))
    (define init-state 99)
    (define (priv x) (+ x 1))
    (define (get-val) init-state)
    (define (helper-entry x) (priv x))))"
    );
    let err = vm.patch_library(&lib, &state_changed, true).unwrap_err();
    assert_eq!(err.reason, PatchRefusal::StateInitializerChanged);
    assert_eq!(err.binding.as_deref(), Some("init-state"));

    let top_expr = format!(
        "{header}
    (define-record-type box-rec (make-box-rec val) box-rec? (val box-val))
    (define-syntax m-id (syntax-rules () ((_ x) x)))
    (define init-state 42)
    (define (priv x) (+ x 1))
    (define (get-val) init-state)
    (define (helper-entry x) (priv x))
    (display \"side-effect\")))"
    );
    assert_eq!(vm.patch_library(&lib, &top_expr, true).unwrap_err().reason, PatchRefusal::StateInitializerChanged);

    // 12. BindingRemoved (private procedure removed)
    let priv_removed = format!(
        "{header}
    (define-record-type box-rec (make-box-rec val) box-rec? (val box-val))
    (define-syntax m-id (syntax-rules () ((_ x) x)))
    (define init-state 42)
    (define (get-val) init-state)
    (define (helper-entry x) x)))"
    );
    let err = vm.patch_library(&lib, &priv_removed, true).unwrap_err();
    assert_eq!(err.reason, PatchRefusal::BindingRemoved);
    assert_eq!(err.binding.as_deref(), Some("priv"));

    // 13. BindingAdded (new procedure added)
    let proc_added = format!(
        "{header}
    (define-record-type box-rec (make-box-rec val) box-rec? (val box-val))
    (define-syntax m-id (syntax-rules () ((_ x) x)))
    (define init-state 42)
    (define (priv x) (+ x 1))
    (define (brand-new x) x)
    (define (get-val) init-state)
    (define (helper-entry x) (priv x))))"
    );
    let err = vm.patch_library(&lib, &proc_added, true).unwrap_err();
    assert_eq!(err.reason, PatchRefusal::BindingAdded);
    assert_eq!(err.binding.as_deref(), Some("brand-new"));

    // 14. ExportRebound (attempting to patch a re-exported binding owned by (acme base))
    let steal_foreign = format!(
        "{header}
    (define-record-type box-rec (make-box-rec val) box-rec? (val box-val))
    (define-syntax m-id (syntax-rules () ((_ x) x)))
    (define init-state 42)
    (define (priv x) (+ x 1))
    (define (get-val) init-state)
    (define (helper-entry x) (priv x))
    (define (foreign-fn) 'hijacked)))"
    );
    let err = vm.patch_library(&lib, &steal_foreign, true).unwrap_err();
    assert_eq!(err.reason, PatchRefusal::ExportRebound);
    assert_eq!(err.binding.as_deref(), Some("foreign-fn"));

    // 15. Duplicate definition in patch source
    let dup = format!(
        "{header}
    (define-record-type box-rec (make-box-rec val) box-rec? (val box-val))
    (define-syntax m-id (syntax-rules () ((_ x) x)))
    (define init-state 42)
    (define (priv x) (+ x 2))
    (define (priv x) (+ x 3))
    (define (get-val) init-state)
    (define (helper-entry x) (priv x))))"
    );
    let err = vm.patch_library(&lib, &dup, true).unwrap_err();
    assert_eq!(err.reason, PatchRefusal::SyntaxError);
    assert_eq!(err.binding.as_deref(), Some("priv"));

    // Live state and behavior remain completely untouched after every refusal.
    assert_eq!(eval_str(&mut vm, "(helper-entry 10)"), "11");
    assert_eq!(eval_str(&mut vm, "(get-val)"), "42");
    assert_eq!(eval_str(&mut vm, "(foreign-fn)"), "from-base");
}

#[test]
fn patch_compile_error_is_atomic() {
    let mut vm = Vm::new();
    vm.eval_source(
        r#"
(define-library (acme atomic)
  (export first-fn second-fn)
  (import (techne))
  (begin
    (define (first-fn) 'v1-first)
    (define (second-fn) 'v1-second)))
(import (acme atomic))
"#,
    )
    .unwrap();

    // First procedure is valid, second fails compilation (malformed `if`).
    let bad_patch = r#"
(define-library (acme atomic)
  (export first-fn second-fn)
  (import (techne))
  (begin
    (define (first-fn) 'v2-first)
    (define (second-fn) (if))))
"#;
    let err = vm.patch_library(&parts(&["acme", "atomic"]), bad_patch, true).unwrap_err();
    assert_eq!(err.reason, PatchRefusal::CompileError);
    assert_eq!(err.binding.as_deref(), Some("second-fn"));

    // First procedure MUST NOT have been committed.
    assert_eq!(eval_str(&mut vm, "(first-fn)"), "v1-first");
    assert_eq!(eval_str(&mut vm, "(second-fn)"), "v1-second");

    // Snapshot remained intact, so a valid patch now applies cleanly.
    let good_patch = r#"
(define-library (acme atomic)
  (export first-fn second-fn)
  (import (techne))
  (begin
    (define (first-fn) 'v2-first)
    (define (second-fn) 'v2-second)))
"#;
    let report = vm.patch_library(&parts(&["acme", "atomic"]), good_patch, true).unwrap();
    assert_eq!(report.patched, vec!["first-fn", "second-fn"]);
    assert_eq!(eval_str(&mut vm, "(list (first-fn) (second-fn))"), "(v2-first v2-second)");
}

#[test]
fn patch_reports_stale_closure_holders() {
    let mut vm = Vm::new();
    vm.eval_source(
        r#"
(define-library (acme callbacks)
  (export handler)
  (import (techne))
  (begin
    (define (handler x) (+ x 1))))
(import (acme callbacks))
(define saved-callbacks (list handler))
"#,
    )
    .unwrap();

    let v2 = r#"
(define-library (acme callbacks)
  (export handler)
  (import (techne))
  (begin
    (define (handler x) (+ x 2))))
"#;
    let report = vm.patch_library(&parts(&["acme", "callbacks"]), v2, true).unwrap();
    assert_eq!(report.patched, vec!["handler"]);
    assert_eq!(report.stale_holders.len(), 1, "{:?}", report.stale_holders);
    assert_eq!(report.stale_holders[0].binding, "handler");
    assert!(!report.stale_holders[0].chain.is_empty(), "{:?}", report.stale_holders);

    // Direct call uses v2, while saved-callbacks still holds v1.
    assert_eq!(eval_str(&mut vm, "(handler 10)"), "12");
    assert_eq!(eval_str(&mut vm, "((car saved-callbacks) 10)"), "11");

    // With check_retained = false, stale_holders is omitted even if a closure is retained.
    vm.eval_source("(set! saved-callbacks (list handler))").unwrap();
    let v3 = r#"
(define-library (acme callbacks)
  (export handler)
  (import (techne))
  (begin
    (define (handler x) (+ x 3))))
"#;
    let report = vm.patch_library(&parts(&["acme", "callbacks"]), v3, false).unwrap();
    assert!(report.stale_holders.is_empty(), "{:?}", report.stale_holders);

    // Once no external reference retains v3, patching to v4 reports zero stale holders.
    vm.eval_source("(set! saved-callbacks '())").unwrap();
    let v4 = r#"
(define-library (acme callbacks)
  (export handler)
  (import (techne))
  (begin
    (define (handler x) (+ x 4))))
"#;
    let report = vm.patch_library(&parts(&["acme", "callbacks"]), v4, true).unwrap();
    assert!(report.stale_holders.is_empty(), "{:?}", report.stale_holders);
    assert_eq!(eval_str(&mut vm, "(handler 10)"), "14");
}

#[test]
fn patch_across_snapshot_bundle_vfs_and_capability_gating() {
    let mut pkgs = BTreeMap::new();
    let mut bob_files = BTreeMap::new();
    bob_files.insert(
        "lib/bob/util.sld".to_string(),
        Arc::<[u8]>::from(
            br#"(define-library (bob util)
  (export greet)
  (import (techne))
  (begin
    (define (greet who) (string-append "hello " who))))"#
                .as_slice(),
        ),
    );
    pkgs.insert(PackagePrefix::new("bob", "util"), BundlePackage { tree_sha256: "b".repeat(64), files: bob_files, declared_deps: vec![] });

    let mut alice_files = BTreeMap::new();
    alice_files.insert(
        "app.scm".to_string(),
        Arc::<[u8]>::from(
            br#"(import (bob util))
(define (run-app) (greet "world"))"#
                .as_slice(),
        ),
    );
    pkgs.insert(
        PackagePrefix::new("alice", "app"),
        BundlePackage { tree_sha256: "a".repeat(64), files: alice_files, declared_deps: vec![PackagePrefix::new("bob", "util")] },
    );

    let bundle = Arc::new(BundleVfs::new(BTreeMap::new(), Some(PackagePrefix::new("alice", "app")), pkgs).unwrap());

    let mut template_vm = Vm::with_grants(Grants::NONE.with(Capability::Loading));
    template_vm.install_bundle(bundle);
    template_vm.eval_bundle_entry("app.scm").unwrap();
    assert_eq!(eval_str(&mut template_vm, "(run-app)"), "\"hello world\"");

    // Snapshot with Loading revoked (just like production deployments / debug instances).
    template_vm.set_grant(Capability::Loading, false);
    let snap = template_vm.snapshot().unwrap();
    let mut patched_world = snap.world();
    let mut untouched_world = snap.world();

    let v2 = r#"(define-library (bob util)
  (export greet)
  (import (techne))
  (begin
    (define (greet who) (string-append "patched " who))))"#;

    // Scheme `(patch-library! ...)` is denied when Capability::Loading is revoked.
    let err = patched_world.eval_source(&format!("(patch-library! '(bob util) {v2:?})")).unwrap_err();
    assert!(err.msg.contains("loading") || err.msg.contains("not granted") || err.msg.contains("capability"), "{err}");

    // Rust `Vm::patch_library` succeeds on the snapshot-instantiated world!
    let report = patched_world.patch_library(&parts(&["bob", "util"]), v2, true).unwrap();
    assert_eq!(report.patched, vec!["greet"]);
    assert_eq!(eval_str(&mut patched_world, "(run-app)"), "\"patched world\"");
    // Sibling world from the same snapshot is completely untouched.
    assert_eq!(eval_str(&mut untouched_world, "(run-app)"), "\"hello world\"");

    // When Capability::Loading is granted, Scheme `(patch-library! ...)` succeeds and returns a report dict.
    patched_world.set_grant(Capability::Loading, true);
    let v3 = r#"(define-library (bob util)
  (export greet)
  (import (techne))
  (begin
    (define (greet who) (string-append "scheme " who))))"#;
    let res = eval_str(
        &mut patched_world,
        &format!(
            "(let ((r (patch-library! '(bob util) {v3:?} #:check-retained #f)))
               (list (dict-ref r 'library)
                     (dict-ref r 'patched)
                     (dict-ref r 'unchanged)
                     (dict-ref r 'stale-holders)
                     (string? (dict-ref r 'source-sha256))))"
        ),
    );
    assert_eq!(res, "((\"bob\" \"util\") #(\"greet\") 0 #() #t)");
    assert_eq!(eval_str(&mut patched_world, "(run-app)"), "\"scheme world\"");
}
