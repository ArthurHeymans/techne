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

    // Scheme `(patch-library! ...)` is denied when Capability::Patching is not granted (even if Loading is granted).
    let err = patched_world.eval_source(&format!("(patch-library! '(bob util) {v2:?})")).unwrap_err();
    assert!(err.msg.contains("patching") || err.msg.contains("not granted") || err.msg.contains("capability"), "{err}");

    patched_world.set_grant(Capability::Loading, true);
    let err = patched_world.eval_source(&format!("(patch-library! '(bob util) {v2:?})")).unwrap_err();
    assert!(err.msg.contains("patching") || err.msg.contains("not granted") || err.msg.contains("capability"), "{err}");

    // Even with Capability::Patching granted, untrusted eval_source in bundle mode cannot patch a bundle package.
    patched_world.set_grant(Capability::Patching, true);
    let err = patched_world.eval_source(&format!("(patch-library! '(bob util) {v2:?})")).unwrap_err();
    assert!(err.msg.contains("owned by package") || err.msg.contains("not accessible"), "{err}");

    // Rust `Vm::patch_library` succeeds on the snapshot-instantiated world (without requiring Scheme capabilities)!
    patched_world.set_grant(Capability::Patching, false);
    patched_world.set_grant(Capability::Loading, false);
    let report = patched_world.patch_library(&parts(&["bob", "util"]), v2, true).unwrap();
    assert_eq!(report.patched, vec!["greet"]);
    assert!(!report.stale_holders_truncated);
    assert_eq!(eval_str(&mut patched_world, "(run-app)"), "\"patched world\"");
    // Sibling world from the same snapshot is completely untouched.
    assert_eq!(eval_str(&mut untouched_world, "(run-app)"), "\"hello world\"");

    // In a non-bundle/non-locked world with Capability::Patching, Scheme `(patch-library! ...)` returns a report dict.
    let mut dev_vm = Vm::with_grants(Grants::NONE.with(Capability::Loading).with(Capability::Patching));
    dev_vm
        .eval_source(
            r#"(define-library (dev util)
                 (export greet)
                 (import (techne))
                 (begin (define (greet who) (string-append "hi " who))))
               (import (dev util))"#,
        )
        .unwrap();
    let v3 = r#"(define-library (dev util)
  (export greet)
  (import (techne))
  (begin
    (define (greet who) (string-append "scheme " who))))"#;
    let res = eval_str(
        &mut dev_vm,
        &format!(
            "(let ((r (patch-library! '(dev util) {v3:?} #:check-retained #f)))
               (list (dict-ref r 'library)
                     (dict-ref r 'patched)
                     (dict-ref r 'unchanged)
                     (dict-ref r 'stale-holders)
                     (dict-ref r 'stale-holders-truncated)
                     (string? (dict-ref r 'source-sha256))))"
        ),
    );
    assert_eq!(res, "((\"dev\" \"util\") #(\"greet\") 0 #() #f #t)");
    assert_eq!(eval_str(&mut dev_vm, "(greet \"world\")"), "\"scheme world\"");
}

#[test]
fn bundle_initialization_refuses_scheme_patch_library_with_loading_only() {
    let mut pkgs = BTreeMap::new();
    let mut bob_files = BTreeMap::new();
    bob_files.insert(
        "lib/bob/util.sld".to_string(),
        Arc::<[u8]>::from(
            br#"(define-library (bob util)
  (export greet)
  (import (techne))
  (begin
    (define (greet) "v1")))"#
                .as_slice(),
        ),
    );
    pkgs.insert(PackagePrefix::new("bob", "util"), BundlePackage { tree_sha256: "b".repeat(64), files: bob_files, declared_deps: vec![] });

    let mut alice_files = BTreeMap::new();
    alice_files.insert(
        "app.scm".to_string(),
        Arc::<[u8]>::from(
            br#"(import (bob util))
(patch-library! '(bob util)
  "(define-library (bob util) (export greet) (import (techne)) (begin (define (greet) \"pwned\")))")"#
                .as_slice(),
        ),
    );
    pkgs.insert(
        PackagePrefix::new("alice", "app"),
        BundlePackage { tree_sha256: "a".repeat(64), files: alice_files, declared_deps: vec![PackagePrefix::new("bob", "util")] },
    );

    let bundle = Arc::new(BundleVfs::new(BTreeMap::new(), Some(PackagePrefix::new("alice", "app")), pkgs).unwrap());

    // Production bundle loading grants Loading only (never Patching).
    let mut vm = Vm::with_grants(Grants::NONE.with(Capability::Loading));
    vm.install_bundle(bundle.clone());
    let err = vm.eval_bundle_entry("app.scm").unwrap_err();
    assert!(err.msg.contains("patching") || err.msg.contains("not granted") || err.msg.contains("capability"), "{err}");

    // Even if Capability::Patching IS granted, `(alice app)` cannot patch `(bob util)` (ownership check).
    let mut vm_with_patch = Vm::with_grants(Grants::NONE.with(Capability::Loading).with(Capability::Patching));
    vm_with_patch.install_bundle(bundle);
    let err = vm_with_patch.eval_bundle_entry("app.scm").unwrap_err();
    assert!(err.msg.contains("owned by package") || err.msg.contains("not accessible"), "{err}");
}

#[test]
fn eval_source_named_vm_helper_cannot_inherit_stdlib_or_package_provenance() {
    // 1. Tracing `eval-source` puts a `(techne debug)` stdlib frame directly under
    //    `prelude.scm`'s `eval-source`. Naming the evaluated source `"<vm helper>"`
    //    must NOT skip that frame to inherit Stdlib provenance.
    let mut vm = Vm::new();
    vm.set_locked_libraries(true);
    let err = vm
        .eval_source(
            r#"(import (techne debug))
               (trace eval-source)
               (eval-source
                 "(define-library (techne forged) (export x) (import (techne)) (begin (define x 1)))"
                 "user"
                 "<vm helper>")"#,
        )
        .unwrap_err();
    assert!(err.msg.contains("not provided in locked mode") || err.msg.contains("reserved") || err.msg.contains("owned"), "{err}");

    // 2. Inside a bundled package `(alice app)`, calling `eval-source` named `"<vm helper>"`
    //    must NOT inherit `(alice app)` provenance for defining or patching `(alice app ...)`.
    let mut pkgs = BTreeMap::new();
    let mut alice_files = BTreeMap::new();
    alice_files.insert(
        "lib/alice/app/util.sld".to_string(),
        Arc::<[u8]>::from(
            br#"(define-library (alice app util)
  (export val)
  (import (techne))
  (begin (define (val) 1)))"#
                .as_slice(),
        ),
    );
    alice_files.insert(
        "app.scm".to_string(),
        Arc::<[u8]>::from(
            br#"(import (alice app util))
(define (try-forge-define)
  (eval-source
    "(define-library (alice app forged) (export x) (import (techne)) (begin (define x 1)))"
    "user"
    "<vm helper>"))
(define (try-forge-patch)
  (eval-source
    "(patch-library! '(alice app util) \"(define-library (alice app util) (export val) (import (techne)) (begin (define (val) 2)))\")"
    "user"
    "<vm helper>"))"#
                .as_slice(),
        ),
    );
    pkgs.insert(
        PackagePrefix::new("alice", "app"),
        BundlePackage { tree_sha256: "a".repeat(64), files: alice_files, declared_deps: vec![] },
    );
    let bundle = Arc::new(BundleVfs::new(BTreeMap::new(), Some(PackagePrefix::new("alice", "app")), pkgs).unwrap());
    let mut bvm = Vm::with_grants(Grants::NONE.with(Capability::Loading).with(Capability::Patching));
    bvm.install_bundle(bundle);
    bvm.eval_bundle_entry("app.scm").unwrap();

    let err = bvm.eval_source("(try-forge-define)").unwrap_err();
    assert!(err.msg.contains("owned by package") || err.msg.contains("cannot define"), "{err}");

    let err = bvm.eval_source("(try-forge-patch)").unwrap_err();
    assert!(err.msg.contains("owned by package") || err.msg.contains("not accessible"), "{err}");
    assert_eq!(eval_str(&mut bvm, "(val)"), "1");
}

#[test]
fn patch_compile_error_does_not_leak_forward_globals_or_bindings() {
    let mut vm = Vm::new();
    vm.eval_source(
        r#"
(define-library (acme clean)
  (export f g)
  (import (techne))
  (begin
    (define (f x) (+ x 1))
    (define (g x) (+ x 2))))
(import (acme clean))
"#,
    )
    .unwrap();

    let before_exports = eval_str(&mut vm, "(length (module-exports \"(acme clean)\"))");
    let lib = parts(&["acme", "clean"]);

    // Referencing an undefined symbol during patch staging fails compilation without
    // allocating a forward global slot or mutating `(acme clean)`'s binding table.
    let bad = r#"
(define-library (acme clean)
  (export f g)
  (import (techne))
  (begin
    (define (f x) (+ x never-defined-leaked-sym))
    (define (g x) (if))))
"#;
    let err = vm.patch_library(&lib, bad, true).unwrap_err();
    assert_eq!(err.reason, PatchRefusal::CompileError);
    assert_eq!(err.binding.as_deref(), Some("f"));

    // Verify `never-defined-leaked-sym` was NOT leaked into `(acme clean)`'s binding table.
    let check_bound = vm.eval_source("(eval-source \"never-defined-leaked-sym\" \"(acme clean)\" \"check\")");
    assert!(check_bound.is_err(), "forward global must not be leaked into module bindings");
    assert_eq!(eval_str(&mut vm, "(list (f 10) (g 10))"), "(11 12)");
    assert_eq!(eval_str(&mut vm, "(length (module-exports \"(acme clean)\"))"), before_exports);

    // Confirm a clean patch still works cleanly afterward.
    let good = r#"
(define-library (acme clean)
  (export f g)
  (import (techne))
  (begin
    (define (f x) (+ x 10))
    (define (g x) (+ x 20))))
"#;
    let report = vm.patch_library(&lib, good, true).unwrap();
    assert_eq!(report.patched, vec!["f", "g"]);
    assert_eq!(eval_str(&mut vm, "(list (f 10) (g 10))"), "(20 30)");
}

#[test]
fn patch_refuses_expression_level_include_and_cond_expand_including_macro_expanded() {
    let dir = tempfile::tempdir().unwrap();
    let secret_file = dir.path().join("should-never-be-read.scm");
    std::fs::write(&secret_file, "999999").unwrap();
    let secret_path = secret_file.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");

    let mut vm = Vm::new();
    vm.eval_source(
        r#"
(define-library (acme stage)
  (export f)
  (import (techne))
  (begin
    (define-syntax splice-inc
      (syntax-rules ()
        ((_ path) (include path))))
    (define (f) 1)))
(import (acme stage))
"#,
    )
    .unwrap();
    let lib = parts(&["acme", "stage"]);

    // 1. Direct expression-level `include` inside a patched procedure body.
    let direct_inc = format!(
        r#"(define-library (acme stage)
             (export f)
             (import (techne))
             (begin
               (define-syntax splice-inc
                 (syntax-rules ()
                   ((_ path) (include path))))
               (define (f) (include "{secret_path}"))))"#
    );
    let err = vm.patch_library(&lib, &direct_inc, true).unwrap_err();
    assert_eq!(err.reason, PatchRefusal::IncludeNotSupported, "{err:?}");

    // 2. Direct expression-level `include-ci`.
    let direct_inc_ci = format!(
        r#"(define-library (acme stage)
             (export f)
             (import (techne))
             (begin
               (define-syntax splice-inc
                 (syntax-rules ()
                   ((_ path) (include path))))
               (define (f) (include-ci "{secret_path}"))))"#
    );
    let err = vm.patch_library(&lib, &direct_inc_ci, true).unwrap_err();
    assert_eq!(err.reason, PatchRefusal::IncludeNotSupported, "{err:?}");

    // 3. Macro-expanded `include` inside a patched procedure body.
    let macro_inc = format!(
        r#"(define-library (acme stage)
             (export f)
             (import (techne))
             (begin
               (define-syntax splice-inc
                 (syntax-rules ()
                   ((_ path) (include path))))
               (define (f) (splice-inc "{secret_path}"))))"#
    );
    let err = vm.patch_library(&lib, &macro_inc, true).unwrap_err();
    assert_eq!(err.reason, PatchRefusal::IncludeNotSupported, "{err:?}");

    // 4. Expression-level `cond-expand` inside a patched procedure body.
    let expr_ce = r#"(define-library (acme stage)
                       (export f)
                       (import (techne))
                       (begin
                         (define-syntax splice-inc
                           (syntax-rules ()
                             ((_ path) (include path))))
                         (define (f) (cond-expand (else 42)))))"#;
    let err = vm.patch_library(&lib, expr_ce, true).unwrap_err();
    assert_eq!(err.reason, PatchRefusal::UnsupportedDeclaration, "{err:?}");

    // Live binding was never touched.
    assert_eq!(eval_str(&mut vm, "(f)"), "1");
}

#[test]
fn patch_diff_tightening_state_order_repurposed_slot_shadowed_begin_and_cond_expand() {
    // 1. Reordered state initializers MUST be refused with StateInitializerChanged.
    {
        let mut vm = Vm::new();
        vm.eval_source(
            r#"
(define-library (acme order)
  (export get-pair)
  (import (techne))
  (begin
    (define a (vector 1))
    (define b (vector (vector-ref a 0)))
    (define (get-pair) (list (vector-ref a 0) (vector-ref b 0)))))
(import (acme order))
"#,
        )
        .unwrap();
        let reordered = r#"
(define-library (acme order)
  (export get-pair)
  (import (techne))
  (begin
    (define b (vector (vector-ref a 0)))
    (define a (vector 1))
    (define (get-pair) (list (vector-ref a 0) (vector-ref b 0)))))
"#;
        let err = vm.patch_library(&parts(&["acme", "order"]), reordered, true).unwrap_err();
        assert_eq!(err.reason, PatchRefusal::StateInitializerChanged, "{err:?}");
        assert_eq!(eval_str(&mut vm, "(get-pair)"), "(1 1)");
    }

    // 2. Slot repurposed via `set!` (non-procedure OR wrapped closure) is refused instead of overwritten.
    {
        let mut vm = Vm::new();
        vm.eval_source(
            r#"
(define-library (acme mutate)
  (export f g repurpose-f! wrap-g!)
  (import (techne))
  (begin
    (define (f x) (+ x 1))
    (define (g x) (+ x 1))
    (define (repurpose-f!) (set! f 42))
    (define (wrap-g!)
      (let ((orig g))
        (set! g (lambda (x) (+ 100 (orig x))))))))
(import (acme mutate))
"#,
        )
        .unwrap();
        let lib = parts(&["acme", "mutate"]);

        // First repurpose `f` to a non-procedure (42).
        vm.eval_source("(repurpose-f!)").unwrap();
        let patch_f = r#"
(define-library (acme mutate)
  (export f g repurpose-f! wrap-g!)
  (import (techne))
  (begin
    (define (f x) (+ x 99))
    (define (g x) (+ x 1))
    (define (repurpose-f!) (set! f 42))
    (define (wrap-g!)
      (let ((orig g))
        (set! g (lambda (x) (+ 100 (orig x))))))))
"#;
        let err = vm.patch_library(&lib, patch_f, true).unwrap_err();
        assert_eq!(err.reason, PatchRefusal::StateInitializerChanged, "{err:?}");
        assert_eq!(err.binding.as_deref(), Some("f"));
        assert_eq!(eval_str(&mut vm, "f"), "42");

        // Now wrap `g` with another closure via `set!`.
        vm.eval_source("(wrap-g!)").unwrap();
        let patch_g = r#"
(define-library (acme mutate)
  (export f g repurpose-f! wrap-g!)
  (import (techne))
  (begin
    (define (f x) (+ x 1))
    (define (g x) (+ x 99))
    (define (repurpose-f!) (set! f 42))
    (define (wrap-g!)
      (let ((orig g))
        (set! g (lambda (x) (+ 100 (orig x))))))))
"#;
        let err = vm.patch_library(&lib, patch_g, true).unwrap_err();
        assert_eq!(err.reason, PatchRefusal::StateInitializerChanged, "{err:?}");
        assert_eq!(err.binding.as_deref(), Some("g"));
        assert_eq!(eval_str(&mut vm, "(g 5)"), "106");
    }

    // 3. Shadowed `begin` macro is NOT misparsed as top-level procedure definitions.
    {
        let mut vm = Vm::new();
        vm.eval_source(
            r#"
(define-library (acme shadow-begin)
  (export f)
  (import (techne))
  (begin
    (define-syntax begin
      (syntax-rules ()
        ((_ body ...) 'shadowed)))
    (define (f) 1)
    (begin (define (not-a-top-level) 2))))
(import (acme shadow-begin))
"#,
        )
        .unwrap();
        let lib = parts(&["acme", "shadow-begin"]);
        // Modifying the shadowed `(begin ...)` form must be refused as a top-level expression change,
        // NOT treated as patching a top-level procedure `not-a-top-level`!
        let changed_shadow = r#"
(define-library (acme shadow-begin)
  (export f)
  (import (techne))
  (begin
    (define-syntax begin
      (syntax-rules ()
        ((_ body ...) 'shadowed)))
    (define (f) 1)
    (begin (define (not-a-top-level) 999))))
"#;
        let err = vm.patch_library(&lib, changed_shadow, true).unwrap_err();
        assert_eq!(err.reason, PatchRefusal::StateInitializerChanged, "{err:?}");

        // Patching the real top-level procedure `f` while leaving the shadowed `(begin ...)` intact succeeds.
        let patch_real = r#"
(define-library (acme shadow-begin)
  (export f)
  (import (techne))
  (begin
    (define-syntax begin
      (syntax-rules ()
        ((_ body ...) 'shadowed)))
    (define (f) 2)
    (begin (define (not-a-top-level) 2))))
"#;
        let report = vm.patch_library(&lib, patch_real, true).unwrap();
        assert_eq!(report.patched, vec!["f"]);
        assert_eq!(eval_str(&mut vm, "(f)"), "2");
    }

    // 4. Divergent `cond-expand` clause selection in patch source is refused cleanly.
    {
        let mut vm = Vm::new();
        vm.eval_source(
            r#"
(define-library (acme ce)
  (export val)
  (import (techne))
  (cond-expand
    ((library (acme optional-missing))
     (begin (define (val) 'opt)))
    (else
     (begin (define (val) 'fallback)))))
(import (acme ce))
"#,
        )
        .unwrap();
        assert_eq!(eval_str(&mut vm, "(val)"), "fallback");

        // Now define `(acme optional-missing)` AFTER `(acme ce)` was loaded.
        // Re-evaluating `cond-expand` during patching would take clause 0 instead of recorded clause 1!
        vm.eval_source("(define-library (acme optional-missing) (export) (import (techne)))").unwrap();

        let patch_same = r#"
(define-library (acme ce)
  (export val)
  (import (techne))
  (cond-expand
    ((library (acme optional-missing))
     (begin (define (val) 'opt-v2)))
    (else
     (begin (define (val) 'fallback-v2)))))
"#;
        let err = vm.patch_library(&parts(&["acme", "ce"]), patch_same, true).unwrap_err();
        assert_eq!(err.reason, PatchRefusal::UnsupportedDeclaration, "{err:?}");
        assert_eq!(eval_str(&mut vm, "(val)"), "fallback");
    }
}

#[test]
fn patch_large_stale_holder_graph_is_bounded_and_truncated() {
    let mut vm = Vm::new();

    // Build a library with 270 procedures (> 256 stale_holders cap) AND retain one of them
    // at a chain depth > 64 (80 nested cons cells).
    let mut defs_v1 = String::new();
    let mut defs_v2 = String::new();
    let mut exports = String::new();
    for i in 0..270 {
        exports.push_str(&format!(" f{i}"));
        defs_v1.push_str(&format!("(define (f{i}) {i})\n"));
        defs_v2.push_str(&format!("(define (f{i}) {})\n", i + 1000));
    }
    let lib_v1 = format!("(define-library (acme many) (export{exports}) (import (techne)) (begin {defs_v1}))");
    let lib_v2 = format!("(define-library (acme many) (export{exports}) (import (techne)) (begin {defs_v2}))");

    vm.eval_source(&lib_v1).unwrap();
    vm.eval_source("(import (acme many))").unwrap();

    // Retain all 270 closures: f0 inside an 80-deep nested cons chain, f1..f269 in a vector.
    let mut retain_vec = String::from("(define kept-many (vector");
    for i in 1..270 {
        retain_vec.push_str(&format!(" f{i}"));
    }
    retain_vec.push_str("))");
    vm.eval_source(&retain_vec).unwrap();
    vm.eval_source(
        r#"(define kept-deep
             (let loop ((n 80) (acc f0))
               (if (= n 0) acc (loop (- n 1) (cons acc '())))))"#,
    )
    .unwrap();

    let report = vm.patch_library(&parts(&["acme", "many"]), &lib_v2, true).unwrap();
    assert_eq!(report.patched.len(), 270);
    assert!(report.stale_holders_truncated, "expected stale_holders_truncated=true for 270 holders / deep chain");
    assert!(report.stale_holders.len() <= techne_vm::patch::MAX_STALE_HOLDERS, "len={}", report.stale_holders.len());
    for holder in &report.stale_holders {
        assert!(holder.chain.len() <= techne_vm::patch::MAX_STALE_CHAIN, "chain len={}", holder.chain.len());
    }
}

#[test]
fn non_interning_library_name_parser_enforces_bounds() {
    use techne_vm::patch::{MAX_LIBRARY_NAME_BYTES, MAX_LIBRARY_PART_BYTES, MAX_LIBRARY_PARTS, parse_library_name, validate_library_parts};

    assert_eq!(parse_library_name("(acme util 2)").unwrap(), vec!["acme", "util", "2"]);
    assert_eq!(parse_library_name(" ( acme  util-2  007 ) ").unwrap(), vec!["acme", "util-2", "7"]);

    // Reject empty, comments, pipe escapes, >32 parts, oversized part, oversized string, negative int, dotted pair, trailing tokens.
    assert!(parse_library_name("()").is_err());
    assert!(parse_library_name("; comment\n(acme util)").is_err());
    assert!(parse_library_name("(acme |bad|)").is_err());
    assert!(parse_library_name("(acme -1)").is_err());
    assert!(parse_library_name("(acme . util)").is_err());
    assert!(parse_library_name("(acme util) (extra)").is_err());

    let too_many_str = format!("({})", vec!["a"; MAX_LIBRARY_PARTS + 1].join(" "));
    assert!(parse_library_name(&too_many_str).is_err());
    assert!(validate_library_parts(&vec!["a".to_string(); MAX_LIBRARY_PARTS + 1]).is_err());

    let huge_part = "a".repeat(MAX_LIBRARY_PART_BYTES + 1);
    assert!(parse_library_name(&format!("({huge_part})")).is_err());
    assert!(validate_library_parts(&[huge_part]).is_err());

    let huge_input = format!("({} a)", " ".repeat(MAX_LIBRARY_NAME_BYTES));
    assert!(parse_library_name(&huge_input).is_err());
}
