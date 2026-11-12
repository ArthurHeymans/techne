//! A fleet: tenants each served by a worker of their own, started when
//! needed and stopped when idle, their KV namespaces kept apart.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use techne_host::{
    fleet::{Fleet, FleetOptions, name_ok},
    supervisor::{Bindings, Options, ResourceConfinement, Sandboxing},
};
use techne_vm::data::Data;

/// What keeps under a key, or reads it, in the namespace bound as
/// `store`, and says who it is.
const APP: &str = r#"
(provide fetch)
(define (fetch request env ctx)
  (let ((path (dict-ref request 'path))
        (store (dict-ref env 'store)))
    (cond ((string=? path "/put") (kv-put! store "k" (dict-ref env 'who)) "kept")
          ((string=? path "/get") (kv-get store "k" "nothing"))
          ((string=? path "/slow") (sleep 400) (dict-ref env 'who))
          (else (dict-ref env 'who)))))
"#;

fn options(live: usize, idle: Duration) -> FleetOptions {
    FleetOptions {
        worker: Options {
            program: env!("CARGO_BIN_EXE_techne-worker").into(),
            args: Vec::new(),
            env: Vec::new(),
            sandboxing: Sandboxing::IfAvailable,
            resources: ResourceConfinement::IfAvailable,
            cgroup: techne_sandbox::cgroup::Limits { memory: 512 << 20, pids: 256, cpu: None },
            wall: Duration::from_secs(10),
            dev: false,
            egress: Default::default(),
        },
        live,
        idle,
    }
}

fn request(path: &str) -> Data {
    Data::Dict(vec![(Data::Symbol("method".into()), Data::String("GET".into())), (Data::Symbol("path".into()), Data::String(path.into()))])
}

/// The status and body of the response to `path` asked of `target`.
async fn ask(fleet: &Fleet, target: &str, path: &str) -> (i64, String) {
    let head = fleet.fetch(target, request(path), None, false).await.unwrap().head;
    let entries = match head {
        Data::Dict(entries) => entries,
        Data::String(body) => return (200, body),
        other => panic!("{other:?}"),
    };
    let get = |k: &str| entries.iter().find(|(key, _)| matches!(key, Data::Symbol(s) if s == k)).map(|(_, v)| v.clone());
    let status = match get("status") {
        Some(Data::Int(s)) => s,
        _ => 200,
    };
    match get("body") {
        Some(Data::String(b)) => (status, b),
        other => (status, format!("{other:?}")),
    }
}

/// What a deployment of a tenant is bound to: who it is, and the
/// namespace `shared`, which every tenant here calls the same.
fn bindings(who: &str) -> Bindings {
    let sym = |s: &str| Data::Symbol(s.into());
    Bindings {
        env: Data::Dict(vec![
            (sym("who"), Data::String(who.into())),
            (sym("store"), Data::Dict(vec![(sym("kind"), sym("kv")), (sym("name"), Data::String("mine".into()))])),
        ]),
        stores: [("mine".to_string(), "shared".to_string())].into(),
    }
}

/// Run `test` with a fleet of `options`, its store kept under a
/// directory of its own, and an entry module of APP.
fn with_fleet(options: FleetOptions, test: impl AsyncFnOnce(Fleet, PathBuf, PathBuf)) {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("app.scm");
    std::fs::write(&entry, APP).unwrap();
    let kept = dir.path().join("kept");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async {
        let fleet = Fleet::new(options).unwrap();
        let mut front = techne_vm::vm::Vm::new();
        fleet.install_store(&mut front);
        let kv = techne_vm::library::lisp_dir().join("cloud/kv.scm").display().to_string();
        let under = kept.display().to_string();
        std::fs::create_dir(&kept).unwrap();
        front.eval_source(&format!("(require {kv:?}) (spawn (lambda () (kv-serve-tenants {under:?})))")).unwrap();
        let driver = fleet.clone();
        tokio::task::spawn_local(async move {
            let _ = driver.run(&mut front).await;
        });
        test(fleet, entry, kept).await;
    });
}

async fn deploy(fleet: &Fleet, tenant: &str, entry: &Path) {
    fleet.tenant(tenant).unwrap().deploy_with("app", entry, bindings(tenant)).await.unwrap();
}

/// Until `fleet`'s live tenants are `want`.
async fn live(fleet: &Fleet, want: &[&str]) {
    let start = Instant::now();
    while fleet.live() != want {
        assert!(start.elapsed() < Duration::from_secs(5), "live: {:?}, not {want:?}", fleet.live());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Each tenant has a worker of its own, and namespaces of its own,
/// whatever it calls them; a tenant the fleet does not have, or a
/// project a tenant has not, is not found, and starts no worker.
#[test]
fn tenants_are_served_apart() {
    with_fleet(options(8, Duration::from_secs(60)), async |fleet, entry, kept| {
        assert!(fleet.live().is_empty());
        deploy(&fleet, "acme", &entry).await;
        assert_eq!(fleet.live(), ["acme"]);
        deploy(&fleet, "bolt", &entry).await;
        assert_eq!(fleet.live(), ["acme", "bolt"]);
        assert_eq!(ask(&fleet, "acme/app", "/").await, (200, "acme".into()));
        assert_eq!(ask(&fleet, "bolt/app", "/").await, (200, "bolt".into()));
        // The same namespace's name, a namespace each.
        assert_eq!(ask(&fleet, "acme/app", "/put").await.1, "kept");
        assert_eq!(ask(&fleet, "bolt/app", "/get").await.1, "nothing");
        assert_eq!(ask(&fleet, "bolt/app", "/put").await.1, "kept");
        assert_eq!(ask(&fleet, "acme/app", "/get").await.1, "acme");
        assert!(kept.join("acme/shared.kv").is_file() && kept.join("bolt/shared.kv").is_file());
        // Not found, and no worker for it.
        for target in ["none/app", "acme/none", "acme", "", "acme/app/x", "../acme/app"] {
            assert_eq!(ask(&fleet, target, "/").await.0, 404, "{target}");
        }
        assert_eq!(fleet.live(), ["acme", "bolt"]);
        assert!(fleet.known("none").is_none());
        for bad in ["", "Acme", "a/b", "a.b", "-a", "a-", "..", &"a".repeat(41)] {
            assert!(!name_ok(bad) && fleet.tenant(bad).is_err(), "{bad}");
        }
    });
}

/// A tenant idle for a while has no worker; its next request starts
/// one, which serves what it had published, with what it had kept.
#[test]
fn an_idle_tenant_sleeps_and_is_woken() {
    with_fleet(options(8, Duration::from_millis(300)), async |fleet, entry, _| {
        deploy(&fleet, "acme", &entry).await;
        assert_eq!(ask(&fleet, "acme/app", "/put").await.1, "kept");
        live(&fleet, &[]).await;
        assert!(fleet.known("acme").unwrap().sleeping());
        // Woken by a request, several at once by one worker.
        let (a, b) = tokio::join!(ask(&fleet, "acme/app", "/get"), ask(&fleet, "acme/app", "/"));
        assert_eq!((a, b), ((200, "acme".into()), (200, "acme".into())));
        assert_eq!(fleet.live(), ["acme"]);
        // Not while it serves a request, however long that is idle.
        assert_eq!(ask(&fleet, "acme/app", "/slow").await, (200, "acme".into()));
        live(&fleet, &[]).await;
        // Woken by a deployment too, the one published before still so.
        let acme = fleet.tenant("acme").unwrap();
        acme.prepare("app", &entry, bindings("other")).await.unwrap();
        assert_eq!(fleet.live(), ["acme"]);
        assert_eq!(ask(&fleet, "acme/app", "/").await, (200, "acme".into()));
    });
}

/// No more workers at once than the fleet may have: the tenant used
/// least recently sleeps for another to be served.
#[test]
fn workers_at_once_are_bounded() {
    with_fleet(options(1, Duration::from_secs(60)), async |fleet, entry, _| {
        deploy(&fleet, "acme", &entry).await;
        deploy(&fleet, "bolt", &entry).await;
        live(&fleet, &["bolt"]).await;
        assert_eq!(ask(&fleet, "acme/app", "/").await, (200, "acme".into()));
        live(&fleet, &["acme"]).await;
        assert_eq!(ask(&fleet, "bolt/app", "/").await, (200, "bolt".into()));
        live(&fleet, &["bolt"]).await;
        assert_eq!(ask(&fleet, "acme/app", "/").await, (200, "acme".into()));
        // While the one worker serves a request, none is started for
        // another tenant: refused, and served once there is room.
        let (slow, refused) = tokio::join!(ask(&fleet, "acme/app", "/slow"), async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let refused = ask(&fleet, "bolt/app", "/").await;
            (refused, fleet.live())
        });
        assert_eq!(slow, (200, "acme".into()));
        assert_eq!(refused, ((503, "Service Unavailable".into()), vec!["acme".to_string()]));
        assert_eq!(ask(&fleet, "bolt/app", "/").await, (200, "bolt".into()));
        assert_eq!(fleet.live(), ["bolt"]);
    });
}

/// A request for a tenant asleep that is given up before its worker has
/// started is not served; a tenant whose worker cannot be started
/// sleeps again, and is woken by the next request once one can.
#[test]
fn a_wake_is_the_supervisors_own() {
    let mut broken = options(8, Duration::from_millis(200));
    let program = std::mem::replace(&mut broken.worker.program, "/nonexistent/worker".into());
    with_fleet(broken, async |fleet, entry, _| {
        // No worker can be started: refused, and asleep still.
        let acme = fleet.tenant("acme").unwrap();
        assert!(acme.deploy_with("app", &entry, bindings("acme")).await.is_err());
        assert!(acme.sleeping() && fleet.live().is_empty());
        assert!(acme.prepare("app", &entry, bindings("acme")).await.is_err());
        assert!(acme.sleeping());
    });
    with_fleet(options(8, Duration::from_millis(200)), async |fleet, entry, _| {
        let _ = &program;
        deploy(&fleet, "acme", &entry).await;
        live(&fleet, &[]).await;
        // Given up at once: dropped, the wake goes on, and the next is
        // served.
        drop(fleet.fetch("acme/app", request("/"), None, false));
        assert_eq!(ask(&fleet, "acme/app", "/").await, (200, "acme".into()));
    });
}
