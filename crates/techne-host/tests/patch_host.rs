//! Integration tests for Mode 2 live in-world library patching in `Host`
//! and `Supervisor` (dev worlds, production refusal, debug-instance isolation,
//! delta tracking, and worker IPC).

use std::{collections::BTreeMap, path::Path, sync::Arc, time::Duration};

use techne_host::{
    Attached, Deployment, Host, Limits,
    supervisor::{Options, ResourceConfinement, Sandboxing, Supervisor},
};
use techne_vm::{
    bundle::{BundlePackage, BundleVfs, PackagePrefix},
    data::Data,
    vm::Vm,
};

struct NoopAttached;

impl Attached for NoopAttached {
    fn poll(&mut self, _vm: &mut Vm) -> bool {
        false
    }
    fn reap(&mut self, _vm: &mut Vm) {}
}

fn pkg_files(pairs: &[(&str, &str)]) -> BTreeMap<String, Arc<[u8]>> {
    pairs.iter().map(|(k, v)| ((*k).to_string(), Arc::<[u8]>::from(v.as_bytes()))).collect()
}

fn request(path: &str) -> Data {
    Data::Dict(vec![(Data::Symbol("method".into()), Data::String("GET".into())), (Data::Symbol("path".into()), Data::String(path.into()))])
}

fn body_of(response: &Data) -> String {
    let entries = match response {
        Data::Dict(entries) => entries,
        Data::String(body) => return body.clone(),
        _ => panic!("{response:?}"),
    };
    entries
        .iter()
        .find(|(k, _)| matches!(k, Data::Symbol(s) if s == "body"))
        .and_then(|(_, v)| match v {
            Data::String(s) => Some(s.clone()),
            _ => None,
        })
        .unwrap_or_else(|| format!("{response:?}"))
}

fn make_bundle() -> Arc<BundleVfs> {
    let bob = PackagePrefix::new("bob", "util");
    let alice = PackagePrefix::new("alice", "app");
    let mut packages = BTreeMap::new();
    packages.insert(
        bob.clone(),
        BundlePackage {
            tree_sha256: "b".repeat(64),
            files: pkg_files(&[(
                "lib/bob/util.sld",
                r#"(define-library (bob util)
  (export next-label)
  (import (techne))
  (begin
    (define counter (vector 0))
    (define (next-label)
      (vector-set! counter 0 (+ (vector-ref counter 0) 1))
      (string-append "v1#" (number->string (vector-ref counter 0))))))"#,
            )]),
            declared_deps: vec![],
        },
    );
    packages.insert(
        alice.clone(),
        BundlePackage {
            tree_sha256: "a".repeat(64),
            files: pkg_files(&[(
                "app.scm",
                r#"(import (bob util))
(provide fetch)
(define (fetch req env ctx)
  (dict 'status 200 'body (next-label)))"#,
            )]),
            declared_deps: vec![bob],
        },
    );
    Arc::new(BundleVfs::new(BTreeMap::new(), Some(alice), packages).unwrap())
}

const BOB_V2: &str = r#"(define-library (bob util)
  (export next-label)
  (import (techne))
  (begin
    (define counter (vector 0))
    (define (next-label)
      (vector-set! counter 0 (+ (vector-ref counter 0) 1))
      (string-append "v2#" (number->string (vector-ref counter 0))))))"#;

const BOB_BAD_STATE: &str = r#"(define-library (bob util)
  (export next-label)
  (import (techne))
  (begin
    (define counter (vector 99))
    (define (next-label)
      (string-append "bad#" (number->string (vector-ref counter 0))))))"#;

#[test]
fn host_patch_dev_world_preserves_state_and_tracks_dirty_deltas() {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async {
        let bundle = make_bundle();
        let dep1 = Deployment::load_bundle(bundle.clone(), "app.scm", Limits::default()).unwrap();
        let host = Host::new();
        host.publish_dev("app", dep1, |_| Ok(Box::new(NoopAttached))).unwrap();
        let driver = host.clone();
        let handle = tokio::task::spawn_local(async move { driver.run(None).await });

        assert!(!host.is_dirty("app"));
        assert!(host.patch_deltas("app").is_empty());

        let r1 = host.fetch("app", request("/"), None).await.unwrap();
        assert_eq!(body_of(&r1.head), "v1#1");

        // Patch `(bob util)` in the dev world.
        let lib = vec!["bob".to_string(), "util".to_string()];
        let report = host.patch_library("app", &lib, BOB_V2, true).unwrap();
        assert_eq!(report.patched, vec!["next-label"]);
        assert!(host.is_dirty("app"));
        assert_eq!(host.patch_deltas("app").get(&lib), Some(&report.source_sha256));

        // Counter survived (now 2), procedure uses v2 immediately!
        let r2 = host.fetch("app", request("/"), None).await.unwrap();
        assert_eq!(body_of(&r2.head), "v2#2");

        // Refused patch does not disturb the live world or delta table.
        let err = host.patch_library("app", &lib, BOB_BAD_STATE, true).unwrap_err();
        assert_eq!(err.reason, "state-initializer-changed");
        assert_eq!(err.binding.as_deref(), Some("counter"));
        let r3 = host.fetch("app", request("/"), None).await.unwrap();
        assert_eq!(body_of(&r3.head), "v2#3");

        // Mode 1 re-publish clears patches and starts a fresh clean dev world.
        let dep2 = Deployment::load_bundle(bundle, "app.scm", Limits::default()).unwrap();
        host.publish_dev("app", dep2, |_| Ok(Box::new(NoopAttached))).unwrap();
        assert!(!host.is_dirty("app"));
        assert!(host.patch_deltas("app").is_empty());
        let r4 = host.fetch("app", request("/"), None).await.unwrap();
        assert_eq!(body_of(&r4.head), "v1#1");

        handle.abort();
    });
}

#[test]
fn host_patch_refuses_production_only_and_isolates_debug_instance() {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async {
        let bundle = make_bundle();
        let dep = Deployment::load_bundle(bundle, "app.scm", Limits::default()).unwrap();
        let host = Host::new();
        host.publish("prod", dep);
        let driver = host.clone();
        let handle = tokio::task::spawn_local(async move { driver.run(None).await });

        let lib = vec!["bob".to_string(), "util".to_string()];

        // Production-only deployment MUST refuse patching.
        let err = host.patch_library("prod", &lib, BOB_V2, true).unwrap_err();
        assert_eq!(err.reason, "no-mutable-world");
        assert!(!host.is_dirty("prod"));

        // Unrouted production request serves v1.
        let r_prod = host.fetch_routed(0, "prod", request("/"), None, false).await.unwrap();
        assert_eq!(body_of(&r_prod.head), "v1#1");

        // Attach a debug instance beside production and patch it.
        let debug_id = host.attach_debug("prod", |_| Ok(Box::new(NoopAttached))).unwrap();
        let report = host.patch_library("prod", &lib, BOB_V2, true).unwrap();
        assert_eq!(report.patched, vec!["next-label"]);
        assert!(host.is_dirty("prod"));

        // Routed debug request gets v2; unrouted production request STILL runs v1!
        let r_debug = host.fetch_routed(0, "prod", request("/"), None, true).await.unwrap();
        assert_eq!(body_of(&r_debug.head), "v2#1");
        let r_prod2 = host.fetch_routed(0, "prod", request("/"), None, false).await.unwrap();
        assert!(body_of(&r_prod2.head).starts_with("v1#"), "{}", body_of(&r_prod2.head));

        // Detaching the debug instance clears patch deltas and restores refusal.
        assert!(host.detach_debug("prod", Some(debug_id)));
        assert!(!host.is_dirty("prod"));
        assert!(host.patch_deltas("prod").is_empty());
        let err2 = host.patch_library("prod", &lib, BOB_V2, true).unwrap_err();
        assert_eq!(err2.reason, "no-mutable-world");

        handle.abort();
    });
}

fn write_file(base: &Path, rel: &str, text: &str) {
    let path = base.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, text).unwrap();
}

fn worker_options(dev: bool) -> Options {
    Options {
        program: env!("CARGO_BIN_EXE_techne-worker").into(),
        args: Vec::new(),
        env: Vec::new(),
        sandboxing: Sandboxing::Off,
        resources: ResourceConfinement::IfAvailable,
        cgroup: techne_sandbox::cgroup::Limits { memory: 512 << 20, pids: 256, cpu: None },
        wall: Duration::from_secs(10),
        dev,
        egress: Default::default(),
    }
}

#[test]
fn supervisor_patch_library_ipc_refuses_production_and_unknown() {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "techne-package.scm", r#"(package (alice app) (version "0.1.0") (cloud "app.scm"))"#);
        write_file(
            dir.path(),
            "lib/alice/app/util.sld",
            r#"(define-library (alice app util)
  (export next-label)
  (import (techne))
  (begin
    (define counter (vector 0))
    (define (next-label)
      (vector-set! counter 0 (+ (vector-ref counter 0) 1))
      (string-append "v1#" (number->string (vector-ref counter 0))))))"#,
        );
        write_file(
            dir.path(),
            "app.scm",
            r#"(import (alice app util))
(provide fetch)
(define (fetch req env ctx)
  (dict 'status 200 'body (next-label)))"#,
        );

        let lib = vec!["alice".to_string(), "app".to_string(), "util".to_string()];
        let patched_src = r#"(define-library (alice app util)
  (export next-label)
  (import (techne))
  (begin
    (define counter (vector 0))
    (define (next-label)
      (vector-set! counter 0 (+ (vector-ref counter 0) 1))
      (string-append "v2#" (number->string (vector-ref counter 0))))))"#;

        let prod_sup = Supervisor::start(worker_options(false)).await.unwrap();

        // Unknown deployment refuses without spawning/mutating anything.
        let err = prod_sup.patch_library("missing", lib.clone(), patched_src.to_string(), true).await.unwrap_err();
        assert_eq!(err.reason, "no-mutable-world");

        // Published production worker refuses Mode 2 patching over IPC with no-mutable-world.
        let p = prod_sup.prepare_directory("app", dir.path()).await.unwrap();
        prod_sup.publish("app", &p.id, None).unwrap();
        let r = prod_sup.fetch("app", request("/"), None).await.unwrap();
        assert_eq!(body_of(&r.head), "v1#1");
        let err = prod_sup.patch_library("app", lib, patched_src.to_string(), true).await.unwrap_err();
        assert_eq!(err.reason, "no-mutable-world");
        let r2 = prod_sup.fetch("app", request("/"), None).await.unwrap();
        assert!(body_of(&r2.head).starts_with("v1#"), "{}", body_of(&r2.head));
    });
}

#[test]
fn deployment_load_bundle_refuses_scheme_patch_library_during_init() {
    let bob = PackagePrefix::new("bob", "util");
    let alice = PackagePrefix::new("alice", "app");
    let mut packages = BTreeMap::new();
    packages.insert(
        bob.clone(),
        BundlePackage {
            tree_sha256: "b".repeat(64),
            files: pkg_files(&[(
                "lib/bob/util.sld",
                r#"(define-library (bob util) (export f) (import (techne)) (begin (define (f) "v1")))"#,
            )]),
            declared_deps: vec![],
        },
    );
    packages.insert(
        alice.clone(),
        BundlePackage {
            tree_sha256: "a".repeat(64),
            files: pkg_files(&[(
                "app.scm",
                r#"(import (bob util))
(patch-library! '(bob util) "(define-library (bob util) (export f) (import (techne)) (begin (define (f) \"hijacked\")))")
(provide fetch)
(define (fetch req env ctx) (dict 'status 200 'body (f)))"#,
            )]),
            declared_deps: vec![bob],
        },
    );
    let bundle = Arc::new(BundleVfs::new(BTreeMap::new(), Some(alice), packages).unwrap());
    let err = Deployment::load_bundle(bundle, "app.scm", Limits::default()).err().expect("bundle init must refuse patch-library!");
    assert!(err.msg.contains("patching") || err.msg.contains("not granted") || err.msg.contains("capability"), "{err}");
}

struct PatchOnceAttached {
    library: Vec<String>,
    code: String,
    done: bool,
}

impl Attached for PatchOnceAttached {
    fn poll(&mut self, vm: &mut Vm) -> bool {
        if !self.done {
            self.done = true;
            vm.patch_library(&self.library, &self.code, true).unwrap();
            return true;
        }
        false
    }
    fn reap(&mut self, _vm: &mut Vm) {}
}

#[test]
fn host_dirty_state_noop_nrepl_reset_and_stale_generation_release() {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async {
        let bundle = make_bundle();
        let lib = vec!["bob".to_string(), "util".to_string()];
        let bob_v1 = r#"(define-library (bob util)
  (export next-label)
  (import (techne))
  (begin
    (define counter (vector 0))
    (define (next-label)
      (vector-set! counter 0 (+ (vector-ref counter 0) 1))
      (string-append "v1#" (number->string (vector-ref counter 0))))))"#;

        let dep1 = Deployment::load_bundle(bundle.clone(), "app.scm", Limits::default()).unwrap();
        let host = Host::new();
        host.publish_dev("app", dep1, |_| Ok(Box::new(NoopAttached))).unwrap();
        let driver = host.clone();
        let handle = tokio::task::spawn_local(async move { driver.run(None).await });

        // 1. No-op patch MUST leave dirty == false and patch_deltas empty.
        let noop = host.patch_library("app", &lib, &format!("; noop comment\n{bob_v1}"), true).unwrap();
        assert!(noop.patched.is_empty());
        assert!(!host.is_dirty("app"));
        assert!(host.patch_deltas("app").is_empty());

        // 2. Real patch marks dirty == true; Host::reset_dev_world resets cleanly to v1!
        let r1 = host.fetch("app", request("/"), None).await.unwrap();
        assert_eq!(body_of(&r1.head), "v1#1");
        let report = host.patch_library("app", &lib, BOB_V2, true).unwrap();
        assert_eq!(report.patched, vec!["next-label"]);
        assert!(host.is_dirty("app"));
        let r2 = host.fetch("app", request("/"), None).await.unwrap();
        assert_eq!(body_of(&r2.head), "v2#2");

        host.reset_dev_world("app", |_| Ok(Box::new(NoopAttached))).unwrap();
        assert!(!host.is_dirty("app"));
        assert!(host.patch_deltas("app").is_empty());
        let r3 = host.fetch("app", request("/"), None).await.unwrap();
        assert_eq!(body_of(&r3.head), "v1#1");

        // 3. Patch applied inside an Attached driver (like nREPL `techne-patch-library`)
        //    is immediately reflected in Host::is_dirty and Host::patch_deltas!
        let dep2 = Deployment::load_bundle(bundle.clone(), "app.scm", Limits::default()).unwrap();
        let lib_for_attach = lib.clone();
        host.publish_dev("app", dep2, move |_| {
            Ok(Box::new(PatchOnceAttached { library: lib_for_attach.clone(), code: BOB_V2.to_string(), done: false }))
        })
        .unwrap();
        // Drive one turn so Attached::poll executes `vm.patch_library`.
        let r4 = host.fetch("app", request("/"), None).await.unwrap();
        assert_eq!(body_of(&r4.head), "v2#1");
        assert!(host.is_dirty("app"), "nREPL/Attached patch must mark Host::is_dirty");
        assert!(host.patch_deltas("app").contains_key(&lib), "nREPL/Attached patch must populate Host::patch_deltas");

        // 4. Stale-generation request finishing (`Host::release`) MUST NOT wipe a newer dev generation's dirty state!
        let slow_dir = tempfile::tempdir().unwrap();
        let slow_entry = slow_dir.path().join("slow.scm");
        std::fs::write(
            &slow_entry,
            r#"(provide fetch)
               (define (fetch req env ctx)
                 (sleep 80)
                 (dict 'status 200 'body "slow-gen1"))"#,
        )
        .unwrap();
        let slow_dep = Deployment::load(&slow_entry, Limits::default()).unwrap();
        host.publish("slow", slow_dep);
        let slow_req = host.fetch("slow", request("/"), None);
        tokio::time::sleep(Duration::from_millis(15)).await;

        // Publish a dev generation on "slow" and patch it while gen1's request is still in flight.
        let dev_dep = Deployment::load_bundle(bundle, "app.scm", Limits::default()).unwrap();
        host.publish_dev("slow", dev_dep, |_| Ok(Box::new(NoopAttached))).unwrap();
        host.patch_library("slow", &lib, BOB_V2, true).unwrap();
        assert!(host.is_dirty("slow"));

        // Wait for gen1's slow request to finish and call `release`.
        let slow_resp = slow_req.await.unwrap();
        assert_eq!(body_of(&slow_resp.head), "slow-gen1");
        assert!(host.is_dirty("slow"), "finishing stale generation must not clear newer generation's dirty flag");
        assert!(host.patch_deltas("slow").contains_key(&lib));

        handle.abort();
    });
}

#[test]
fn supervisor_rejects_over_32_part_library_without_killing_live_worker() {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async {
        let dir = tempfile::tempdir().unwrap();
        write_file(dir.path(), "techne-package.scm", r#"(package (alice app) (version "0.1.0") (cloud "app.scm"))"#);
        write_file(
            dir.path(),
            "lib/alice/app/util.sld",
            r#"(define-library (alice app util) (export tag) (import (techne)) (begin (define (tag) "alive")))"#,
        );
        write_file(
            dir.path(),
            "app.scm",
            r#"(import (alice app util))
(provide fetch)
(define (fetch req env ctx) (dict 'status 200 'body (tag)))"#,
        );

        let sup = Supervisor::start(worker_options(false)).await.unwrap();
        let p = sup.prepare_directory("app", dir.path()).await.unwrap();
        sup.publish("app", &p.id, None).unwrap();

        let r1 = sup.fetch("app", request("/"), None).await.unwrap();
        assert_eq!(body_of(&r1.head), "alive");

        // Over-32-part library name is rejected at supervisor validation with syntax-error without killing worker.
        let over_parts = vec!["p".to_string(); techne_vm::patch::MAX_LIBRARY_PARTS + 1];
        let err = sup.patch_library("app", over_parts, "(define-library (p))".into(), true).await.unwrap_err();
        assert_eq!(err.reason, "syntax-error");

        // Live worker continues serving requests uninterrupted.
        let r2 = sup.fetch("app", request("/"), None).await.unwrap();
        assert_eq!(body_of(&r2.head), "alive");
    });
}
