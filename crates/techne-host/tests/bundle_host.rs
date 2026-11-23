//! Integration tests for Phase 3 VM Bundle VFS loading, zero-tmpfs worker
//! execution, locked packaged cloud deployments, tree-sha256 caching,
//! binding contract validation, and server-side tree verification.

use std::{collections::BTreeMap, path::Path, sync::Arc, time::Duration};

use techne_host::{
    Deployment, Host, Limits,
    package::{PackageSpec, canonical_tree_sha256, load_verified_packaged_trees, read_clean_package_tree},
    supervisor::{Options, ResourceConfinement, Sandboxing, Supervisor},
    wire::BUNDLE_BYTES,
};
use techne_vm::{
    bundle::{BundlePackage, BundleVfs, PackagePrefix},
    data::Data,
    vm::Vm,
};

fn options(env: &[(&str, &str)], sandboxing: Sandboxing) -> Options {
    Options {
        program: env!("CARGO_BIN_EXE_techne-worker").into(),
        args: Vec::new(),
        env: env.iter().map(|(k, v)| (k.into(), v.into())).collect(),
        sandboxing,
        resources: ResourceConfinement::IfAvailable,
        cgroup: techne_sandbox::cgroup::Limits { memory: 512 << 20, pids: 256, cpu: None },
        wall: Duration::from_secs(10),
        dev: false,
        egress: Default::default(),
    }
}

fn request(path: &str) -> Data {
    Data::Dict(vec![(Data::Symbol("method".into()), Data::String("GET".into())), (Data::Symbol("path".into()), Data::String(path.into()))])
}

fn answer(response: &Data) -> (i64, String) {
    let entries = match response {
        Data::Dict(entries) => entries,
        Data::String(body) => return (200, body.clone()),
        _ => panic!("{response:?}"),
    };
    let get = |k: &str| entries.iter().find(|(key, _)| matches!(key, Data::Symbol(s) if s == k)).map(|(_, v)| v);
    let status = match get("status") {
        Some(Data::Int(s)) => *s,
        _ => 200,
    };
    let body = match get("body") {
        Some(Data::String(b)) => b.clone(),
        other => format!("{other:?}"),
    };
    (status, body)
}

fn write(base: &Path, rel: &str, text: &str) {
    let path = base.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, text).unwrap();
}

fn lock_project(dir: &Path) {
    let mut vm = Vm::new();
    vm.eval_source("(import (techne lock))").unwrap();
    vm.eval_source(&format!("(lock-project! {:?})", dir.display().to_string())).unwrap();
}

fn pkg_files(pairs: &[(&str, &str)]) -> BTreeMap<String, Arc<[u8]>> {
    pairs.iter().map(|(k, v)| ((*k).to_string(), Arc::<[u8]>::from(v.as_bytes()))).collect()
}

/// Direct `Deployment::load_bundle` loads a packaged `BundleVfs` strictly in
/// memory with zero OS paths and enforces declared dependency boundaries.
#[test]
fn deployment_loads_packaged_bundle_vfs_strictly() {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async {
        let app_prefix = PackagePrefix::new("acme", "app");
        let mid_prefix = PackagePrefix::new("acme", "mid");
        let core_prefix = PackagePrefix::new("acme", "core");

        let mut packages = BTreeMap::new();
        packages.insert(
            core_prefix.clone(),
            BundlePackage {
                tree_sha256: "0".repeat(64),
                files: pkg_files(&[(
                    "lib/acme/core.sld",
                    "(define-library (acme core) (export core-tag) (import (techne)) (begin (define core-tag \"core-v1\")))",
                )]),
                declared_deps: vec![],
            },
        );
        packages.insert(
            mid_prefix.clone(),
            BundlePackage {
                tree_sha256: "1".repeat(64),
                files: pkg_files(&[
                    (
                        "lib/acme/mid.sld",
                        "(define-library (acme mid) (export mid-tag) (import (techne) (acme core)) (include \"mid-body.scm\"))",
                    ),
                    ("lib/acme/mid-body.scm", "(define (mid-tag) (string-append \"mid+\" core-tag))"),
                ]),
                declared_deps: vec![core_prefix.clone()],
            },
        );
        packages.insert(
            app_prefix.clone(),
            BundlePackage {
                tree_sha256: "2".repeat(64),
                files: pkg_files(&[
                    ("helper.scm", "(provide wrap) (define (wrap s) (string-append \"[\" s \"]\"))"),
                    (
                        "app.scm",
                        r#"
(import (acme mid))
(require "helper.scm")
(provide fetch)
(define core-visible? (cond-expand ((library (acme core)) "yes") (else "no")))
(define (fetch req env ctx)
  (dict 'status 200 'body (string-append (wrap (mid-tag)) ":" core-visible?)))
"#,
                    ),
                ]),
                declared_deps: vec![mid_prefix.clone()],
            },
        );

        let bundle = Arc::new(BundleVfs::new(BTreeMap::new(), Some(app_prefix.clone()), packages.clone()).unwrap());
        let dep = Deployment::load_bundle(bundle, "app.scm", Limits::default()).unwrap();
        let host = Host::new();
        host.publish("app", dep);
        let driver = host.clone();
        let handle = tokio::task::spawn_local(async move { driver.run(None).await });
        let resp = host.fetch("app", request("/"), None).await.unwrap();
        assert_eq!(answer(&resp.head), (200, "[mid+core-v1]:no".to_string()));
        handle.abort();

        // Attempting to import undeclared transitive dependency (acme core) directly fails.
        let mut bad_packages = packages;
        bad_packages.get_mut(&app_prefix).unwrap().files.insert(
            "app.scm".into(),
            Arc::<[u8]>::from(b"(import (acme core)) (provide fetch) (define (fetch r e c) (dict 'status 200 'body core-tag))".as_slice()),
        );
        let bad_bundle = Arc::new(BundleVfs::new(BTreeMap::new(), Some(app_prefix), bad_packages).unwrap());
        let err = Deployment::load_bundle(bad_bundle, "app.scm", Limits::default()).err().expect("undeclared transitive import must fail");
        assert!(err.msg.contains("does not depend on (acme core)"), "{}", err.msg);
    });
}

/// Workers load both legacy and packaged deployments strictly from in-memory
/// `BundleVfs` without unpacking anything into `TMPDIR/bundles-*`.
#[test]
fn worker_loads_without_writing_any_tmpfs_bundle_files() {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async {
        let tmp = tempfile::tempdir().unwrap();
        let tmp_str = tmp.path().display().to_string();
        let sup = Supervisor::start(options(&[("TMPDIR", &tmp_str)], Sandboxing::Off)).await.unwrap();

        // Legacy plain-file deployment.
        let legacy_dir = tempfile::tempdir().unwrap();
        write(legacy_dir.path(), "app.scm", "(provide fetch) (define (fetch r e c) (dict 'status 200 'body \"legacy-ok\"))");
        sup.deploy("legacy", &legacy_dir.path().join("app.scm")).await.unwrap();
        let r = sup.fetch("legacy", request("/"), None).await.unwrap();
        assert_eq!(answer(&r.head), (200, "legacy-ok".into()));

        // Packaged zero-dep deployment.
        let pkg_dir = tempfile::tempdir().unwrap();
        write(pkg_dir.path(), "techne-package.scm", "(package (acme zero) (version \"0.1.0\") (cloud \"app.scm\"))");
        write(pkg_dir.path(), "app.scm", "(provide fetch) (define (fetch r e c) (dict 'status 200 'body \"pkg-ok\"))");
        let p = sup.prepare_directory("pkg", pkg_dir.path()).await.unwrap();
        sup.publish("pkg", &p.id, None).unwrap();
        let r = sup.fetch("pkg", request("/"), None).await.unwrap();
        assert_eq!(answer(&r.head), (200, "pkg-ok".into()));

        // Zero bundle directories or files were ever created under TMPDIR.
        let entries: Vec<_> = std::fs::read_dir(tmp.path()).unwrap().collect();
        assert!(entries.is_empty(), "worker unexpectedly wrote into TMPDIR: {entries:?}");
    });
}

/// Full packaged cloud workflow: locked transitive deps, binding-kind contract
/// validation, dependency `(cloud ...)` clause ignored, tree-sha256 caching,
/// deterministic deployment IDs, and packaged rollback via `%cloud-bundle`.
#[test]
fn packaged_cloud_app_with_locked_deps_cache_bindings_and_rollback() {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async {
        let workspace = tempfile::tempdir().unwrap();
        let core_dir = workspace.path().join("core");
        let helper_dir = workspace.path().join("helper");
        let app_dir = workspace.path().join("app");

        write(&core_dir, "techne-package.scm", "(package (acme core) (version \"0.1.0\"))");
        write(
            &core_dir,
            "lib/acme/core.sld",
            "(define-library (acme core) (export core-val) (import (techne)) (begin (define core-val \"C1\")))",
        );

        // Dependency also declares a `(cloud ...)` clause with a bogus binding:
        // must be completely ignored because only root's `(cloud ...)` governs.
        write(
            &helper_dir,
            "techne-package.scm",
            r#"(package (acme helper)
                 (version "0.1.0")
                 (depends ((acme core) (path "../core")))
                 (cloud "ignored.scm" (bindings (bogus kv))))"#,
        );
        write(&helper_dir, "ignored.scm", "(provide fetch) (define (fetch r e c) \"ignored\")");
        write(
            &helper_dir,
            "lib/acme/helper.sld",
            "(define-library (acme helper) (export helper-val) (import (techne) (acme core)) (begin (define (helper-val) (string-append \"H+\" core-val))))",
        );

        write(
            &app_dir,
            "techne-package.scm",
            r#"(package (acme app)
                 (version "0.1.0")
                 (depends ((acme helper) (path "../helper")))
                 (cloud "app.scm" (bindings (greeting text) (api-key secret))))"#,
        );
        write(
            &app_dir,
            "app.scm",
            r#"(import (acme helper))
(provide fetch)
(define (fetch req env ctx)
  (dict 'status 200
        'body (string-append "v1:" (dict-ref env 'greeting) ":" (dict-ref env 'api-key) ":" (helper-val))))"#,
        );

        let sup = Supervisor::start(options(&[], Sandboxing::IfAvailable)).await.unwrap();

        // Missing techne.lock when dependencies exist fails clearly.
        let err = sup.prepare_directory("app", &app_dir).await.unwrap_err();
        assert!(err.contains("techne.lock") && err.contains("techne lock"), "{err}");

        // Lock the project offline.
        lock_project(&app_dir);

        // Missing manifest.scm fails contract validation (missing declared bindings).
        let err = sup.prepare_directory("app", &app_dir).await.unwrap_err();
        assert!(err.contains("Missing binding") || err.contains("greeting"), "{err}");

        // Mismatched binding kind (text instead of secret) fails clearly.
        write(&app_dir, "manifest.scm", r#"{bindings {greeting (text "hi") api-key (text "not-a-secret")}}"#);
        let err = sup.prepare_directory("app", &app_dir).await.unwrap_err();
        assert!(err.contains("api-key") && err.contains("secret") && err.contains("text"), "{err}");

        // Extra undeclared binding fails clearly.
        write(&app_dir, "manifest.scm", r#"{bindings {greeting (text "hi") api-key (secret "KEY") extra (text "nope")}}"#);
        write(&app_dir, ".secrets.scm", r#"{"KEY" "s3cret"}"#);
        let err = sup.prepare_directory("app", &app_dir).await.unwrap_err();
        assert!(err.contains("extra") && err.contains("Undeclared"), "{err}");

        // Exact matching bindings succeed (and helper's `(cloud ... (bindings (bogus kv)))` is ignored).
        write(&app_dir, "manifest.scm", r#"{bindings {greeting (text "hi") api-key (secret "KEY")}}"#);

        // Compute expected dep tree hashes.
        let mut total = 0;
        let helper_sha = canonical_tree_sha256(&read_clean_package_tree(&helper_dir, &mut total, BUNDLE_BYTES).unwrap());
        let core_sha = canonical_tree_sha256(&read_clean_package_tree(&core_dir, &mut total, BUNDLE_BYTES).unwrap());

        // Drive via front world + control.scm so we also exercise `%cloud-prepare-directory`,
        // `%cloud-bundle`, and `%cloud-prepare-bundle` rollback on a packaged deployment.
        let mut front = Vm::new();
        sup.install_control(&mut front);
        let control_path = techne_vm::library::lisp_dir().join("cloud/control.scm").display().to_string();
        let state_dir = workspace.path().join("state");
        front
            .eval_source(&format!(
                r#"(require {control_path:?})
                   (define ctl (make-control "app" {:?} {:?}))
                   (spawn (lambda () (control-deploy! ctl)))"#,
                app_dir.display().to_string(),
                state_dir.display().to_string(),
            ))
            .unwrap();
        sup.run(&mut front).await.unwrap();
        let v1_id = sup.route("app").expect("v1 should be published");

        // Dependencies are cached by canonical tree-sha256 in Supervisor.
        assert!(sup.has_cached_tree(&helper_sha));
        assert!(sup.has_cached_tree(&core_sha));

        let resp = sup.fetch("app", request("/"), None).await.unwrap();
        assert_eq!(answer(&resp.head), (200, "v1:hi:s3cret:H+C1".into()));

        // Deterministic deployment ID: re-preparing unchanged package yields the same ID.
        let again = sup.prepare_directory("app", &app_dir).await.unwrap();
        assert_eq!(again.id, v1_id);
        assert!(again.output.is_empty());

        // Also works when pointing directly at the package's entry file.
        let by_entry = sup.prepare_directory("app", &app_dir.join("app.scm")).await.unwrap();
        assert_eq!(by_entry.id, v1_id);

        // Pointing at a different file inside the package fails clearly.
        write(&app_dir, "other.scm", "(define x 1)");
        let err = sup.prepare_directory("app", &app_dir.join("other.scm")).await.unwrap_err();
        assert!(err.contains("does not match package (cloud ...) entry"), "{err}");
        std::fs::remove_file(app_dir.join("other.scm")).unwrap();

        // Deploy v2.
        write(
            &app_dir,
            "app.scm",
            r#"(import (acme helper))
(provide fetch)
(define (fetch req env ctx)
  (dict 'status 200
        'body (string-append "v2:" (dict-ref env 'greeting) ":" (helper-val))))"#,
        );
        front.eval_source("(spawn (lambda () (control-deploy! ctl)))").unwrap();
        sup.run(&mut front).await.unwrap();
        let v2_id = sup.route("app").unwrap();
        assert_ne!(v2_id, v1_id);
        let resp = sup.fetch("app", request("/"), None).await.unwrap();
        assert_eq!(answer(&resp.head), (200, "v2:hi:H+C1".into()));

        // Roll back to v1 via control-rollback! (uses saved Bundle::encode / %cloud-prepare-bundle).
        front.eval_source("(spawn (lambda () (control-rollback! ctl)))").unwrap();
        sup.run(&mut front).await.unwrap();
        assert_eq!(sup.route("app").as_deref(), Some(v1_id.as_str()));
        let resp = sup.fetch("app", request("/"), None).await.unwrap();
        assert_eq!(answer(&resp.head), (200, "v1:hi:s3cret:H+C1".into()));
    });
}

/// Server-side `load_verified_packaged_trees` rejects forged dependency graphs,
/// corrupted cached trees, wrong entry paths, and unreachable smuggled packages.
#[test]
fn load_verified_packaged_trees_rejects_forged_deps_and_corrupted_trees() {
    let dir = tempfile::tempdir().unwrap();
    let trees = dir.path().join("trees");

    // Stage dep tree.
    let dep_tmp = dir.path().join("dep-src");
    write(&dep_tmp, "techne-package.scm", "(package (acme dep) (version \"0.1.0\"))");
    write(&dep_tmp, "lib/acme/dep.sld", "(define-library (acme dep) (export val) (import (techne)) (begin (define val \"ok\")))");
    let mut total = 0;
    let dep_files = read_clean_package_tree(&dep_tmp, &mut total, BUNDLE_BYTES).unwrap();
    let dep_sha = canonical_tree_sha256(&dep_files);
    for (rel, bytes) in &dep_files {
        let p = trees.join(&dep_sha).join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    // Stage root tree.
    let root_tmp = dir.path().join("root-src");
    write(
        &root_tmp,
        "techne-package.scm",
        r#"(package (acme app)
             (version "0.1.0")
             (depends ((acme dep) (path "../dep")))
             (cloud "app.scm" (bindings (greeting text))))"#,
    );
    write(
        &root_tmp,
        "techne.lock",
        &format!(
            r#"(lock
                 (techne "0.1.0")
                 (package (acme dep) "0.1.0"
                   (source (path "../dep"))
                   (tree-sha256 "{dep_sha}")))"#
        ),
    );
    write(&root_tmp, "app.scm", "(import (acme dep)) (provide fetch) (define (fetch r e c) val)");
    let root_files = read_clean_package_tree(&root_tmp, &mut total, BUNDLE_BYTES).unwrap();
    let root_sha = canonical_tree_sha256(&root_files);
    for (rel, bytes) in &root_files {
        let p = trees.join(&root_sha).join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    let root_pkg = ("acme".to_string(), "app".to_string());
    let valid_specs = vec![
        PackageSpec {
            owner: "acme".into(),
            pkg: "app".into(),
            tree_sha256: root_sha.clone(),
            depends: vec![("acme".into(), "dep".into())],
        },
        PackageSpec { owner: "acme".into(), pkg: "dep".into(), tree_sha256: dep_sha.clone(), depends: vec![] },
    ];

    // Valid trees succeed and return root declared bindings.
    let (pkgs, declared) = load_verified_packaged_trees(&trees, "app.scm", &root_pkg, &valid_specs, None).unwrap();
    assert_eq!(pkgs.len(), 2);
    assert_eq!(declared.len(), 1);
    assert_eq!(declared[0].name, "greeting");

    // Wrong entry rejected.
    let err = load_verified_packaged_trees(&trees, "other.scm", &root_pkg, &valid_specs, None).unwrap_err();
    assert!(err.contains("does not match root package (cloud ...) entry"), "{err}");

    // Forged dependency list (omitting declared dep or adding extra dep) rejected.
    let forged = vec![PackageSpec { owner: "acme".into(), pkg: "app".into(), tree_sha256: root_sha.clone(), depends: vec![] }];
    let err = load_verified_packaged_trees(&trees, "app.scm", &root_pkg, &forged, None).unwrap_err();
    assert!(err.contains("dependency list does not match techne-package.scm"), "{err}");

    // Tampered file in cached tree fails tree-sha256 integrity check.
    std::fs::write(trees.join(&dep_sha).join("lib/acme/dep.sld"), "tampered").unwrap();
    let err = load_verified_packaged_trees(&trees, "app.scm", &root_pkg, &valid_specs, None).unwrap_err();
    assert!(err.contains("corrupted"), "{err}");
}
