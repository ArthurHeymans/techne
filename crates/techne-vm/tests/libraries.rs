//! Shipped libraries work without the development shell or upstream sources.
use std::{
    fs,
    process::{Command, Stdio},
    time::Duration,
};

use wait_timeout::ChildExt;

const CASES: &[(&str, &str, &str)] = &[
    ("'(scheme base) '(srfi 1)", "(list (zip '(a b) '(1 2)) (fold + 0 (iota 5)) (take '(a b c) 2))", "(((a 1) (b 2)) 10 (a b))"),
    (
        "'(scheme base) '(srfi 1)",
        "(let* ((tail (list 4 5))
                (joined (append-reverse! (list 1 2 3) tail)))
           (list joined (append-reverse! '() tail)
                 (append-reverse! (list 1 2) '())
                 (append-reverse! (list 1 2) 'tail)
                 (eq? (drop joined 3) tail)))",
        "((3 2 1 4 5) (4 5) (2 1) (2 1 . tail) #t)",
    ),
    (
        "'(scheme base) '(srfi 1)",
        "(let ((circle (circular-list 1 2)))
           (map (lambda (x) (list (proper-list? x) (dotted-list? x) (circular-list? x)))
                (list '() '(1) '(1 2) '(1 . 2) 'atom
                      (circular-list 1) circle (cons 0 circle))))",
        "((#t #f #f) (#t #f #f) (#t #f #f) (#f #t #f) (#f #t #f) (#f #f #t) (#f #f #t) (#f #f #t))",
    ),
    (
        "'(scheme base) '(srfi 1)",
        "(list (call-with-values (lambda () (lset-diff+intersection = '(1 2 3) '(1) '(2))) list)
               (call-with-values (lambda () (lset-diff+intersection! = '(1 2 3) '(1) '(2))) list)
               (call-with-values (lambda () (lset-diff+intersection = '(1 2))) list)
               (call-with-values (lambda () (lset-diff+intersection = '(1 1 2) '(1) '(2))) list)
               (call-with-values (lambda () (lset-diff+intersection = '() '(1))) list))",
        "(((3) (1 2)) ((3) (1 2)) ((1 2) ()) (() (1 1 2)) (() ()))",
    ),
    (
        "'(scheme base) '(srfi 1)",
        "(list (every (lambda (a b) b) '(1 2) '(x y))
               (every (lambda (a b) b) '(1 2) '(x))
               (every (lambda args (error \"empty input\")) '() '(1))
               (let* ((calls 0)
                      (result (every (lambda (a b) (set! calls (+ calls 1)) (and (< a 2) b))
                                     '(1 2 3) '(x y z))))
                 (list result calls)))",
        "(y x #t (#f 2))",
    ),
    (
        "'(scheme base) '(scheme char) '(srfi 113) '(srfi 128)",
        "(let* ((cmp (make-comparator string? string-ci=? string-ci<? string-ci-hash))
                (s (set cmp \"Hello\" \"HELLO\"))
                (b (bag cmp \"Hello\" \"HELLO\")))
           (list (set-size s) (set-contains? s \"hello\") (bag-element-count b \"hello\")))",
        "(1 #t 2)",
    ),
    (
        "'(scheme base) '(srfi 117)",
        "(let ((q (list-queue 1 2)))
           (list-queue-add-back! q 3)
           (list (list-queue-remove-front! q) (list-queue-list q)))",
        "(1 (2 3))",
    ),
    (
        "'(scheme base) '(srfi 128)",
        "(let ((cmp (make-default-comparator)))
           (list (<? cmp '(1 2) '(1 3)) (=? cmp '(1 2) '(1 2))
                 (<? cmp '() '()) (<? cmp '(1 2) '(1 2)) (<? cmp '(1 3) '(1 2))
                 (= (comparator-hash cmp (list 1 2)) (comparator-hash cmp (list 1 2)))))",
        "(#t #t #f #f #f #t)",
    ),
    (
        "'(scheme base) '(srfi 132)",
        "(list (list-sort < '(3 1 2))
               (list-stable-sort (lambda (a b) (< (car a) (car b))) '((2 . a) (1 . b) (2 . c)))
               (vector-sort < '#(3 1 2)))",
        "((1 2 3) ((1 . b) (2 . a) (2 . c)) #(1 2 3))",
    ),
    // Import the vector API without (scheme base), so missing re-exports
    // cannot be hidden by the base library or the REPL's root bindings.
    (
        "'(only (scheme base) list +) '(srfi 133)",
        "(list (vector-fold + 0 (vector 1 2 3))
               (vector-copy (vector 1 2 3) 1)
               (vector-map (lambda (x) (+ x 1)) (vector 1 2))
               (vector->string (string->vector \"hello\")))",
        "(6 #(2 3) #(2 3) \"hello\")",
    ),
    (
        "'(scheme base) '(srfi 151)",
        "(list (bitwise-and 255 15) (bitwise-ior 1 4)
               (arithmetic-shift 1 80) (bit-count -1) (bits #t #f #t))",
        "(15 5 1208925819614629174706176 0 5)",
    ),
    (
        "'(scheme base) '(techne crypto)",
        "(list (hex-encode (sha-256 \"\")) (base64-encode \"foo\"))",
        "(\"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\" \"Zm9v\")",
    ),
    ("'(scheme base) '(techne uuid)", "(list (uuid? (uuid-v4)) (uuid? (uuid-v7)))", "(#t #t)"),
    ("'(scheme base) '(techne time)", "(instant->rfc3339 0 \"Asia/Tokyo\")", "\"1970-01-01T09:00:00.000+09:00\""),
];

#[test]
fn bundled_libraries() {
    let dir = std::env::temp_dir().join(format!("techne-bundled-libraries-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    for (i, (imports, expression, expected)) in CASES.iter().enumerate() {
        let file = dir.join(format!("{i}.scm"));
        fs::write(&file, format!("(write (eval '{expression} (environment {imports}))) (newline)\n")).unwrap();
        for (jit, stress) in [("0", "0"), ("1", "0"), ("0", "1")] {
            let mut child = Command::new(env!("CARGO_BIN_EXE_techne-vm"))
                .arg(&file)
                .current_dir(&dir)
                .env_remove("TECHNE_LIBRARY_PATH")
                .env_remove("TECHNE_R7RS_TESTS")
                .env_remove("TECHNE_SRFI_SOURCES")
                .env("TECHNE_JIT", jit)
                .env("TECHNE_GC_STRESS", stress)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let finished = child.wait_timeout(Duration::from_secs(30)).unwrap().is_some();
            if !finished {
                child.kill().unwrap();
            }
            let out = child.wait_with_output().unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(
                finished && out.status.success() && stdout.trim_end() == *expected,
                "imports={imports}, jit={jit}, gc={stress}, finished={finished}:\n{stdout}\n{}",
                String::from_utf8_lossy(&out.stderr),
            );
        }
    }
    fs::remove_dir_all(dir).unwrap();
}

/// A program in a package directory imports the package's libraries, the
/// manifest above it read first (`cli::use_package_of`), by Techne's own
/// `(techne package)`: not one in the working directory the program is
/// started from, which would run before the program asked for anything.
#[test]
fn a_package_is_used_by_techne_s_own_library() {
    let root = std::env::temp_dir().join(format!("techne-package-run-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    for d in ["pkg/lib/acme/pkg", "elsewhere/techne"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    fs::write(root.join("pkg/techne-package.scm"), "(package (acme pkg))").unwrap();
    fs::write(
        root.join("pkg/lib/acme/pkg/say.sld"),
        "(define-library (acme pkg say) (export said) (import (scheme base)) (begin (define said \"ran\")))",
    )
    .unwrap();
    fs::write(root.join("pkg/main.scm"), "(import (scheme base) (scheme write) (acme pkg say)) (display said)").unwrap();
    fs::write(
        root.join("elsewhere/techne/package.sld"),
        "(define-library (techne package) (export use-package-directory!) (import (scheme base) (scheme write))
           (begin (display \"hijacked \") (define (use-package-directory! directory) #f)))",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_techne-vm"))
        .arg(root.join("pkg/main.scm"))
        .current_dir(root.join("elsewhere"))
        .env_remove("TECHNE_LIBRARY_PATH")
        .env_remove("TECHNE_LISP_DIR")
        .output()
        .unwrap();
    fs::remove_dir_all(&root).unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "ran");
}

/// Techne's own Lisp is found where an installation puts it, beside the
/// program, or where `TECHNE_LISP_DIR` says, before the source tree.
#[test]
fn own_lisp_where_installed() {
    let root = std::env::temp_dir().join(format!("techne-installed-{}", std::process::id()));
    let (bin, installed, elsewhere) = (root.join("bin"), root.join("share/techne/lisp/mine"), root.join("elsewhere/mine"));
    for d in [&bin, &installed, &elsewhere] {
        fs::create_dir_all(d).unwrap();
    }
    let program = bin.join("techne-vm");
    fs::copy(env!("CARGO_BIN_EXE_techne-vm"), &program).unwrap();
    let library =
        |where_: &str| format!("(define-library (mine lib) (export where) (import (scheme base)) (begin (define where {where_:?})))");
    fs::write(installed.join("lib.sld"), library("installed")).unwrap();
    fs::write(elsewhere.join("lib.sld"), library("elsewhere")).unwrap();
    let file = root.join("main.scm");
    fs::write(&file, "(import (scheme base) (scheme write) (mine lib)) (write where)").unwrap();
    let run = |lisp: Option<&std::path::Path>| {
        let mut command = Command::new(&program);
        command.arg(&file).current_dir(&root).env_remove("TECHNE_LIBRARY_PATH").env_remove("TECHNE_LISP_DIR");
        if let Some(dir) = lisp {
            command.env("TECHNE_LISP_DIR", dir);
        }
        let out = command.output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    };
    assert_eq!(run(None), "\"installed\"");
    assert_eq!(run(Some(&root.join("elsewhere"))), "\"elsewhere\"");
    fs::remove_dir_all(root).unwrap();
}

/// Inline `define-library` and `%name-library` cannot spoof a registered package
/// provider or Techne stdlib library; importing directories cannot shadow stdlib;
/// and `include` / `include-ci` inside a package rejects hidden files and paths
/// escaping the package directory.
#[test]
fn provider_ownership_stdlib_shadowing_and_include_containment() {
    let root = std::env::temp_dir().join(format!("techne-vm-security-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("dep/lib/acme/dep")).unwrap();
    fs::create_dir_all(root.join("evil/lib/acme/evil/techne")).unwrap();
    fs::create_dir_all(root.join("outside")).unwrap();

    fs::write(root.join("dep/techne-package.scm"), "(package (acme dep) (version \"0.1.0\"))").unwrap();
    fs::write(
        root.join("dep/lib/acme/dep/real.sld"),
        "(define-library (acme dep real) (export real-val) (import (scheme base)) (begin (define real-val 'real)))",
    )
    .unwrap();

    // Package shipping a shadowed `techne/http.sld` right beside its own library,
    // plus hidden and escaping include targets.
    fs::write(root.join("evil/techne-package.scm"), "(package (acme evil) (version \"0.1.0\") (depends ((acme dep) (path \"../dep\"))))")
        .unwrap();
    fs::write(
        root.join("evil/lib/acme/evil/techne/http.sld"),
        "(define-library (techne http) (export hijacked) (import (scheme base)) (begin (define hijacked #t)))",
    )
    .unwrap();
    fs::write(
        root.join("evil/lib/acme/evil/use-stdlib.sld"),
        "(define-library (acme evil use-stdlib) (export ok) (import (scheme base) (techne http)) (begin (define ok (procedure? read-request))))",
    )
    .unwrap();
    fs::write(root.join("evil/lib/acme/evil/.secret.scm"), "(define secret 'leaked)").unwrap();
    fs::write(
        root.join("evil/lib/acme/evil/inc-hidden.sld"),
        "(define-library (acme evil inc-hidden) (export secret) (import (scheme base)) (include \".secret.scm\"))",
    )
    .unwrap();
    fs::write(root.join("outside/escape.scm"), "(define escaped 'leaked)").unwrap();
    fs::write(
        root.join("evil/lib/acme/evil/inc-escape.sld"),
        "(define-library (acme evil inc-escape) (export escaped) (import (scheme base)) (include \"../../../../outside/escape.scm\"))",
    )
    .unwrap();

    let run = |script_name: &str, code: &str| {
        let script = root.join("evil").join(script_name);
        fs::write(&script, code).unwrap();
        Command::new(env!("CARGO_BIN_EXE_techne-vm"))
            .arg(&script)
            .current_dir(&root)
            .env_remove("TECHNE_LIBRARY_PATH")
            .env_remove("TECHNE_LISP_DIR")
            .output()
            .unwrap()
    };

    // 1. Importing `(techne http)` from a package that ships `techne/http.sld` beside its library
    //    must load real stdlib `(techne http)`, never the local shadow!
    let out = run("check-stdlib.scm", "(import (scheme base) (scheme write) (acme evil use-stdlib)) (write ok)");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "#t");

    // 2. Inline `define-library` spoofing a registered provider `(acme dep spoof)` is rejected.
    let out = run("spoof-provider.scm", "(define-library (acme dep spoof) (export x) (import (scheme base)) (begin (define x 1)))");
    assert!(!out.status.success(), "inline define-library must not spoof registered provider");

    // 3. Inline `define-library` spoofing a stdlib module `(techne http)` is rejected.
    let out = run("spoof-stdlib.scm", "(define-library (techne http) (export x) (import (scheme base)) (begin (define x 1)))");
    assert!(!out.status.success(), "inline define-library must not spoof stdlib");

    // 4. `%name-library` spoofing a registered provider or stdlib is rejected.
    let out = run("rename-provider.scm", "(import (scheme base)) (%name-library (make-module 'fake) '(acme dep spoof))");
    assert!(!out.status.success(), "%name-library must not spoof registered provider");
    let out = run("rename-stdlib.scm", "(import (scheme base)) (%name-library (make-module 'fake) '(techne http))");
    assert!(!out.status.success(), "%name-library must not spoof stdlib");

    // 5. Hidden file `include` inside a package library is rejected.
    let out = run("use-hidden-inc.scm", "(import (scheme base) (acme evil inc-hidden)) secret");
    assert!(!out.status.success(), "hidden file include inside package must be rejected");

    // 6. Escaping `include` outside the package directory is rejected.
    let out = run("use-escape-inc.scm", "(import (scheme base) (acme evil inc-escape)) escaped");
    assert!(!out.status.success(), "escaping include outside package must be rejected");

    fs::remove_dir_all(&root).unwrap();
}

/// Regression tests for BundleVfs security and semantics:
/// 1. Caller-supplied filenames in `eval-source` / `load-module-from` cannot
///    forge package or stdlib ownership or canonicalize host paths.
/// 2. Direct-dependency enforcement is fail-closed for `(environment ...)`,
///    `(eval ... "/pkg/...")`, `(in-module "/pkg/...")`, and unattributed callers.
/// 3. Legacy `BundleVfs` (`root_package: None`) imports sibling `.sld` libraries.
/// 4. Nested `include-library-declarations` resolves relative to the included
///    file's own directory (both in `BundleVfs` and on the host filesystem).
#[test]
fn bundle_provenance_dep_isolation_legacy_sld_and_nested_include() {
    use std::{collections::BTreeMap, sync::Arc};
    use techne_vm::{
        bundle::{BundlePackage, BundleVfs, PackagePrefix},
        vm::{Capability, Grants, Vm},
    };

    let files_map = |entries: &[(&str, &[u8])]| -> BTreeMap<String, Arc<[u8]>> {
        entries.iter().map(|(k, v)| ((*k).into(), Arc::<[u8]>::from(*v))).collect()
    };

    let make_pkg_bundle = |app_source: &str| {
        let mut pkgs = BTreeMap::new();
        pkgs.insert(
            PackagePrefix::new("acme", "app"),
            BundlePackage {
                tree_sha256: "a".repeat(64),
                declared_deps: vec![PackagePrefix::new("acme", "mid")],
                files: files_map(&[
                    ("techne-package.scm", b"(package (acme app) (version \"0.1.0\"))"),
                    ("app.scm", app_source.as_bytes()),
                ]),
            },
        );
        pkgs.insert(
            PackagePrefix::new("acme", "mid"),
            BundlePackage {
                tree_sha256: "b".repeat(64),
                declared_deps: vec![PackagePrefix::new("acme", "leaf")],
                files: files_map(&[
                    ("techne-package.scm", b"(package (acme mid) (version \"0.1.0\"))"),
                    ("lib/acme/mid.sld", br#"(define-library (acme mid) (include-library-declarations "decls/outer.inc"))"#),
                    (
                        "lib/acme/decls/outer.inc",
                        br#"(export mid-val)
                            (import (scheme base) (acme leaf))
                            (include-library-declarations "inner.inc")"#,
                    ),
                    ("lib/acme/decls/inner.inc", br#"(begin (define mid-val (string-append "mid+" leaf-val)))"#),
                ]),
            },
        );
        pkgs.insert(
            PackagePrefix::new("acme", "leaf"),
            BundlePackage {
                tree_sha256: "c".repeat(64),
                declared_deps: vec![],
                files: files_map(&[
                    ("techne-package.scm", b"(package (acme leaf) (version \"0.1.0\"))"),
                    (
                        "lib/acme/leaf.sld",
                        br#"(define-library (acme leaf)
                              (export leaf-val)
                              (import (scheme base))
                              (begin (define leaf-val "leaf")))"#,
                    ),
                ]),
            },
        );
        Arc::new(BundleVfs::new(BTreeMap::new(), Some(PackagePrefix::new("acme", "app")), pkgs).expect("valid packaged bundle"))
    };

    // (1) Valid direct import + nested `include-library-declarations` inside `BundleVfs`.
    {
        let mut vm = Vm::with_grants(Grants::NONE.with(Capability::Loading));
        vm.install_bundle(make_pkg_bundle("(import (scheme base) (acme mid)) mid-val"));
        let v = vm.eval_bundle_entry("app.scm").expect("valid packaged entry");
        assert_eq!(vm.get::<String>(&v).unwrap(), "mid+leaf");

        // Attributed root package (acme app) can dynamically import direct dep (acme mid)
        // via (environment ...), but fails closed for undeclared transitive dep (acme leaf).
        let err = vm.eval_source("(eval 'leaf-val (environment '(acme leaf)))").expect_err("undeclared transitive env import must fail");
        assert!(err.msg.contains("does not depend on (acme leaf)"), "{}", err.msg);

        // Truly unattributed caller (fresh USER_MODULE with bundle installed) fails closed
        // even for a valid bundle package like (acme mid).
        let mut unattributed = Vm::with_grants(Grants::NONE.with(Capability::Loading));
        unattributed.install_bundle(make_pkg_bundle("(import (scheme base)) #t"));
        let err =
            unattributed.eval_source("(eval 'mid-val (environment '(acme mid)))").expect_err("unattributed bundle import must fail closed");
        assert!(err.msg.contains("unattributed") || err.msg.contains("not allowed") || err.msg.contains("depend"), "{}", err.msg);
    }

    // (2) Spoofed `eval-source` filename claiming `/pkg/acme/leaf/...` or stdlib path cannot forge ownership.
    {
        let stdlib_spoof = techne_vm::library::lisp_dir().join("techne/spoof.sld");
        let spoof_src = r#"(import (scheme base))
               (eval-source "(define-library (acme leaf spoof) (export x) (import (scheme base)) (begin (define x 1)))"
                            "user" "/pkg/acme/leaf/lib/acme/leaf/spoof.sld")"#;
        let mut vm = Vm::with_grants(Grants::NONE.with(Capability::Loading));
        vm.install_bundle(make_pkg_bundle(spoof_src));
        assert!(vm.eval_bundle_entry("app.scm").is_err(), "eval-source spoofed /pkg/... filename must be rejected");

        let stdlib_src = format!(
            r#"(import (scheme base))
               (eval-source "(define-library (techne spoof) (export x) (import (scheme base)) (begin (define x 1)))"
                            "user" {})"#,
            techne_vm::reader::string_repr(&stdlib_spoof.display().to_string())
        );
        let mut vm2 = Vm::with_grants(Grants::NONE.with(Capability::Loading));
        vm2.install_bundle(make_pkg_bundle(&stdlib_src));
        assert!(vm2.eval_bundle_entry("app.scm").is_err(), "eval-source spoofed stdlib filename must be rejected");
    }

    // (3) `load-module-from` with spoofed `/pkg/acme/leaf/...` or host OS path is rejected under BundleVfs.
    {
        let host_tmp = tempfile::NamedTempFile::new().unwrap();
        let lmf_pkg = r#"(import (scheme base))
            (load-module-from "/pkg/acme/leaf/lib/acme/leaf/spoof.sld"
                              "(define-library (acme leaf spoof) (export x) (import (scheme base)) (begin (define x 1)))")"#;
        let mut vm = Vm::with_grants(Grants::NONE.with(Capability::Loading));
        vm.install_bundle(make_pkg_bundle(lmf_pkg));
        assert!(vm.eval_bundle_entry("app.scm").is_err(), "load-module-from spoofed /pkg/... must be rejected");

        let lmf_host = format!(
            r#"(import (scheme base)) (load-module-from {} "(define x 1)")"#,
            techne_vm::reader::string_repr(&host_tmp.path().display().to_string())
        );
        let mut vm2 = Vm::with_grants(Grants::NONE.with(Capability::Loading));
        vm2.install_bundle(make_pkg_bundle(&lmf_host));
        assert!(vm2.eval_bundle_entry("app.scm").is_err(), "load-module-from host path must be rejected in bundle mode");
    }

    // (4) Dynamic `(environment '(acme leaf))`, `(eval ... "/pkg/acme/leaf/...")`, and `(in-module "/pkg/acme/leaf/...")`
    //     cannot bypass direct-dependency boundaries from root `(acme app)`.
    for attack in [
        r#"(import (scheme base) (acme mid)) (eval 'leaf-val (environment '(acme leaf)))"#,
        r#"(import (scheme base) (acme mid)) (eval 'leaf-val "/pkg/acme/leaf/lib/acme/leaf.sld")"#,
        r#"(import (scheme base) (acme mid)) (in-module "/pkg/acme/leaf/lib/acme/leaf.sld")"#,
    ] {
        let mut vm = Vm::with_grants(Grants::NONE.with(Capability::Loading));
        vm.install_bundle(make_pkg_bundle(attack));
        let err = vm.eval_bundle_entry("app.scm").expect_err(&format!("bundle boundary bypass must fail: {attack}"));
        assert!(
            err.msg.contains("depend") || err.msg.contains("not permitted") || err.msg.contains("not allowed"),
            "{attack}: unexpected error: {}",
            err.msg
        );
    }

    // (5) Legacy BundleVfs (`root_package: None`) imports sibling `.sld` libraries cleanly.
    {
        let legacy = Arc::new(
            BundleVfs::new(
                files_map(&[
                    ("app.scm", b"(import (scheme base) (helper) (sub nested)) (string-append h-val \":\" n-val)"),
                    ("helper.sld", b"(define-library (helper) (export h-val) (import (scheme base)) (begin (define h-val \"sib\")))"),
                    (
                        "sub/nested.sld",
                        b"(define-library (sub nested) (export n-val) (import (scheme base)) (begin (define n-val \"nest\")))",
                    ),
                ]),
                None,
                BTreeMap::new(),
            )
            .expect("valid legacy bundle with sibling .sld"),
        );
        let mut vm = Vm::with_grants(Grants::NONE.with(Capability::Loading));
        vm.install_bundle(legacy);
        let v = vm.eval_bundle_entry("app.scm").expect("legacy bundle with sibling .sld must load");
        assert_eq!(vm.get::<String>(&v).unwrap(), "sib:nest");
    }

    // (6) Nested `include-library-declarations` on host filesystem resolves relative to the included file's directory.
    {
        let dir = tempfile::tempdir().unwrap();
        let pkg = dir.path().join("pkg");
        fs::create_dir_all(pkg.join("lib/acme/inc/decls")).unwrap();
        fs::write(pkg.join("techne-package.scm"), "(package (acme inc) (version \"0.1.0\"))").unwrap();
        fs::write(pkg.join("lib/acme/inc.sld"), "(define-library (acme inc) (include-library-declarations \"inc/decls/outer.inc\"))")
            .unwrap();
        fs::write(
            pkg.join("lib/acme/inc/decls/outer.inc"),
            "(export inc-val) (import (scheme base)) (include-library-declarations \"inner.inc\")",
        )
        .unwrap();
        fs::write(pkg.join("lib/acme/inc/decls/inner.inc"), "(begin (define inc-val \"host-nested-ok\"))").unwrap();
        fs::write(pkg.join("main.scm"), "(import (scheme base) (scheme write) (acme inc)) (display inc-val)").unwrap();

        let out = Command::new(env!("CARGO_BIN_EXE_techne-vm"))
            .arg(pkg.join("main.scm"))
            .current_dir(dir.path())
            .env_remove("TECHNE_LIBRARY_PATH")
            .env_remove("TECHNE_LISP_DIR")
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(String::from_utf8(out.stdout).unwrap(), "host-nested-ok");
    }
}
