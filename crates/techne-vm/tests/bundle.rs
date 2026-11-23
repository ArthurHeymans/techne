//! Read-only in-memory `BundleVfs` loading, isolation, ownership, and snapshot tests.

use std::{collections::BTreeMap, sync::Arc};

use techne_vm::{
    bundle::{BundlePackage, BundleVfs, PackagePrefix},
    vm::{Capability, Grants, Vm},
};

fn bytes(s: &str) -> Arc<[u8]> {
    Arc::from(s.as_bytes().to_vec().into_boxed_slice())
}

fn files(entries: &[(&str, &str)]) -> BTreeMap<String, Arc<[u8]>> {
    entries.iter().map(|(k, v)| ((*k).to_string(), bytes(v))).collect()
}

fn pkg(tree_sha256: &str, entries: &[(&str, &str)], deps: &[(&str, &str)]) -> BundlePackage {
    BundlePackage {
        tree_sha256: tree_sha256.to_string(),
        files: files(entries),
        declared_deps: deps.iter().map(|(o, p)| PackagePrefix::new(o, p)).collect(),
    }
}

#[test]
fn bundle_construction_validation() {
    // Invalid package names
    for (o, p) in [("Alice", "app"), ("alice", "App"), (".alice", "app"), ("alice", ".app"), ("", "app"), ("a/b", "app")] {
        let mut pkgs = BTreeMap::new();
        pkgs.insert(PackagePrefix::new(o, p), pkg("sha", &[("app.scm", "1")], &[]));
        assert!(BundleVfs::new(BTreeMap::new(), None, pkgs).is_err(), "expected error for ({o} {p})");
    }

    // Reserved prefixes and shipped stdlib
    for (o, p) in [("scheme", "base"), ("srfi", "1"), ("techne", "crypto"), ("techne", "editor"), ("techne", "package")] {
        let mut pkgs = BTreeMap::new();
        pkgs.insert(PackagePrefix::new(o, p), pkg("sha", &[("app.scm", "1")], &[]));
        assert!(BundleVfs::new(BTreeMap::new(), None, pkgs).is_err(), "expected error for reserved ({o} {p})");
    }

    // Unclean and hidden paths
    for bad in
        ["", "/app.scm", "app.scm/", "./app.scm", "../app.scm", "a//b.scm", "a\\b.scm", ".secrets.scm", ".git/config", "lib/.hidden.scm"]
    {
        assert!(BundleVfs::from_root_files(files(&[(bad, "1")])).is_err(), "expected error for root path {bad:?}");
        let mut pkgs = BTreeMap::new();
        pkgs.insert(PackagePrefix::new("alice", "app"), pkg("sha", &[(bad, "1")], &[]));
        assert!(
            BundleVfs::new(BTreeMap::new(), Some(PackagePrefix::new("alice", "app")), pkgs).is_err(),
            "expected error for pkg path {bad:?}"
        );
    }

    // Missing root package
    assert!(BundleVfs::new(BTreeMap::new(), Some(PackagePrefix::new("alice", "app")), BTreeMap::new()).is_err());

    // Missing declared dependency
    let mut pkgs = BTreeMap::new();
    pkgs.insert(PackagePrefix::new("alice", "app"), pkg("sha", &[("app.scm", "1")], &[("bob", "missing")]));
    assert!(BundleVfs::new(BTreeMap::new(), Some(PackagePrefix::new("alice", "app")), pkgs).is_err());
}

#[test]
fn bundle_import_require_include_cond_expand_and_stdlib() {
    let mut pkgs = BTreeMap::new();
    pkgs.insert(
        PackagePrefix::new("carol", "leaf"),
        pkg(
            "sha-leaf",
            &[("lib/carol/leaf.sld", "(define-library (carol leaf) (export leaf-val) (import (scheme base)) (begin (define leaf-val 7)))")],
            &[],
        ),
    );
    pkgs.insert(
        PackagePrefix::new("bob", "mid"),
        pkg(
            "sha-mid",
            &[
                (
                    "lib/bob/mid.sld",
                    "(define-library (bob mid)
                       (include-library-declarations \"decls.scm\")
                       (import (scheme base) (carol leaf))
                       (include \"body.scm\")
                       (include-ci \"ci.scm\"))",
                ),
                ("lib/bob/decls.scm", "(export mid-val ci-sym)"),
                ("lib/bob/body.scm", "(define mid-val (+ leaf-val 35))"),
                ("lib/bob/ci.scm", "(define ci-sym 'HeLLo)"),
            ],
            &[("carol", "leaf")],
        ),
    );
    pkgs.insert(
        PackagePrefix::new("alice", "app"),
        pkg(
            "sha-app",
            &[
                ("lib/alice/app/helper.sld", "(define-library (alice app helper) (export helper-val) (import (scheme base)) (begin (define helper-val 100)))"),
                ("sub/util.scm", "(require \"../base.scm\") (provide util-val) (define util-val (+ base-val 1))"),
                ("base.scm", "(provide base-val) (define base-val 9)"),
                ("inc.scm", "(define inc-val 5)"),
                (
                    "app.scm",
                    "(import (bob mid) (alice app helper) (techne crypto))
                     (require \"sub/util.scm\")
                     (include \"inc.scm\")
                     (define has-mid (cond-expand ((library (bob mid)) #t) (else #f)))
                     (define has-leaf (cond-expand ((library (carol leaf)) #t) (else #f)))
                     (define has-crypto (cond-expand ((library (techne crypto)) #t) (else #f)))
                     (define result (list mid-val ci-sym helper-val util-val inc-val has-mid has-leaf has-crypto (hex-encode (sha-256 \"\"))))",
                ),
            ],
            &[("bob", "mid")],
        ),
    );

    let bundle = Arc::new(BundleVfs::new(BTreeMap::new(), Some(PackagePrefix::new("alice", "app")), pkgs).unwrap());
    let mut vm = Vm::with_grants(Grants::NONE.with(Capability::Loading));
    vm.install_bundle(bundle);
    vm.eval_bundle_entry("app.scm").unwrap();

    let res = vm.eval_source("result").unwrap();
    assert_eq!(vm.repr(&res), "(42 hello 100 10 5 #t #f #t \"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\")");
    assert!(vm.loaded_module("/pkg/alice/app/app.scm").is_some());
    assert!(vm.loaded_module("/pkg/alice/app/sub/util.scm").is_some());
}

#[test]
fn bundle_rejects_undeclared_transitive_imports() {
    let mut pkgs = BTreeMap::new();
    pkgs.insert(
        PackagePrefix::new("carol", "leaf"),
        pkg(
            "sha-leaf",
            &[("lib/carol/leaf.sld", "(define-library (carol leaf) (export x) (import (scheme base)) (begin (define x 1)))")],
            &[],
        ),
    );
    pkgs.insert(
        PackagePrefix::new("bob", "mid"),
        pkg(
            "sha-mid",
            &[("lib/bob/mid.sld", "(define-library (bob mid) (export y) (import (scheme base) (carol leaf)) (begin (define y (+ x 1))))")],
            &[("carol", "leaf")],
        ),
    );
    pkgs.insert(
        PackagePrefix::new("alice", "app"),
        pkg("sha-app", &[("app.scm", "(import (bob mid)) (import (carol leaf))")], &[("bob", "mid")]),
    );

    let bundle = Arc::new(BundleVfs::new(BTreeMap::new(), Some(PackagePrefix::new("alice", "app")), pkgs).unwrap());
    let mut vm = Vm::with_grants(Grants::NONE.with(Capability::Loading));
    vm.install_bundle(bundle);
    let err = vm.eval_bundle_entry("app.scm").unwrap_err();
    assert!(err.msg.contains("does not depend on (carol leaf)"), "unexpected error: {}", err.msg);
}

#[test]
fn bundle_enforces_provider_ownership_and_name_library() {
    let make_vm = |app_src: &str| {
        let mut pkgs = BTreeMap::new();
        pkgs.insert(
            PackagePrefix::new("bob", "dep"),
            pkg(
                "sha-dep",
                &[("lib/bob/dep.sld", "(define-library (bob dep) (export d) (import (scheme base)) (begin (define d 1)))")],
                &[],
            ),
        );
        pkgs.insert(PackagePrefix::new("alice", "app"), pkg("sha-app", &[("app.scm", app_src)], &[("bob", "dep")]));
        let bundle = Arc::new(BundleVfs::new(BTreeMap::new(), Some(PackagePrefix::new("alice", "app")), pkgs).unwrap());
        let mut vm = Vm::with_grants(Grants::NONE.with(Capability::Loading));
        vm.install_bundle(bundle);
        vm
    };

    // Defining another package's library
    let mut vm = make_vm("(define-library (bob dep) (export d) (import (scheme base)) (begin (define d 99)))");
    let err = vm.eval_bundle_entry("app.scm").unwrap_err();
    assert!(err.msg.contains("owned by package (bob dep)"), "unexpected error: {}", err.msg);

    // Defining own library outside lib/
    let mut vm = make_vm("(define-library (alice app) (export a) (import (scheme base)) (begin (define a 1)))");
    let err = vm.eval_bundle_entry("app.scm").unwrap_err();
    assert!(err.msg.contains("owned by package (alice app)"), "unexpected error: {}", err.msg);

    // Defining unprovided library in locked mode
    let mut vm = make_vm("(define-library (evil forged) (export a) (import (scheme base)) (begin (define a 1)))");
    let err = vm.eval_bundle_entry("app.scm").unwrap_err();
    assert!(err.msg.contains("not provided in locked mode"), "unexpected error: {}", err.msg);

    // Forging with %name-library
    let mut vm = make_vm("(%name-library '(bob dep))");
    let err = vm.eval_bundle_entry("app.scm").unwrap_err();
    assert!(err.msg.contains("owned by package (bob dep)"), "unexpected error: {}", err.msg);

    let mut vm = make_vm("(%name-library '(techne crypto))");
    let err = vm.eval_bundle_entry("app.scm").unwrap_err();
    assert!(err.msg.contains("Techne's own libraries"), "unexpected error: {}", err.msg);

    // OS provider / import declaration mutation is refused when bundle is installed
    let tmp = tempfile::tempdir().unwrap();
    assert!(vm.provide_libraries("eve", "lib", tmp.path()).is_err());
    assert!(vm.declare_imports(tmp.path(), ("eve", "lib"), &[]).is_err());
}

#[test]
fn bundle_rejects_path_escapes_hidden_files_and_ambient_os_libraries() {
    let ambient = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(ambient.path().join("eve")).unwrap();
    std::fs::write(
        ambient.path().join("eve/ambient.sld"),
        "(define-library (eve ambient) (export x) (import (scheme base)) (begin (define x 1)))",
    )
    .unwrap();
    std::fs::write(ambient.path().join("outside.scm"), "(define secret 42)").unwrap();

    let run = |src: &str| {
        let bundle = Arc::new(BundleVfs::from_root_files(files(&[("app.scm", src)])).unwrap());
        let mut vm = Vm::with_grants(Grants::NONE.with(Capability::Loading));
        vm.install_bundle(bundle);
        vm.eval_bundle_entry("app.scm")
    };

    // Escape & hidden paths in require / include
    assert!(run("(require \"../outside.scm\")").is_err());
    assert!(run(&format!("(require {:?})", ambient.path().join("outside.scm").display().to_string())).is_err());
    assert!(run("(require \".secrets.scm\")").is_err());
    assert!(run("(include \"../outside.scm\")").is_err());
    assert!(run(&format!("(include {:?})", ambient.path().join("outside.scm").display().to_string())).is_err());
    assert!(run("(include \".secrets.scm\")").is_err());

    // Ambient OS library is invisible even if TECHNE_LIBRARY_PATH is set
    // (we avoid mutating env globally; check locked refusal directly)
    let err = run("(import (eve ambient))").unwrap_err();
    assert!(err.msg.contains("not provided in locked mode"), "unexpected error: {}", err.msg);

    // cond-expand reports #f for ambient / nonexistent libraries
    let bundle = Arc::new(
        BundleVfs::from_root_files(files(&[("app.scm", "(define ok (cond-expand ((library (eve ambient)) #t) (else #f)))")])).unwrap(),
    );
    let mut vm = Vm::with_grants(Grants::NONE.with(Capability::Loading));
    vm.install_bundle(bundle);
    vm.eval_bundle_entry("app.scm").unwrap();
    let v = vm.eval_source("ok").unwrap();
    assert_eq!(vm.repr(&v), "#f");
}

#[test]
fn bundle_snapshot_isolation_has_no_os_paths() {
    let bundle = Arc::new(
        BundleVfs::from_root_files(files(&[
            ("helper.scm", "(provide double) (define (double x) (* x 2))"),
            ("app.scm", "(require \"helper.scm\") (define (answer x) (double x))"),
        ]))
        .unwrap(),
    );
    let mut vm = Vm::with_grants(Grants::NONE.with(Capability::Loading));
    vm.install_bundle(bundle);
    vm.eval_bundle_entry("app.scm").unwrap();
    vm.set_grant(Capability::Loading, false);

    let snap = vm.snapshot().unwrap();
    let mut w1 = snap.world();
    let mut w2 = snap.world();

    assert!(w1.reading_root().is_none());
    assert!(w1.bundle().is_some());
    assert!(w1.locked_libraries());
    assert!(w1.loaded_module("/bundle/app.scm").is_some());
    assert!(w1.loaded_module("/bundle/helper.scm").is_some());

    let v1 = w1.eval_source("(answer 21)").unwrap();
    let v2 = w2.eval_source("(answer 10)").unwrap();
    assert_eq!(w1.repr(&v1), "42");
    assert_eq!(w2.repr(&v2), "20");

    // Loading remains revoked in snapshot worlds
    assert!(w1.eval_source("(require \"missing.scm\")").is_err());
}
