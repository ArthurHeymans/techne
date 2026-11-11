//! A fleet: tenants, each served by a supervisor and worker process of
//! its own ([`crate::supervisor`]), never shared with another tenant
//! (doc/serverless.md, "Workers"). A tenant's projects share its worker.
//!
//! A tenant without traffic has no worker: its supervisor sleeps
//! ([`Supervisor::sleep`]) once it was idle for a while, and its worker
//! is started again by its next request or deployment, with what it had
//! published. The number of workers at once is bounded: past it, the
//! tenant used least recently that can sleep does, and where none can
//! (every worker serves something) no other is started: what needed it
//! is refused, with 503 for a request.
//!
//! Requests are asked by target, `tenant/project` ([`Fleet::fetch`],
//! and `host-fetch` in a front world, [`Fleet::install`]): which target
//! a request is for (its host name, say) is its front's to decide, in
//! Lisp (`lisp/cloud/hosts.scm`). A tenant the fleet does not have, or
//! a project its tenant has not published, is answered 404, and starts
//! no worker.
//!
//! What tenants keep in their KV namespaces is kept by one store, a task
//! of the front world ([`Fleet::install_store`]), told with each
//! operation whose it is: the tenant is the supervisor's word, not the
//! worker's.

use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use techne_sandbox::cgroup::Cgroups;
use techne_vm::{data::Data, tasks::Progress, vm::Vm};
use tokio::sync::{Notify, oneshot};

use crate::{
    Response,
    ask::Asker,
    body::{self, Reader},
    supervisor::{Options, Supervisor, adopt},
};

/// How a fleet runs its tenants.
#[derive(Clone, Debug)]
pub struct FleetOptions {
    /// How each tenant's supervisor starts its workers.
    pub worker: Options,
    /// Workers at once, at most (at least one).
    pub live: usize,
    /// How long a tenant may be idle before its worker is stopped.
    pub idle: Duration,
}

struct Tenant {
    supervisor: Supervisor,
    /// When it was last asked for something.
    used: Instant,
}

struct State {
    tenants: HashMap<String, Tenant>,
    store: Option<Asker>,
}

/// Tenants and their supervisors: see the module.
#[derive(Clone)]
pub struct Fleet {
    options: Rc<FleetOptions>,
    cgroups: Rc<Option<Cgroups>>,
    state: Rc<RefCell<State>>,
    /// Wakes the front world's driver.
    wake: Arc<Notify>,
}

/// Whether `name` may name a tenant or a project: 1 to 40 lowercase
/// letters, digits and `-`, not at either end. It is part of a host
/// name, of a cgroup's name and of a directory's.
pub fn name_ok(name: &str) -> bool {
    let bytes = name.as_bytes();
    (1..=40).contains(&bytes.len())
        && bytes.iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
        && bytes[0] != b'-'
        && bytes[bytes.len() - 1] != b'-'
}

impl Fleet {
    /// A fleet with no tenants. Call it on a current-thread runtime
    /// within a `LocalSet`: it starts what puts idle tenants to sleep.
    pub fn new(options: FleetOptions) -> std::io::Result<Fleet> {
        let cgroups = Rc::new(adopt(&options.worker)?);
        let fleet = Fleet {
            options: Rc::new(options),
            cgroups,
            state: Rc::new(RefCell::new(State { tenants: HashMap::new(), store: None })),
            wake: Arc::new(Notify::new()),
        };
        let resting = fleet.clone();
        tokio::task::spawn_local(async move {
            // Often enough that workers past the bound do not last.
            let every = (resting.options.idle / 4).clamp(Duration::from_millis(20), Duration::from_secs(1));
            loop {
                tokio::time::sleep(every).await;
                resting.rest();
            }
        });
        Ok(fleet)
    }

    /// The supervisor of tenant `name`, added if the fleet does not have
    /// it: asleep, until it is given something to do.
    pub fn tenant(&self, name: &str) -> Result<Supervisor, String> {
        if !name_ok(name) {
            return Err(format!("{name}: not a tenant's name"));
        }
        let mut s = self.state.borrow_mut();
        if let Some(t) = s.tenants.get_mut(name) {
            t.used = Instant::now();
            return Ok(t.supervisor.clone());
        }
        // A cgroup's name of its own among the tenants': `-` is in no
        // tenant's name twice in a row at its end.
        let supervisor =
            Supervisor::asleep(self.options.worker.clone(), self.cgroups.clone(), &format!("{name}--")).map_err(|e| e.to_string())?;
        if let Some(store) = &s.store {
            supervisor.set_store(store.clone(), name);
        }
        // A worker is started for it only where there is room for one.
        let fleet = self.clone();
        supervisor.set_gate(move || fleet.room());
        s.tenants.insert(name.into(), Tenant { supervisor: supervisor.clone(), used: Instant::now() });
        Ok(supervisor)
    }

    /// The supervisor of tenant `name`, if the fleet has it.
    pub fn known(&self, name: &str) -> Option<Supervisor> {
        self.state.borrow().tenants.get(name).map(|t| t.supervisor.clone())
    }

    /// The tenants whose workers run now, in order.
    pub fn live(&self) -> Vec<String> {
        let mut live: Vec<String> =
            self.state.borrow().tenants.iter().filter(|(_, t)| !t.supervisor.sleeping()).map(|(n, _)| n.clone()).collect();
        live.sort();
        live
    }

    /// Whether another worker may be started: fewer run than may at
    /// once, or the tenant used least recently that can sleep now does,
    /// for it. Where every worker is busy, there is no room.
    fn room(&self) -> bool {
        let mut awake: Vec<(Instant, Supervisor)> =
            self.state.borrow().tenants.values().filter(|t| !t.supervisor.sleeping()).map(|t| (t.used, t.supervisor.clone())).collect();
        if awake.len() < self.options.live.max(1) {
            return true;
        }
        awake.sort_by_key(|(used, _)| *used);
        awake.into_iter().any(|(_, supervisor)| supervisor.sleep())
    }

    /// Put to sleep the tenants idle for long enough, and, past the
    /// workers there may be at once, those used least recently: of those
    /// that can sleep (one serving a request does not).
    fn rest(&self) {
        let now = Instant::now();
        let mut awake: Vec<(Instant, Supervisor)> =
            self.state.borrow().tenants.values().filter(|t| !t.supervisor.sleeping()).map(|t| (t.used, t.supervisor.clone())).collect();
        awake.sort_by_key(|(used, _)| *used);
        let mut live = awake.len();
        for (used, supervisor) in awake {
            if (now.duration_since(used) >= self.options.idle || live > self.options.live.max(1)) && supervisor.sleep() {
                live -= 1;
            }
        }
    }

    /// Ask `target`, `tenant/project`, to answer `request`, as
    /// [`Supervisor::fetch_routed`] does; a target that is none, or of a
    /// tenant the fleet does not have, is answered 404.
    pub fn fetch(&self, target: &str, request: Data, body: Option<Reader>, debug: bool) -> oneshot::Receiver<Response> {
        let found = target.split_once('/').and_then(|(tenant, project)| {
            let mut s = self.state.borrow_mut();
            let t = s.tenants.get_mut(tenant)?;
            t.used = Instant::now();
            Some((t.supervisor.clone(), project.to_string()))
        });
        match found {
            Some((supervisor, project)) => supervisor.fetch_routed(&project, request, body, debug),
            None => {
                let (tx, rx) = oneshot::channel();
                let _ = tx.send(Response { head: crate::failed(404), body: None });
                rx
            }
        }
    }

    /// Define `(host-fetch target request #:debug #f)` in the front
    /// world, as [`crate::Host::install`] does, TARGET a string,
    /// `tenant/project`.
    pub fn install(&self, front: &mut Vm) {
        self.wakes(front);
        let me = self.clone();
        body::install_front(front, move |target, request, body, debug| me.fetch(target, request, body, debug));
    }

    /// Have a task of `front` keep what all tenants keep in their KV
    /// namespaces (`crate::ask`, as `store`): as
    /// [`Supervisor::install_store`], each operation asked a list of its
    /// tenant, its namespace and the operation
    /// (`kv-serve-tenants` in `lisp/cloud/kv.scm` keeps each tenant's
    /// apart).
    pub fn install_store(&self, front: &mut Vm) {
        self.wakes(front);
        let store = crate::ask::install(front, "store", 256);
        let mut s = self.state.borrow_mut();
        for (name, t) in &s.tenants {
            t.supervisor.set_store(store.clone(), name);
        }
        s.store = Some(store);
    }

    /// Have what `front`'s tasks wait on wake the fleet's driver
    /// ([`Fleet::run`]).
    pub fn wakes(&self, front: &mut Vm) {
        let wake = self.wake.clone();
        front.set_wake_notifier(move || wake.notify_one());
    }

    /// Drive `front` (installed) until its tasks have all finished.
    pub async fn run(&self, front: &mut Vm) -> Result<(), techne_vm::vm::Error> {
        loop {
            match front.run_tasks_for(Duration::from_millis(10))? {
                Progress::Finished => return Ok(()),
                Progress::OutOfTime => tokio::task::yield_now().await,
                Progress::Blocked => match front.next_timer() {
                    Some(t) => {
                        let _ = tokio::time::timeout_at(t.into(), self.wake.notified()).await;
                    }
                    None => self.wake.notified().await,
                },
            }
        }
    }
}
