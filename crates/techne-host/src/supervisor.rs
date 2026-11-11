//! A supervisor: deployments served by a worker process
//! ([`crate::worker`]), with sandbox entry and cgroup resource confinement
//! required independently (`Options::sandboxing`, `Options::resources`),
//! for a front world of the supervisor's (`host-fetch`, as
//! [`crate::Host::install`] has it).
//!
//! The supervisor trusts nothing the worker says (`wire`): a reply must be
//! to a request it sent, and anything else (a malformed frame, a reply to
//! a request never sent, a body past its credits) kills the worker; a
//! reply to one it no longer waits for (abandoned, cut short) is dropped.
//! It bounds what it waits for too: a request the worker has not answered
//! a little past its wall time kills the worker; a response's body still
//! streaming then, behind a client slow to read it, is cut short. A
//! request cancelled so is waited for until the worker says it is done
//! with it, which it does at once, or a worker that cannot is killed.
//! What is sent to a worker waits bounded (`QUEUED`): one that does not
//! read it is killed too. A worker that ends, killed or for
//! a native that held its thread past a kill (`worker::STUCK`) or past its
//! cgroup's memory, has its requests answered for, with 500, reported;
//! another is started, at most a few a minute, and given the
//! deployments published. Requests are not run again.
//!
//! A deployment is what it is made of: its id is a hash of its name,
//! entry, files and bindings, and the worker serves it under its name and
//! id, beside the others of its name. Which of them a name's requests go
//! to is the supervisor's route for the name. So deploying is two steps:
//! [`Supervisor::prepare`] has the worker load a deployment, which
//! changes nothing of what is served, and fails alone;
//! [`Supervisor::publish`] points the name at one prepared, at once, and
//! only if it still points where the publisher saw it, when it says
//! where (a late or repeated publish does not undo a newer one). A
//! request is pinned when it is sent to the deployment its name was
//! routed to then: it is served by that code, with those bindings,
//! whatever is published after. A deployment no longer routed to is
//! retired from the worker once its requests are done. Rolling back is
//! publishing an earlier one, prepared again if it was retired.
//!
//! What a worker's requests fetch is fetched here ([`crate::egress`]),
//! within the policy given (`Options::egress`): for a request the
//! supervisor sent it and still serves, a bounded number of times each
//! (`FETCHES`), a bounded number at once (`FETCHING`); the worker's own
//! counts are not trusted.
//!
//! What a worker's requests keep in their deployment's KV namespaces is
//! kept by the supervisor too, for a store it is given
//! ([`Supervisor::install_store`]: a task of the front world's, in Lisp).
//! A deployment is deployed with its bindings ([`Bindings`]): what its
//! requests are given, and which namespace each of its KV bindings stands
//! for. A worker names a binding as its deployment does, never a
//! namespace: the supervisor looks it up among those of the deployment
//! the request it sent was for, and the store is asked for that
//! namespace. So a worker reaches the namespaces of the deployments it
//! serves requests of, and no other, whatever it says.
//!
//! For development (`Options::dev`), the worker serves each deployment
//! from its dev world, with a REPL server in it ([`crate::worker::Dev`]),
//! whose clients connect to the supervisor ([`Supervisor::serve_repl`]),
//! which has the network: each connection is relayed as bytes both ways.
//! A worker replaced takes its REPL connections with it: they are closed,
//! and clients connect again to the next.

use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    ffi::OsString,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::process::ExitStatusExt,
    },
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use techne_sandbox::cgroup::{self, Cgroups};
use techne_vm::{data::Data, tasks::Progress, vm::Vm};
use tokio::{
    io::AsyncWriteExt,
    net::UnixStream,
    sync::{Notify, Semaphore, mpsc, oneshot},
};

use crate::{
    Failure, Printed, Response,
    ask::Asker,
    body::{self, DataValue, Piece, Reader},
    outbound::Service,
    wire::{BUNDLE_BYTES, FromWorker, MAX_FRAME, REPL_CHUNK, ToWorker, WINDOW, encode, read_frame, write_encoded},
    worker,
};

/// How long past a request's wall time the supervisor waits for the worker
/// to answer it (the worker answers 504 at its wall time).
const GRACE: Duration = Duration::from_secs(2);

/// Workers started a minute, at most: past them, requests are refused.
const RESTARTS: usize = 5;

/// How long a worker may take to be ready.
const START: Duration = Duration::from_secs(10);

/// Whether workers are sandboxed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sandboxing {
    /// Always; a worker that cannot be is an error.
    Required,
    /// Where the kernel allows it; elsewhere (a dev machine without user
    /// namespaces), not, said once.
    IfAvailable,
    Off,
}

/// Whether workers must have their configured cgroup resource bounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceConfinement {
    /// Refuse to start without cgroups; never run a worker unbounded.
    Required,
    /// For development: run workers unbounded, with a diagnostic, when
    /// cgroups cannot be adopted or a worker's cgroup cannot have every
    /// configured bound.
    IfAvailable,
}

/// The confinement of the worker serving now, not just what was requested.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Confinement {
    /// The worker entered its OS sandbox before becoming ready.
    pub sandboxed: bool,
    /// The worker joined a cgroup with every configured bound applied.
    pub cgroup_bounded: bool,
}

/// How the supervisor starts its workers.
#[derive(Clone, Debug)]
pub struct Options {
    /// The worker's program, and its arguments before `--fd N`.
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// Its environment, beside the runtime's settings (`TECHNE_*`) the
    /// supervisor has.
    pub env: Vec<(OsString, OsString)>,
    pub sandboxing: Sandboxing,
    /// Required independently of sandbox entry.
    pub resources: ResourceConfinement,
    /// What a worker's cgroup bounds, when resource confinement is available.
    pub cgroup: cgroup::Limits,
    /// A request's wall time, the deployments' own.
    pub wall: Duration,
    /// Whether deployments are served for development (their dev worlds,
    /// with a REPL).
    pub dev: bool,
    /// What requests may fetch.
    pub egress: crate::egress::Policy,
}

/// Bytes waiting to be written to a worker, at most: a deployment and
/// more. A worker that does not read them is ended.
const QUEUED: usize = 48 << 20;

/// What goes to a worker: each message encoded as it is sent, the bytes
/// waiting bounded (`QUEUED`).
#[derive(Clone)]
struct Link {
    tx: mpsc::UnboundedSender<Vec<u8>>,
    queued: Rc<std::cell::Cell<usize>>,
    /// Whether a message was refused for the bytes waiting.
    over: Rc<std::cell::Cell<bool>>,
}

impl Link {
    fn send(&self, m: ToWorker) -> Result<(), ()> {
        let bytes = encode(&m.to_data());
        if bytes.len() > MAX_FRAME || self.queued.get() + bytes.len() > QUEUED {
            self.over.set(true);
            return Err(());
        }
        self.queued.set(self.queued.get() + bytes.len());
        self.tx.send(bytes).map_err(|_| ())
    }
}

/// The most bytes of a fetch a worker asks for: a request body's and its
/// URL and headers.
const FETCH_BYTES: usize = (1 << 20) + (64 << 10);

/// Requests that may wait for a worker asleep to be started: more are
/// refused.
const DEFERRED: usize = 32;

/// Fetches of a request, and of a worker's requests at once (with what
/// they ask of the store).
const FETCHES: usize = 50;
const FETCHING: usize = 64;

/// Operations of a request on its bindings' storage.
const STORE_OPS: usize = 1000;

/// How long the store may take to answer.
const STORE: Duration = Duration::from_secs(10);

/// What a deployment is deployed with.
#[derive(Clone, Debug)]
pub struct Bindings {
    /// What its requests are given as `env`: a dict, by the names the
    /// deployment has for its bindings.
    pub env: Data,
    /// The KV namespace each of its KV bindings stands for, by the
    /// binding's name.
    pub stores: HashMap<String, String>,
}

impl Bindings {
    /// The bindings `data` describes, as `lisp/cloud/manifest.scm` reads
    /// a manifest: a dict of `env` and of `stores`, a dict of strings.
    pub fn from_data(data: &Data) -> Result<Bindings, String> {
        let Data::Dict(parts) = data else { return Err("bindings are a dict of env and stores".into()) };
        let part = |name: &str| parts.iter().find(|(k, _)| matches!(k, Data::Symbol(s) if s == name)).map(|(_, v)| v);
        let mut stores = HashMap::new();
        match part("stores") {
            Some(Data::Dict(entries)) => {
                for entry in entries {
                    let (Data::String(binding), Data::String(namespace)) = entry else {
                        return Err("stores are strings by strings".into());
                    };
                    stores.insert(binding.clone(), namespace.clone());
                }
            }
            None => {}
            Some(_) => return Err("stores are a dict".into()),
        }
        let env = match part("env") {
            Some(env @ Data::Dict(_)) => env.clone(),
            None => Data::Dict(Vec::new()),
            Some(_) => return Err("env is a dict".into()),
        };
        Ok(Bindings { env, stores })
    }

    fn to_data(&self) -> Data {
        let mut stores: Vec<_> = self.stores.iter().map(|(b, n)| (Data::String(b.clone()), Data::String(n.clone()))).collect();
        stores.sort_by(|a, b| format!("{:?}", a.0).cmp(&format!("{:?}", b.0)));
        Data::Dict(vec![(Data::Symbol("env".into()), self.env.clone()), (Data::Symbol("stores".into()), Data::Dict(stores))])
    }
}

impl Bundle {
    /// All of it, as bytes to keep: `Bundle::decode` reads them.
    fn encode(&self) -> Vec<u8> {
        encode(&Data::List(vec![
            Data::String(self.name.clone()),
            Data::String(self.entry.clone()),
            Data::List(self.files.iter().map(|(p, b)| Data::List(vec![Data::String(p.clone()), Data::Bytes(b.clone())])).collect()),
            self.bindings.to_data(),
        ]))
    }

    /// The name, entry, files and bindings `bytes` keep.
    #[allow(clippy::type_complexity)]
    fn decode(bytes: &[u8]) -> Result<(String, String, Vec<(String, Vec<u8>)>, Bindings), String> {
        let bad = || "not a deployment's bundle".to_string();
        let Ok(Data::List(parts)) = crate::wire::decode(bytes) else { return Err(bad()) };
        let [Data::String(name), Data::String(entry), Data::List(files), bindings] = &parts[..] else { return Err(bad()) };
        let files = files
            .iter()
            .map(|f| match f {
                Data::List(f) => match &f[..] {
                    [Data::String(path), Data::Bytes(bytes)] => Ok((path.clone(), bytes.clone())),
                    _ => Err(bad()),
                },
                _ => Err(bad()),
            })
            .collect::<Result<_, _>>()?;
        Ok((name.clone(), entry.clone(), files, Bindings::from_data(bindings)?))
    }
}

impl Default for Bindings {
    fn default() -> Bindings {
        Bindings { env: Data::Dict(Vec::new()), stores: HashMap::new() }
    }
}

/// What a worker asked of a KV binding (`outbound::Service::Kv`), as the
/// store is handed it: the binding named, the operation, and whether it
/// is a `get`. Only what an operation has is kept of it, and a value to
/// keep is encoded (`wire::encode`): the store's world, which lasts and
/// is not the tenant's, is given a tenant's strings and bytes and never
/// its symbols, which it would intern for good.
fn kv_operation(request: &Data) -> Result<(String, Data, bool), String> {
    let Data::Dict(entries) = request else { return Err("not an operation".into()) };
    let field = |name: &str| entries.iter().find_map(|(k, v)| matches!(k, Data::Symbol(s) if s == name).then_some(v));
    let string = |name: &str, default: Option<&str>| match (field(name), default) {
        (Some(Data::String(s)), _) => Ok(Data::String(s.clone())),
        (None, Some(default)) => Ok(Data::String(default.into())),
        _ => Err(format!("an operation's {name} is a string")),
    };
    let sym = |s: &str| Data::Symbol(s.into());
    let Some(Data::String(binding)) = field("binding") else { return Err("no binding named".into()) };
    let Some(Data::Symbol(op)) = field("op") else { return Err("no operation named".into()) };
    let fields = match op.as_str() {
        "get" | "delete" => vec![(sym("key"), string("key", None)?)],
        "put" => {
            let value = field("value").ok_or("nothing to keep")?;
            let ttl = match field("ttl") {
                Some(Data::Int(n)) => Data::Int(*n),
                Some(Data::Float(f)) => Data::Float(*f),
                Some(Data::Bool(false)) | None => Data::Bool(false),
                Some(_) => return Err("an operation's ttl is a number of seconds".into()),
            };
            vec![(sym("key"), string("key", None)?), (sym("value"), Data::Bytes(encode(value))), (sym("ttl"), ttl)]
        }
        "list" => {
            let limit = match field("limit") {
                Some(Data::Int(n)) => Data::Int(*n),
                None => Data::Int(1000),
                Some(_) => return Err("an operation's limit is an integer".into()),
            };
            vec![(sym("prefix"), string("prefix", Some(""))?), (sym("limit"), limit)]
        }
        _ => return Err("no such operation".into()),
    };
    let operation = std::iter::once((sym("op"), sym(op))).chain(fields).collect();
    Ok((binding.clone(), Data::Dict(operation), op == "get"))
}

/// The store's answer to a `get`, the value it kept decoded again.
fn kv_found(answer: Data) -> Result<Data, String> {
    let Data::Dict(entries) = answer else { return Ok(answer) };
    let decoded = entries.into_iter().map(|(k, v)| match (&k, v) {
        (Data::Symbol(s), Data::Bytes(bytes)) if s == "value" => {
            crate::wire::decode(&bytes).map(|v| (k, v)).map_err(|_| "what is kept there cannot be read".to_string())
        }
        (_, v) => Ok((k, v)),
    });
    decoded.collect::<Result<_, _>>().map(Data::Dict)
}

/// Bytes a relayed REPL connection may have waiting to be written to its
/// client.
const REPL_QUEUE: usize = 64;

/// A REPL connection relayed: what goes to its client, and the task
/// reading from it, which ends when the connection is dropped.
struct Repl {
    queue: mpsc::Sender<Vec<u8>>,
    reader: tokio::task::AbortHandle,
}

impl Drop for Repl {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

/// What a deployment's worker is given: its name and id, entry, and
/// files, and its bindings, of which the worker is given the `env`.
#[derive(Clone)]
struct Bundle {
    name: String,
    id: String,
    entry: String,
    files: Vec<(String, Vec<u8>)>,
    bindings: Bindings,
}

impl Bundle {
    /// The name the worker serves it under.
    fn served(&self) -> String {
        served(&self.name, &self.id)
    }
}

/// The name a worker serves deployment `id` of `name` under.
fn served(name: &str, id: &str) -> String {
    format!("{name}@{id}")
}

/// The id of a deployment: a hash of all it is.
fn deployment_id(name: &str, entry: &str, files: &[(String, Vec<u8>)], bindings: &Bindings) -> String {
    use sha2::Digest;
    let mut stores: Vec<_> = bindings.stores.iter().collect();
    stores.sort();
    let all = Data::List(vec![
        Data::String(name.into()),
        Data::String(entry.into()),
        Data::List(files.iter().map(|(p, b)| Data::List(vec![Data::String(p.clone()), Data::Bytes(b.clone())])).collect()),
        bindings.env.clone(),
        Data::List(stores.into_iter().map(|(b, n)| Data::List(vec![Data::String(b.clone()), Data::String(n.clone())])).collect()),
    ]);
    // The whole digest: an id is what tells two deployments apart.
    sha2::Sha256::digest(encode(&all)).iter().map(|b| format!("{b:02x}")).collect()
}

/// A deployment the worker has loaded.
struct Ready {
    bundle: Bundle,
    /// Whether its name was ever routed to it: one that was and is no
    /// longer is retired once its requests are done.
    published: bool,
    since: Instant,
    /// Whether the worker there is now has it: not one given it again
    /// after the worker before ended, until it says so.
    loaded: bool,
}

/// How long a deployment prepared and never published is kept, and how
/// many of a name's are.
const UNPUBLISHED: Duration = Duration::from_secs(300);
const UNPUBLISHED_EACH: usize = 4;

/// What preparing a deployment gave ([`Supervisor::prepare`]).
#[derive(Clone, Debug)]
pub struct Prepared {
    /// The deployment's id, to publish it by.
    pub id: String,
    /// What loading it wrote; nothing for one that was loaded already.
    pub output: String,
}

/// A request the worker serves: who waits for its response, until it is
/// given; the queue its response's body arrives on, while that streams;
/// the credits for its own body's chunks, while that streams.
/// A request cancelled (abandoned, its body cut short) waits for the
/// worker to say it is done with it, as it does at once, within its
/// deadline.
struct Pending {
    name: String,
    /// The deployment it was sent for, as the worker serves it.
    served: String,
    reply: Option<oneshot::Sender<Response>>,
    deadline: Instant,
    body: Option<mpsc::Sender<Piece>>,
    credits: Option<Rc<Semaphore>>,
    /// What sends its body.
    pump: Option<tokio::task::AbortHandle>,
    cancelled: bool,
    /// Its fetches so far, and its operations on storage.
    fetches: usize,
    store_ops: usize,
    /// The KV namespaces of the deployment it was sent for, as they were
    /// then: a deployment since does not change what it reaches.
    stores: Rc<HashMap<String, String>>,
}

impl Pending {
    /// Cancel it: nothing more of it is sent or received.
    fn cancel(&mut self) {
        self.cancelled = true;
        self.reply = None;
        self.body = None;
        if let Some(p) = self.pump.take() {
            p.abort();
        }
    }
}

impl Drop for Pending {
    /// No more of the request's body is sent.
    fn drop(&mut self) {
        if let Some(p) = &self.pump {
            p.abort();
        }
    }
}

/// The worker running now.
struct Running {
    generation: u64,
    send: Link,
    /// The worker's pidfd: a kill through it cannot reach another process
    /// given the worker's pid once its waiter reaped it.
    pidfd: OwnedFd,
    cgroup: Option<cgroup::Worker>,
}

struct State {
    /// No worker until one is next needed (`Supervisor::sleep`), and one
    /// being started for that.
    asleep: bool,
    waking: bool,
    /// Requests waiting for a worker to be started for them.
    deferred: usize,
    /// Whether a worker may be started for a supervisor asleep, if
    /// someone bounds them.
    gate: Option<Rc<dyn Fn() -> bool>>,
    /// What the store is told the namespaces asked for are those of, if
    /// it is one that supervisors share.
    store_scope: Option<Rc<str>>,
    worker: Option<Running>,
    generation: u64,
    next_rid: u64,
    pending: HashMap<u64, Pending>,
    next_deploy: u64,
    deploys: HashMap<u64, (Bundle, oneshot::Sender<Result<String, String>>)>,
    /// The deployments the worker has loaded, by the names it serves
    /// them under.
    ready: HashMap<String, Ready>,
    /// Who else waits for a deployment being loaded, by the name it is
    /// served under.
    joining: HashMap<String, Vec<oneshot::Sender<Result<String, String>>>>,
    /// The id of the deployment each name's requests go to.
    routes: HashMap<String, String>,
    /// The KV namespaces of the deployments loaded, by the names they are
    /// served under, and binding.
    stores: HashMap<String, Rc<HashMap<String, String>>>,
    /// What keeps them, once installed: as many asks wait there as may
    /// be made at once.
    store: Option<Asker>,
    sandbox: bool,
    started: VecDeque<Instant>,
    /// REPL connections relayed to the worker now, by number: what goes
    /// to each client.
    repls: HashMap<u64, Repl>,
    next_cid: u64,
    /// Fetches being made.
    fetching: usize,
    on_failure: Box<dyn FnMut(&Failure)>,
    on_output: Box<dyn FnMut(&Printed)>,
}

impl State {
    /// Keep a few of `name`'s deployments prepared and not published at
    /// most, `kept`, just loaded, among them: the oldest others go. Done
    /// once a new one has loaded, so that one that does not load costs
    /// none of those there.
    fn evict_unpublished(&mut self, name: &str, kept: &str) {
        loop {
            let mut waiting: Vec<(Instant, String)> = self
                .ready
                .iter()
                .filter(|(_, r)| r.bundle.name == name && !r.published)
                .map(|(served, r)| (r.since, served.clone()))
                .collect();
            if waiting.len() <= UNPUBLISHED_EACH {
                return;
            }
            waiting.sort();
            let Some((_, oldest)) = waiting.into_iter().find(|(_, served)| served != kept) else { return };
            self.ready.remove(&oldest);
            self.stores.remove(&oldest);
            if let Some(w) = &self.worker {
                let _ = w.send.send(ToWorker::Retire { name: oldest });
            }
        }
    }

    /// The store, the namespace the binding `request` names stood for in
    /// the deployment request `rid` was sent for, when it was sent, the
    /// operation as the store takes it and whether it is a `get`: or why
    /// not.
    #[allow(clippy::type_complexity)]
    fn namespace(&self, rid: u64, request: &Data) -> Result<(Asker, String, Data, bool), String> {
        let store = self.store.clone().ok_or("this host keeps nothing")?;
        let (binding, operation, get) = kv_operation(request)?;
        let stores = self.pending.get(&rid).map(|p| &p.stores);
        let namespace = stores.and_then(|s| s.get(&binding)).ok_or_else(|| format!("the deployment has no KV binding {binding}"))?;
        Ok((store, namespace.clone(), operation, get))
    }
}

/// The most bytes of a manifest, and of the secrets beside it.
const MANIFEST_BYTES: u64 = 256 << 10;

/// The bindings of the deployment in `dir`, as its manifest and secrets
/// say (`lisp/cloud/manifest.scm`): read in a world of their own, made
/// for it and dropped after, of bounded memory. A manifest is its
/// tenant's to write: what reading it interns and allocates goes with
/// that world, and nothing of it but the bindings, as data, comes out.
pub fn read_bindings(dir: &Path) -> Result<Bindings, String> {
    for file in ["manifest.scm", ".secrets.scm"] {
        if std::fs::metadata(dir.join(file)).is_ok_and(|m| m.len() > MANIFEST_BYTES) {
            return Err(format!("{file} is larger than a manifest may be"));
        }
    }
    let failed = |e: techne_vm::vm::Error| format!("the manifest: {}", e.msg);
    let mut vm = Vm::new();
    vm.set_memory_limit(64 << 20);
    let manifest = techne_vm::library::lisp_dir().join("cloud/manifest.scm").display().to_string();
    vm.eval_source(&format!("(require {manifest:?})")).map_err(failed)?;
    let read = vm.eval_source("read-bindings").map_err(failed)?;
    let dir = vm.from_data(&Data::String(dir.display().to_string())).map_err(failed)?;
    let found = vm.call(&read, &[dir]).map_err(failed)?;
    Bindings::from_data(&vm.to_data(&found, 4 << 20).map_err(failed)?)
}

/// Which supervisor a name given to the natives that deploy is of, and
/// its deployment's name there: or why none.
pub(crate) type Resolve = Rc<dyn Fn(&str) -> Result<(Supervisor, String), String>>;

/// Define in `front` the natives [`Supervisor::install_control`] says,
/// each NAME resolved by `resolve` to a supervisor and a name there (a
/// fleet's are `tenant/project`).
pub(crate) fn install_control_with(front: &mut Vm, resolve: Resolve) {
    use techne_vm::{
        api::{Bytes, Value},
        vm::Error,
    };
    let listed = |p: Prepared| DataValue(Data::List(vec![Data::String(p.id), Data::String(p.output)]));
    let of = resolve.clone();
    front.register_async("%cloud-prepare-directory", 2, move |vm, args| {
        let asked = (|| {
            let (name, entry) = (args.get::<String>(vm, 0)?, args.get::<String>(vm, 1)?);
            let (me, name) = of(&name).map_err(Error::new)?;
            Ok::<_, Error>((me, name, entry))
        })();
        async move {
            let (me, name, entry) = asked?;
            let entry = Path::new(&entry);
            let bindings = read_bindings(entry.parent().unwrap_or(Path::new("/"))).map_err(Error::new)?;
            me.prepare(&name, entry, bindings).await.map(listed).map_err(Error::new)
        }
    });
    let of = resolve.clone();
    front.register_async("%cloud-prepare", 3, move |vm, args| {
        let asked = (|| {
            let (name, entry) = (args.get::<String>(vm, 0)?, args.get::<String>(vm, 1)?);
            let bindings = args.value(vm, 2)?;
            let bindings = Bindings::from_data(&vm.to_data(&bindings, crate::REQUEST_BYTES)?).map_err(Error::new)?;
            let (me, name) = of(&name).map_err(Error::new)?;
            Ok::<_, Error>((me, name, entry, bindings))
        })();
        async move {
            let (me, name, entry, bindings) = asked?;
            me.prepare(&name, Path::new(&entry), bindings).await.map(listed).map_err(Error::new)
        }
    });
    let of = resolve.clone();
    front.register_async("%cloud-prepare-bundle", 2, move |vm, args| {
        let asked = (|| {
            let (name, bytes) = (args.get::<String>(vm, 0)?, args.get::<Bytes>(vm, 1)?.0);
            let (me, name) = of(&name).map_err(Error::new)?;
            Ok::<_, Error>((me, name, bytes))
        })();
        async move {
            let (me, name, bytes) = asked?;
            if bytes.len() > MAX_FRAME {
                return Err(Error::new("the bundle is larger than a deployment may be"));
            }
            let (kept, entry, files, bindings) = Bundle::decode(&bytes).map_err(Error::new)?;
            if kept != name {
                return Err(Error::new(format!("the bundle is a deployment of {kept}, not {name}")));
            }
            me.prepare_files(&name, &entry, files, bindings).await.map(listed).map_err(Error::new)
        }
    });
    let of = resolve.clone();
    front.register_fn_vm("%cloud-publish", move |vm: &mut Vm, name: String, id: String, expected: Value| -> Result<DataValue, Error> {
        let expected = match vm.to_data(&expected, 1024)? {
            Data::Bool(true) => None,
            Data::Bool(false) => Some(None),
            Data::String(id) => Some(Some(id)),
            _ => return Err(Error::new("what is expected is #t, #f or an id")),
        };
        let (me, name) = of(&name).map_err(Error::new)?;
        let before = me.publish(&name, &id, expected.as_ref().map(|e| e.as_deref())).map_err(Error::new)?;
        Ok(DataValue(before.map_or(Data::Bool(false), Data::String)))
    });
    let of = resolve.clone();
    front.register_fn("%cloud-route", move |name: String| -> DataValue {
        DataValue(of(&name).ok().and_then(|(me, name)| me.route(&name)).map_or(Data::Bool(false), Data::String))
    });
    front.register_fn("%cloud-bundle", move |name: String, id: String| -> DataValue {
        DataValue(resolve(&name).ok().and_then(|(me, name)| me.bundle_of(&name, &id)).map_or(Data::Bool(false), Data::Bytes))
    });
}

/// Kill worker `w`, all of it.
fn kill(w: &Running) {
    match &w.cgroup {
        Some(c) => {
            let _ = c.kill();
        }
        // SAFETY: signalling the worker's waiter, whose death ends the
        // worker, through its pidfd: none once reaped.
        None => unsafe {
            libc::syscall(libc::SYS_pidfd_send_signal, w.pidfd.as_raw_fd(), libc::SIGKILL, std::ptr::null::<libc::siginfo_t>(), 0);
        },
    }
}

/// Serves deployments with a worker process: see the module.
#[derive(Clone)]
pub struct Supervisor {
    options: Rc<Options>,
    egress: Rc<crate::egress::Egress>,
    cgroups: Rc<Option<Cgroups>>,
    state: Rc<RefCell<State>>,
    /// Wakes the front world's driver.
    wake: Arc<Notify>,
    /// Wakes the watcher of deadlines.
    changed: Rc<Notify>,
    /// Told when a worker woken has loaded what is published, or has not.
    woke: Rc<Notify>,
    /// What its workers' cgroups are named with, before their number:
    /// its own among the supervisors that share the cgroups.
    label: Rc<str>,
}

/// The cgroups workers are put in, adopted from the process's own: none,
/// said, where there are none to adopt and `options` do not require
/// them, an error where they do.
pub fn adopt(options: &Options) -> std::io::Result<Option<Cgroups>> {
    match Cgroups::adopt() {
        Ok(c) => Ok(Some(c)),
        Err(e) if options.resources == ResourceConfinement::Required => {
            Err(std::io::Error::new(e.kind(), format!("workers require cgroup resource confinement: {e}")))
        }
        Err(e) => {
            eprintln!("techne cloud: workers' resources are not bounded: {e}");
            Ok(None)
        }
    }
}

impl Supervisor {
    /// A supervisor with its first worker started and ready. Call it on a
    /// current-thread runtime within a `LocalSet`.
    pub async fn start(options: Options) -> std::io::Result<Supervisor> {
        let cgroups = adopt(&options)?;
        let s = Supervisor::asleep(options, Rc::new(cgroups), "")?;
        s.state.borrow_mut().asleep = false;
        s.start_worker().await?;
        Ok(s)
    }

    /// A supervisor with no worker yet: one is started when first needed,
    /// by a deployment prepared or a request ([`Supervisor::sleep`]).
    /// Its workers are put in `cgroups`, which other supervisors may
    /// share, named with `label`, its own among them. Call it on a
    /// current-thread runtime within a `LocalSet`.
    pub fn asleep(options: Options, cgroups: Rc<Option<Cgroups>>, label: &str) -> std::io::Result<Supervisor> {
        let sandbox = options.sandboxing != Sandboxing::Off;
        let egress = Rc::new(crate::egress::Egress::new(options.egress.clone()).map_err(std::io::Error::other)?);
        let s = Supervisor {
            egress,
            options: Rc::new(options),
            cgroups,
            woke: Rc::new(Notify::new()),
            label: label.into(),
            state: Rc::new(RefCell::new(State {
                asleep: true,
                waking: false,
                deferred: 0,
                gate: None,
                store_scope: None,
                worker: None,
                generation: 0,
                next_rid: 0,
                pending: HashMap::new(),
                next_deploy: 0,
                deploys: HashMap::new(),
                ready: HashMap::new(),
                joining: HashMap::new(),
                routes: HashMap::new(),
                stores: HashMap::new(),
                store: None,
                sandbox,
                started: VecDeque::new(),
                repls: HashMap::new(),
                next_cid: 0,
                fetching: 0,
                on_failure: Box::new(|f| eprintln!("techne cloud: {} #{}: {} {}", f.deployment, f.request, f.status, f.message)),
                on_output: Box::new(|p| {
                    for line in p.output.text.lines() {
                        eprintln!("techne cloud: {} #{}: {line}", p.deployment, p.request);
                    }
                    if p.output.truncated {
                        eprintln!("techne cloud: {} #{}: (output cut short)", p.deployment, p.request);
                    }
                }),
            })),
            wake: Arc::new(Notify::new()),
            changed: Rc::new(Notify::new()),
        };
        tokio::task::spawn_local(s.clone().watch());
        Ok(s)
    }

    /// Start its worker now, as the first thing that needs one would:
    /// whether one can be, confined as its options require.
    pub(crate) async fn start_now(&self) -> std::io::Result<()> {
        self.state.borrow_mut().asleep = false;
        let started = self.start_worker().await;
        if started.is_err() {
            self.state.borrow_mut().asleep = true;
        }
        started
    }

    /// Stop the worker until it is next needed, if nothing needs it now:
    /// no request being served, deployment being loaded, REPL client or
    /// fetch. What is published stays so, and is loaded again by the
    /// worker a request or a deployment next starts; what was prepared
    /// and not published is forgotten. Whether it sleeps now.
    pub fn sleep(&self) -> bool {
        let mut s = self.state.borrow_mut();
        let idle = s.pending.is_empty() && s.deploys.is_empty() && s.repls.is_empty() && s.fetching == 0 && !s.waking;
        let Some(w) = s.worker.take_if(|_| idle) else { return s.asleep };
        kill(&w);
        s.asleep = true;
        // Not one that ended by itself: not counted against restarts.
        s.started.pop_back();
        let State { ready, routes, stores, .. } = &mut *s;
        ready.retain(|_, r| routes.get(&r.bundle.name) == Some(&r.bundle.id));
        ready.values_mut().for_each(|r| r.loaded = false);
        stores.retain(|served, _| ready.contains_key(served));
        true
    }

    /// Whether it has no worker, until one is next needed.
    pub fn sleeping(&self) -> bool {
        self.state.borrow().asleep
    }

    /// Have a worker, with what is published loaded: started now if it
    /// sleeps, waited for if it is being started. Why not, if not.
    async fn awake(&self) -> Result<(), String> {
        // Whether there is room for its worker is asked first, of who
        // bounds the workers there are (a fleet).
        let gate = self.state.borrow().gate.clone().filter(|_| self.sleeping());
        if gate.is_some_and(|room| !room()) {
            return Err("no room for another worker now".into());
        }
        let start = {
            let mut s = self.state.borrow_mut();
            let start = s.asleep;
            if start {
                (s.asleep, s.waking) = (false, true);
            }
            start
        };
        if start {
            // The supervisor's own to finish, whoever asked first and
            // however long they wait: one that could not start a worker
            // sleeps again, to be woken by the next that needs it.
            let me = self.clone();
            tokio::task::spawn_local(async move {
                let started = me.restart(true).await;
                let mut s = me.state.borrow_mut();
                s.waking = false;
                s.asleep = !started && s.worker.is_none();
                drop(s);
                me.woke.notify_waiters();
            });
        }
        let deadline = Instant::now() + START + DEPLOY;
        loop {
            {
                let s = self.state.borrow();
                if !s.waking {
                    return if s.worker.is_some() { Ok(()) } else { Err("no worker".into()) };
                }
            }
            if Instant::now() >= deadline {
                return Err("the worker did not start in time".into());
            }
            let _ = tokio::time::timeout(Duration::from_millis(50), self.woke.notified()).await;
        }
    }

    /// Ask `room` before a worker is started for a supervisor asleep:
    /// none is, and what needed it is refused, unless it says there is
    /// room for one (a fleet's bound on workers at once).
    pub fn set_gate(&self, room: impl Fn() -> bool + 'static) {
        self.state.borrow_mut().gate = Some(Rc::new(room));
    }

    /// Have the store asked for this supervisor's namespaces be `store`
    /// (`crate::ask`), as [`Supervisor::install_store`] installs one, but
    /// one that others share: each ask is then a list of `scope`, a
    /// string that is this supervisor's own among them (its tenant), the
    /// namespace and the operation.
    pub fn set_store(&self, store: Asker, scope: &str) {
        let mut s = self.state.borrow_mut();
        s.store = Some(store);
        s.store_scope = Some(scope.into());
    }

    /// The effective confinement of the current worker; none while no
    /// worker is ready (including during replacement).
    pub fn confinement(&self) -> Option<Confinement> {
        let s = self.state.borrow();
        s.worker.as_ref().map(|w| Confinement { sandboxed: s.sandbox, cgroup_bounded: w.cgroup.is_some() })
    }

    /// Call `f` with each failure answered for, the worker's or its own,
    /// in place of writing it to standard error (as
    /// [`crate::Host::on_failure`]; the request's number is the worker's,
    /// 0 for one it never had).
    pub fn on_failure(&self, f: impl FnMut(&Failure) + 'static) {
        self.state.borrow_mut().on_failure = Box::new(f);
    }

    /// Call `f` with what each request wrote, in place of writing it to
    /// standard error (as [`crate::Host::on_output`]).
    pub fn on_output(&self, f: impl FnMut(&Printed) + 'static) {
        self.state.borrow_mut().on_output = Box::new(f);
    }

    /// Start a worker, and wait until it is ready: in its cgroup, then
    /// let through its gate, sandboxed, or not where the kernel does not
    /// allow it and sandboxing is not required.
    async fn start_worker(&self) -> std::io::Result<()> {
        loop {
            let sandbox = self.state.borrow().sandbox;
            match self.spawn(sandbox).await {
                // Once: the sandbox is all that falls back.
                Err(e) if e.kind() == std::io::ErrorKind::Unsupported && sandbox && self.options.sandboxing == Sandboxing::IfAvailable => {
                    eprintln!("techne cloud: workers are not sandboxed here: {e}");
                    self.state.borrow_mut().sandbox = false;
                }
                r => return r,
            }
        }
    }

    async fn spawn(&self, sandbox: bool) -> std::io::Result<()> {
        use std::os::unix::process::CommandExt as _;
        let generation = {
            let mut s = self.state.borrow_mut();
            s.generation += 1;
            s.started.push_back(Instant::now());
            s.generation
        };
        let (ours, theirs) = std::os::unix::net::UnixStream::pair()?;
        let fd = theirs.as_raw_fd();
        let mut command = std::process::Command::new(&self.options.program);
        command.args(&self.options.args).arg("--fd").arg(fd.to_string());
        if !sandbox {
            command.arg("--unsandboxed");
        }
        // Its environment the runtime's settings, no more.
        command.env_clear().envs(std::env::vars_os().filter(|(k, _)| k.to_string_lossy().starts_with("TECHNE_") || k == "RUST_BACKTRACE"));
        command.envs(self.options.env.iter().map(|(k, v)| (k, v)));
        command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null());
        // SAFETY: one system call between fork and exec: the worker keeps
        // its end of the socket.
        unsafe {
            command.pre_exec(move || {
                libc::fcntl(fd, libc::F_SETFD, 0);
                Ok(())
            })
        };
        let mut child = tokio::process::Command::from(command).kill_on_drop(true).spawn()?;
        drop(theirs);
        let pid = child.id().unwrap_or(0);
        // SAFETY: pidfd_open takes a pid and flags; the child is not
        // reaped before its waiter, below, is.
        let pidfd = match unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } {
            fd if fd >= 0 => unsafe { OwnedFd::from_raw_fd(fd as i32) },
            // Not `Unsupported` where pidfds are not (before Linux 5.3):
            // that is the sandbox's refusal, which falls back.
            _ => return Err(std::io::Error::other(format!("pidfd_open: {}", std::io::Error::last_os_error()))),
        };
        let cgroup = match &*self.cgroups {
            Some(c) => match c.worker(&format!("{}w{generation}", self.label), &self.options.cgroup).and_then(|w| w.add(pid).map(|()| w)) {
                Ok(w) => Some(w),
                // As when there are no cgroups: development goes on.
                Err(e) if self.options.resources == ResourceConfinement::IfAvailable => {
                    eprintln!("techne cloud: worker {generation}'s resources are not bounded: {e}");
                    None
                }
                Err(e) => return Err(e),
            },
            None => None,
        };
        ours.set_nonblocking(true)?;
        let mut stream = UnixStream::from_std(ours)?;
        stream.write_all(&[1]).await?;
        let ready = tokio::time::timeout(START, read_frame(&mut stream)).await;
        match ready.map(|r| r.map(|d| d.map(FromWorker::from_data))) {
            Ok(Ok(Some(Ok(FromWorker::Ready)))) => {}
            _ => {
                let status = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
                return Err(match status {
                    Ok(Ok(s)) if s.code() == Some(worker::UNSANDBOXED.into()) => {
                        std::io::Error::new(std::io::ErrorKind::Unsupported, "the kernel does not let it sandbox itself")
                    }
                    _ => std::io::Error::other("the worker did not start"),
                });
            }
        }
        let (mut read, mut write) = stream.into_split();
        let (tx, mut outgoing) = mpsc::unbounded_channel::<Vec<u8>>();
        let send = Link { tx, queued: Default::default(), over: Default::default() };
        let (me, queued) = (self.clone(), send.queued.clone());
        tokio::task::spawn_local(async move {
            while let Some(bytes) = outgoing.recv().await {
                if let Err(e) = write_encoded(&mut write, &bytes).await {
                    me.end_worker(generation, &format!("the worker is not reachable: {e}"));
                    break;
                }
                queued.set(queued.get() - bytes.len());
            }
        });
        let me = self.clone();
        tokio::task::spawn_local(async move {
            loop {
                let message = match read_frame(&mut read).await {
                    Ok(Some(d)) => FromWorker::from_data(d),
                    // Ending, likely: why, its exit says, if it comes.
                    Ok(None) => {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        me.end_worker(generation, "the worker closed its socket");
                        break;
                    }
                    Err(e) => Err(e),
                };
                let fault = match message {
                    Ok(m) => me.heard(generation, m),
                    Err(e) => Some(e.to_string()),
                };
                if let Some(why) = fault {
                    me.end_worker(generation, &format!("the worker misbehaved: {why}"));
                    break;
                }
            }
        });
        let me = self.clone();
        tokio::task::spawn_local(async move {
            let status = child.wait().await;
            let why = match status {
                Ok(s) if s.code() == Some(worker::STUCK.into()) => "a native ran on past a kill".to_string(),
                Ok(s) if s.signal() == Some(libc::SIGKILL) => "the worker was killed (past its memory?)".to_string(),
                Ok(s) => format!("the worker ended ({s})"),
                Err(e) => format!("the worker is gone: {e}"),
            };
            me.end_worker(generation, &why);
        });
        self.state.borrow_mut().worker = Some(Running { generation, send, pidfd, cgroup });
        Ok(())
    }

    /// What worker `generation` said: why it is at fault, if it is.
    fn heard(&self, generation: u64, message: FromWorker) -> Option<String> {
        let mut s = self.state.borrow_mut();
        // Of a worker gone: nothing it says counts.
        if s.worker.as_ref().is_none_or(|w| w.generation != generation) {
            return None;
        }
        // Of a request sent, served no longer.
        let past = |rid: u64, s: &State| (rid <= s.next_rid).then_some(None);
        match message {
            FromWorker::Response { rid, response, streams } => {
                let Some(p) = s.pending.get_mut(&rid) else {
                    return past(rid, &s).unwrap_or_else(|| Some(format!("a response to no request ({rid})")));
                };
                if p.cancelled {
                    return None;
                }
                let Some(reply) = p.reply.take() else { return Some(format!("a second response ({rid})")) };
                let body = streams.then(|| {
                    let (writer, reader) = body::channel();
                    let (queue, pieces) = mpsc::channel(WINDOW + 1);
                    p.body = Some(queue);
                    let me = self.clone();
                    tokio::task::spawn_local(async move {
                        let send = me.state.borrow().worker.as_ref().map(|w| w.send.clone());
                        let credit = |send: &Option<Link>| {
                            send.as_ref().map(|s| s.send(ToWorker::Credit { rid }));
                        };
                        // Its reader gone: the request is abandoned.
                        if !body::relay(pieces, writer, || credit(&send)).await {
                            me.cancel(rid);
                        }
                    });
                    reader
                });
                if !streams {
                    s.pending.remove(&rid);
                }
                let _ = reply.send(Response { head: response, body });
                None
            }
            FromWorker::Body { rid, bytes } => match s.pending.get(&rid) {
                Some(p) if p.cancelled => None,
                Some(p) => match p.body.as_ref().map(|q| q.try_send(Piece::Bytes(bytes))) {
                    Some(Err(mpsc::error::TrySendError::Full(_))) => Some(format!("a body past its credits ({rid})")),
                    Some(_) => None,
                    None => Some(format!("a body to no response streaming ({rid})")),
                },
                None => past(rid, &s).unwrap_or_else(|| Some(format!("a body to no request ({rid})"))),
            },
            // The end of its response's body, or of the request cancelled.
            FromWorker::BodyEnd { rid, error } => match s.pending.remove(&rid) {
                Some(p) if p.cancelled => None,
                Some(p) => match p.body.as_ref().map(|q| q.try_send(Piece::End(error))) {
                    Some(Err(mpsc::error::TrySendError::Full(_))) => Some(format!("a body past its credits ({rid})")),
                    Some(_) => None,
                    None => Some(format!("a body to no response streaming ({rid})")),
                },
                None => past(rid, &s).unwrap_or_else(|| Some(format!("a body to no request ({rid})"))),
            },
            // No more than it was sent chunks for.
            FromWorker::Credit { rid } => match s.pending.get(&rid).and_then(|p| p.credits.as_ref()) {
                Some(c) if c.available_permits() >= WINDOW => Some(format!("credits past its body's chunks ({rid})")),
                Some(c) => {
                    c.add_permits(1);
                    None
                }
                None => None,
            },
            FromWorker::Deployed { id, error, output } => match s.deploys.remove(&id) {
                Some((bundle, reply)) => {
                    let served = bundle.served();
                    let result = error.map_or(Ok(output), Err);
                    if result.is_ok() {
                        s.stores.insert(served.clone(), Rc::new(bundle.bindings.stores.clone()));
                        let published = s.routes.get(&bundle.name) == Some(&bundle.id) || s.ready.get(&served).is_some_and(|r| r.published);
                        let name = bundle.name.clone();
                        s.ready.insert(served.clone(), Ready { bundle, published, since: Instant::now(), loaded: true });
                        s.evict_unpublished(&name, &served);
                    }
                    for waiter in s.joining.remove(&served).unwrap_or_default() {
                        let _ = waiter.send(result.clone());
                    }
                    let _ = reply.send(result);
                    None
                }
                None => Some(format!("a deployment it was not given ({id})")),
            },
            FromWorker::Failure { deployment, request, status, message } => {
                (s.on_failure)(&Failure { deployment, request, status, message });
                None
            }
            FromWorker::Output { deployment, request, text, truncated } => {
                let mut output = techne_vm::vm::Output::default();
                (output.text, output.truncated) = (text, truncated);
                (s.on_output)(&Printed { deployment, request, output });
                None
            }
            FromWorker::Fetch { rid, fid, service, request } => {
                let Some(service) = Service::named(&service) else { return Some(format!("no service {service}")) };
                let (fetching, sent) = (s.fetching, s.next_rid);
                let refused = match s.pending.get_mut(&rid) {
                    Some(p) => {
                        let (count, limit, what) = match service {
                            Service::Fetch => (&mut p.fetches, FETCHES, "fetches"),
                            Service::Kv => (&mut p.store_ops, STORE_OPS, "uses its bindings"),
                        };
                        if *count >= limit {
                            Some(format!("a request {what} {limit} times at most"))
                        } else if p.cancelled {
                            Some("the request was cancelled".to_string())
                        } else if fetching >= FETCHING {
                            Some("too many fetches at once".to_string())
                        } else {
                            *count += 1;
                            None
                        }
                    }
                    None if rid <= sent => Some("the request is over".to_string()),
                    None => return Some(format!("a fetch for no request ({rid})")),
                };
                let send = s.worker.as_ref().map(|w| w.send.clone());
                let refused = refused.or_else(|| (encode(&request).len() > FETCH_BYTES).then(|| "a fetch too large".to_string()));
                // The namespace its binding stands for, in the deployment
                // the request was sent for: not what the worker says.
                let store = match (service, &refused) {
                    (Service::Kv, None) => s.namespace(rid, &request).map(Some),
                    _ => Ok(None),
                };
                let (refused, store) = match store {
                    Ok(store) => (refused, store),
                    Err(why) => (Some(why), None),
                };
                if let Some(why) = refused {
                    send.map(|w| w.send(ToWorker::Fetched { fid, response: Err(why) }));
                    return None;
                }
                s.fetching += 1;
                let me = self.clone();
                tokio::task::spawn_local(async move {
                    // Its place given back however the fetch ends.
                    struct Place(Supervisor);
                    impl Drop for Place {
                        fn drop(&mut self) {
                            self.0.state.borrow_mut().fetching -= 1;
                        }
                    }
                    let place = Place(me.clone());
                    let response = match store {
                        None => me.egress.fetch(&request).await,
                        // Past what may wait there (a store that answers
                        // none): refused, not queued.
                        Some((store, namespace, operation, get)) => {
                            let scope = me.state.borrow().store_scope.clone();
                            let asked = scope.map(|s| Data::String(s.to_string())).into_iter();
                            let asked = Data::List(asked.chain([Data::String(namespace), operation]).collect());
                            let answered = store.ask(asked, STORE).await.map_err(|why| format!("the store: {why}"));
                            if get { answered.and_then(kv_found) } else { answered }
                        }
                    };
                    drop(place);
                    // To the worker that asked, if it is still the one.
                    let current = me.state.borrow().worker.as_ref().filter(|w| w.generation == generation).map(|w| w.send.clone());
                    current.map(|w| w.send(ToWorker::Fetched { fid, response }));
                });
                None
            }
            FromWorker::ReplData { cid, bytes } => {
                // One gone meanwhile, or that cannot keep up, is closed.
                if s.repls.get(&cid).is_some_and(|r| r.queue.try_send(bytes).is_err()) {
                    s.repls.remove(&cid);
                    if let Some(w) = &s.worker {
                        let _ = w.send.send(ToWorker::ReplClose { cid });
                    }
                }
                None
            }
            FromWorker::ReplClose { cid } => {
                s.repls.remove(&cid);
                None
            }
            FromWorker::Ready => Some("ready again".into()),
        }
    }

    /// Worker `generation` is done for, for `why`: kill it, answer for its
    /// requests, and start another with the last deployment.
    fn end_worker(&self, generation: u64, why: &str) {
        let (requests, deploys) = {
            let mut s = self.state.borrow_mut();
            let Some(w) = s.worker.take_if(|w| w.generation == generation) else { return };
            kill(&w);
            // Its REPL connections end with it, and what it had loaded:
            // the next is given what is published.
            s.repls.clear();
            let State { ready, routes, stores, .. } = &mut *s;
            ready.retain(|_, r| routes.get(&r.bundle.name) == Some(&r.bundle.id));
            ready.values_mut().for_each(|r| r.loaded = false);
            stores.retain(|served, _| ready.contains_key(served));
            for (_, waiters) in s.joining.drain() {
                for waiter in waiters {
                    let _ = waiter.send(Err(why.into()));
                }
            }
            (std::mem::take(&mut s.pending), std::mem::take(&mut s.deploys))
        };
        // Their responses' bodies streaming are cut short.
        for (_, mut p) in requests {
            match p.reply.take() {
                Some(reply) => self.answer(reply, &p.name, 500, why),
                None => self.report(&p.name, 500, why),
            }
        }
        for (_, (_, reply)) in deploys {
            let _ = reply.send(Err(why.into()));
        }
        let me = self.clone();
        tokio::task::spawn_local(async move {
            me.restart(false).await;
        });
    }

    /// Start another worker, if not too many were this minute, and give
    /// it the deployments published: until it has loaded one, that one's
    /// requests are refused.
    async fn restart(&self, quiet: bool) -> bool {
        let wait = {
            let mut s = self.state.borrow_mut();
            let now = Instant::now();
            while s.started.front().is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60)) {
                s.started.pop_front();
            }
            match s.started.len() >= RESTARTS {
                true => s.started.front().map(|t| Duration::from_secs(60).saturating_sub(now.duration_since(*t))),
                false => None,
            }
        };
        if let Some(wait) = wait {
            eprintln!("techne cloud: workers keep ending; the next starts in {}s", wait.as_secs());
            tokio::time::sleep(wait).await;
        }
        if let Err(e) = self.start_worker().await {
            eprintln!("techne cloud: no worker: {e}");
            return false;
        }
        let published: Vec<Bundle> = self.state.borrow().ready.values().map(|r| r.bundle.clone()).collect();
        for b in published {
            let (name, served) = (b.name.clone(), b.served());
            match self.send_deploy(b).await {
                Ok(_) => {
                    if let Some(w) = &self.state.borrow().worker {
                        let _ = w.send.send(ToWorker::Activate { name: served });
                    }
                    if !quiet {
                        eprintln!("techne cloud: a new worker serves {name}")
                    }
                }
                Err(e) => eprintln!("techne cloud: the new worker did not load {name}: {e}"),
            }
        }
        true
    }

    /// Have the worker serve no more the deployments its names are no
    /// longer routed to, once their requests are done, and those
    /// prepared long ago and never published.
    fn retire(&self) {
        let mut s = self.state.borrow_mut();
        let State { ready, routes, pending, deploys, stores, worker, .. } = &mut *s;
        let now = Instant::now();
        ready.retain(|served, r| {
            let routed = routes.get(&r.bundle.name) == Some(&r.bundle.id);
            let waited = r.published || now.duration_since(r.since) > UNPUBLISHED;
            let busy = pending.values().any(|p| &p.served == served) || deploys.values().any(|(b, _)| &b.served() == served);
            if routed || !waited || busy {
                return true;
            }
            stores.remove(served);
            if let Some(w) = worker {
                let _ = w.send.send(ToWorker::Retire { name: served.clone() });
            }
            false
        });
    }

    /// Answer `reply` for deployment `name` with `status`, and report it.
    fn answer(&self, reply: oneshot::Sender<Response>, name: &str, status: u16, why: &str) {
        self.report(name, status, why);
        let _ = reply.send(Response { head: crate::failed(status), body: None });
    }

    fn report(&self, name: &str, status: u16, why: &str) {
        let failure = Failure { deployment: name.into(), request: 0, status, message: why.into() };
        (self.state.borrow_mut().on_failure)(&failure);
    }

    /// Kill the worker over a request it has not answered past its wall
    /// time and grace, or for not reading what it is sent; cut short a
    /// response's body streaming then, and cancel requests the front
    /// abandoned; for ever.
    async fn watch(self) {
        loop {
            self.retire();
            let now = Instant::now();
            let (abandoned, cut): (Vec<u64>, Vec<u64>) = {
                let s = self.state.borrow();
                let abandoned = s.pending.iter().filter(|(_, p)| p.reply.as_ref().is_some_and(|r| r.is_closed()));
                let cut = s.pending.iter().filter(|(_, p)| !p.cancelled && p.reply.is_none() && p.deadline <= now);
                (abandoned.map(|(r, _)| *r).collect(), cut.map(|(r, _)| *r).collect())
            };
            for rid in abandoned {
                self.cancel(rid);
            }
            // Its body cut short; the worker given a grace to be done.
            for rid in cut {
                let name = self.state.borrow_mut().pending.get_mut(&rid).map(|p| {
                    p.deadline = now + GRACE;
                    p.name.clone()
                });
                self.report(&name.unwrap_or_default(), 504, "the response's body was cut short past its wall time");
                self.cancel(rid);
            }
            let (next, late, worker) = {
                let s = self.state.borrow();
                let late = s.pending.values().any(|p| p.deadline <= now);
                let next = s.pending.values().map(|p| p.deadline).min();
                (next, late, s.worker.as_ref().map(|w| (w.generation, w.send.over.get())))
            };
            match worker {
                Some((g, _)) if late => self.end_worker(g, "the worker did not answer a request past its wall time"),
                Some((g, true)) => self.end_worker(g, "the worker does not read what it is sent"),
                _ => {}
            }
            // Abandoned requests are looked for now and then.
            let until =
                next.map_or(Duration::from_millis(250), |t| t.saturating_duration_since(Instant::now())).min(Duration::from_millis(250));
            let _ = tokio::time::timeout(until, self.changed.notified()).await;
        }
    }

    /// Cancel request `rid`, if it is not yet: the worker is told.
    fn cancel(&self, rid: u64) {
        let mut s = self.state.borrow_mut();
        let Some(p) = s.pending.get_mut(&rid).filter(|p| !p.cancelled) else { return };
        p.cancel();
        if let Some(w) = &s.worker {
            let _ = w.send.send(ToWorker::Cancel { rid });
        }
    }

    /// Ask the deployment published as `name` to answer `request`, as
    /// [`crate::Host::fetch`] does.
    pub fn fetch(&self, name: &str, request: Data, body: Option<Reader>) -> oneshot::Receiver<Response> {
        self.fetch_routed(name, request, body, false)
    }

    /// [`Supervisor::fetch`], the request routed to the deployment's
    /// debug instance if `debug`, as [`crate::Host::fetch_routed`] does:
    /// the worker is waited for [`crate::DEBUG_PAUSE`] longer.
    pub fn fetch_routed(&self, name: &str, request: Data, body: Option<Reader>, debug: bool) -> oneshot::Receiver<Response> {
        let (tx, rx) = oneshot::channel();
        let mut s = self.state.borrow_mut();
        // Pinned here to the deployment the name is routed to now.
        let Some(served) = s.routes.get(name).map(|id| served(name, id)) else {
            drop(s);
            self.answer(tx, name, 404, "no such deployment");
            return rx;
        };
        // Asleep, or being woken: asked once its worker has what is
        // published.
        if s.asleep || s.waking {
            // A bounded number wait so: each holds its request.
            if s.deferred >= DEFERRED {
                drop(s);
                self.answer(tx, name, 503, "too many requests wait for the worker to start");
                return rx;
            }
            s.deferred += 1;
            drop(s);
            let (me, name) = (self.clone(), name.to_string());
            let wall = self.options.wall;
            tokio::task::spawn_local(async move {
                let mut tx = tx;
                // Abandoned by who asked, while it waits or after:
                // dropped, and abandoned there too.
                let woken = tokio::select! {
                    woken = tokio::time::timeout(wall, me.awake()) => Some(woken.unwrap_or_else(|_| Err("the worker did not start in time".into()))),
                    _ = tx.closed() => None,
                };
                me.state.borrow_mut().deferred -= 1;
                match woken {
                    Some(Ok(())) => {
                        let served = me.fetch_routed(&name, request, body, debug);
                        tokio::select! {
                            response = served => {
                                if let Ok(response) = response {
                                    let _ = tx.send(response);
                                }
                            }
                            _ = tx.closed() => {}
                        }
                    }
                    Some(Err(why)) => me.answer(tx, &name, 503, &why),
                    None => {}
                }
            });
            return rx;
        }
        let Some(send) = s.worker.as_ref().map(|w| w.send.clone()) else {
            drop(s);
            self.answer(tx, name, 503, "no worker");
            return rx;
        };
        // A worker started anew has yet to load it.
        if !s.ready.get(&served).is_some_and(|r| r.loaded) {
            drop(s);
            self.answer(tx, name, 503, "the deployment is being loaded");
            return rx;
        }
        s.next_rid += 1;
        let rid = s.next_rid;
        let pause = if debug { crate::DEBUG_PAUSE } else { Duration::ZERO };
        let deadline = Instant::now() + self.options.wall + pause + GRACE;
        let credits = body.as_ref().map(|_| Rc::new(Semaphore::new(WINDOW)));
        let _ = send.send(ToWorker::Request { rid, name: served.clone(), request, streams: body.is_some(), debug });
        let pump = body.zip(credits.clone()).map(|(reader, credits)| {
            let send = send.clone();
            let pumping = tokio::task::spawn_local(body::pump(reader, credits, move |piece| {
                send.send(match piece {
                    Piece::Bytes(bytes) => ToWorker::Body { rid, bytes },
                    Piece::End(error) => ToWorker::BodyEnd { rid, error },
                })
                .is_ok()
            }));
            pumping.abort_handle()
        });
        let stores = s.stores.get(&served).cloned().unwrap_or_default();
        let pending = Pending {
            name: name.into(),
            served,
            reply: Some(tx),
            deadline,
            body: None,
            credits,
            pump,
            cancelled: false,
            fetches: 0,
            store_ops: 0,
            stores,
        };
        s.pending.insert(rid, pending);
        drop(s);
        self.changed.notify_one();
        rx
    }

    /// Have the worker serve the deployment whose entry module is `entry`
    /// as `name`, its files those under the entry's directory (but hidden
    /// ones, and in hidden directories, `target` and `node_modules`):
    /// prepared, then published whatever the name was routed to. What
    /// loading it wrote, or why it was not.
    pub async fn deploy(&self, name: &str, entry: &Path) -> Result<String, String> {
        self.deploy_with(name, entry, Bindings::default()).await
    }

    /// [`Supervisor::deploy`], the deployment given `bindings`: once
    /// published, the name's requests reach the namespaces these name.
    pub async fn deploy_with(&self, name: &str, entry: &Path, bindings: Bindings) -> Result<String, String> {
        let prepared = self.prepare(name, entry, bindings).await?;
        self.publish(name, &prepared.id, None)?;
        Ok(prepared.output)
    }

    /// Have the worker load the deployment whose entry module is `entry`,
    /// for `name`, with `bindings`, its files those under the entry's
    /// directory as [`Supervisor::deploy`] takes them: its id, by which
    /// to publish it. Nothing served changes; one that does not load is
    /// an error and no more. The same deployment prepared again is the
    /// one there is.
    pub async fn prepare(&self, name: &str, entry: &Path, bindings: Bindings) -> Result<Prepared, String> {
        let dir = entry.parent().ok_or("an entry in no directory")?;
        let files = bundle(dir)?;
        let entry = entry.strip_prefix(dir).map_err(|e| e.to_string())?.to_string_lossy().into_owned();
        self.prepare_files(name, &entry, files, bindings).await
    }

    /// [`Supervisor::prepare`], of `files` given, by their paths, `entry`
    /// among them.
    pub async fn prepare_files(
        &self,
        name: &str,
        entry: &str,
        files: Vec<(String, Vec<u8>)>,
        bindings: Bindings,
    ) -> Result<Prepared, String> {
        if name.is_empty() || name.contains('@') {
            return Err(format!("{name}: not a deployment's name"));
        }
        let id = deployment_id(name, entry, &files, &bindings);
        let key = served(name, &id);
        if self.sleeping() || self.state.borrow().waking {
            self.awake().await?;
        }
        let joined = {
            let mut s = self.state.borrow_mut();
            if s.ready.get(&key).is_some_and(|r| r.loaded) {
                return Ok(Prepared { id, output: String::new() });
            }
            // Being loaded already: that loading is this one's too.
            if s.deploys.values().any(|(b, _)| b.served() == key) {
                let (tx, rx) = oneshot::channel();
                s.joining.entry(key).or_default().push(tx);
                Some(rx)
            } else if s.deploys.values().filter(|(b, _)| b.name == name).count() >= UNPUBLISHED_EACH {
                return Err(format!("too many deployments of {name} are being prepared"));
            } else {
                None
            }
        };
        let output = match joined {
            // No longer than the loading it joined is waited for.
            Some(loading) => match tokio::time::timeout(DEPLOY, loading).await {
                Ok(loaded) => loaded.map_err(|_| "the worker is gone".to_string())?,
                Err(_) => Err("the worker did not load it in time".into()),
            },
            None => self.send_deploy(Bundle { name: name.into(), id: id.clone(), entry: entry.into(), files, bindings }).await,
        };
        output.map(|output| Prepared { id, output })
    }

    /// Route `name`'s requests to its deployment `id`, prepared, from now
    /// on: those sent already finish where they were sent. With
    /// `expected`, only if the name is routed there now (to that id, or
    /// nowhere): else an error, and nothing changes. The id it was routed
    /// to before.
    pub fn publish(&self, name: &str, id: &str, expected: Option<Option<&str>>) -> Result<Option<String>, String> {
        let mut s = self.state.borrow_mut();
        let now = s.routes.get(name).cloned();
        if let Some(expected) = expected
            && now.as_deref() != expected
        {
            return Err(format!("{name} is at {}, not {}", now.as_deref().unwrap_or("nothing"), expected.unwrap_or("nothing")));
        }
        let served = served(name, id);
        let Some(ready) = s.ready.get_mut(&served).filter(|r| r.loaded) else {
            return Err(format!("{name} has no deployment {id} prepared"));
        };
        ready.published = true;
        s.routes.insert(name.into(), id.into());
        if let Some(w) = &s.worker {
            let _ = w.send.send(ToWorker::Activate { name: served });
        }
        drop(s);
        self.changed.notify_one();
        Ok(now)
    }

    /// The id of the deployment `name`'s requests go to.
    pub fn route(&self, name: &str) -> Option<String> {
        self.state.borrow().routes.get(name).cloned()
    }

    /// The ids of `name`'s deployments the worker has loaded, in order.
    pub fn prepared(&self, name: &str) -> Vec<String> {
        let mut ids: Vec<String> =
            self.state.borrow().ready.values().filter(|r| r.bundle.name == name).map(|r| r.bundle.id.clone()).collect();
        ids.sort();
        ids
    }

    /// Have a task of `front` keep what requests keep in their KV
    /// namespaces (`crate::ask`, as `store`): `(%store-next)` returns the
    /// next operation asked, a list of its number and what is asked, a
    /// list of its namespace (a string, the deployment's binding looked
    /// up) and the operation, a dict of its `op` and fields, a value to
    /// keep a bytevector (`kv_operation`); `(%store-reply number ok
    /// value)` answers it. One not answered within ten seconds is failed,
    /// and not handed on if it still waited; as many wait as may be asked
    /// at once, and more are refused. Without a store, requests keep
    /// nothing.
    pub fn install_store(&self, front: &mut Vm) {
        self.wakes(front);
        self.state.borrow_mut().store = Some(crate::ask::install(front, "store", FETCHING));
    }

    /// Give `front` what deploys: for Lisp there that keeps the record of
    /// deployments and decides what is published (`lisp/cloud/control.scm`).
    /// `(%cloud-prepare-directory name entry)` prepares the deployment
    /// whose entry module is the file ENTRY, bound as the manifest beside
    /// it says ([`read_bindings`]: the manifest is not read in `front`),
    /// and `(%cloud-prepare name entry bindings)` the deployment whose
    /// entry module is the file ENTRY, with BINDINGS (a dict of `env` and
    /// `stores`, as `lisp/cloud/manifest.scm` reads them), as
    /// [`Supervisor::prepare`] does: a list of its id and what loading
    /// wrote. `(%cloud-publish name id expected)` publishes it, as
    /// [`Supervisor::publish`] does: whatever NAME is routed to if
    /// EXPECTED is #t, only if it is routed to nothing if #f, or to the
    /// id EXPECTED; the id it was routed to, or #f. `(%cloud-route name)`
    /// is that id now. `(%cloud-bundle name id)` is all of a deployment
    /// prepared, as a bytevector to keep, or #f, and
    /// `(%cloud-prepare-bundle name bytes)` prepares the deployment of
    /// NAME kept so, as `%cloud-prepare` does. What fails is an error.
    pub fn install_control(&self, front: &mut Vm) {
        self.wakes(front);
        let me = self.clone();
        install_control_with(front, Rc::new(move |name: &str| Ok((me.clone(), name.to_string()))));
    }

    /// All of deployment `id` of `name`, prepared, as bytes to keep.
    fn bundle_of(&self, name: &str, id: &str) -> Option<Vec<u8>> {
        self.state.borrow().ready.get(&served(name, id)).map(|r| r.bundle.encode())
    }

    /// Have what `front`'s tasks wait on wake the supervisor's driver
    /// (`Supervisor::run`), as `install` does: for a front world given
    /// natives of the caller's own.
    pub fn wakes(&self, front: &mut Vm) {
        let wake = self.wake.clone();
        front.set_wake_notifier(move || wake.notify_one());
    }

    async fn send_deploy(&self, bundle: Bundle) -> Result<String, String> {
        let (tx, rx) = oneshot::channel();
        let served = bundle.served();
        let sent = {
            let mut s = self.state.borrow_mut();
            let Some(send) = s.worker.as_ref().map(|w| w.send.clone()) else { return Err("no worker".into()) };
            s.next_deploy += 1;
            let id = s.next_deploy;
            let message = ToWorker::Deploy {
                id,
                name: bundle.served(),
                entry: bundle.entry.clone(),
                files: bundle.files.clone(),
                wall_ms: self.options.wall.as_millis() as u64,
                dev: self.options.dev,
                env: bundle.bindings.env.clone(),
            };
            s.deploys.insert(id, (bundle, tx));
            let _ = send.send(message);
            id
        };
        match tokio::time::timeout(DEPLOY, rx).await {
            Ok(r) => r.map_err(|_| "the worker is gone".to_string())?,
            // Given up: no one joins it from now on, those who did are
            // told, and a worker that says later that it loaded it says
            // what it was not asked (`heard`).
            Err(_) => {
                let why = "the worker did not load it in time".to_string();
                let mut s = self.state.borrow_mut();
                s.deploys.remove(&sent);
                for waiter in s.joining.remove(&served).unwrap_or_default() {
                    let _ = waiter.send(Err(why.clone()));
                }
                Err(why)
            }
        }
    }

    /// Accept REPL clients on `addr` (loopback, for development), each
    /// relayed to the worker's dev world, for no one in particular: the
    /// address bound. Anyone who can connect evaluates there: what
    /// listens elsewhere authenticates its clients first, and relays
    /// them itself ([`Supervisor::relay`]).
    pub async fn serve_repl(&self, addr: &str) -> std::io::Result<std::net::SocketAddr> {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let bound = listener.local_addr()?;
        let me = self.clone();
        tokio::task::spawn_local(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let _ = stream.set_nodelay(true);
                me.relay(stream, None, Vec::new());
            }
        });
        Ok(bound)
    }

    /// Relay `stream`, a REPL client, to the worker as a connection of its
    /// own, until either end closes it: for `principal`, if the caller
    /// authenticated the client as that (the worker's REPL keeps what is
    /// each principal's apart), `sent` being what the client sent already
    /// that is the REPL's to read.
    pub fn relay(&self, stream: impl tokio::io::AsyncRead + tokio::io::AsyncWrite + 'static, principal: Option<String>, sent: Vec<u8>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (cid, send) = {
            let mut s = self.state.borrow_mut();
            let Some(send) = s.worker.as_ref().map(|w| w.send.clone()) else { return };
            s.next_cid += 1;
            (s.next_cid, send)
        };
        let (queue, mut queued) = mpsc::channel::<Vec<u8>>(REPL_QUEUE);
        let _ = send.send(ToWorker::ReplOpen { cid, principal });
        for bytes in sent.chunks(REPL_CHUNK) {
            let _ = send.send(ToWorker::ReplData { cid, bytes: bytes.to_vec() });
        }
        let (mut from, mut to) = tokio::io::split(stream);
        tokio::task::spawn_local(async move {
            while let Some(bytes) = queued.recv().await {
                if to.write_all(&bytes).await.is_err() {
                    break;
                }
            }
            let _ = to.shutdown().await;
        });
        let me = self.clone();
        let reader = tokio::task::spawn_local(async move {
            let mut chunk = vec![0; REPL_CHUNK];
            while let Ok(n @ 1..) = from.read(&mut chunk).await {
                if send.send(ToWorker::ReplData { cid, bytes: chunk[..n].to_vec() }).is_err() {
                    break;
                }
            }
            let ended = me.state.borrow_mut().repls.remove(&cid);
            if ended.is_some() {
                let _ = send.send(ToWorker::ReplClose { cid });
            }
        });
        let reader = reader.abort_handle();
        self.state.borrow_mut().repls.insert(cid, Repl { queue, reader });
    }

    /// Define `(host-fetch deployment request #:debug #f)` in the front
    /// world, as [`crate::Host::install`] does.
    pub fn install(&self, front: &mut Vm) {
        self.wakes(front);
        let me = self.clone();
        body::install_front(front, move |deployment, request, body, debug| me.fetch_routed(deployment, request, body, debug));
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

/// How long a worker may take to load a deployment: its loading's bound
/// and more.
const DEPLOY: Duration = Duration::from_secs(60);

/// The files under `dir`, by their paths in it: not hidden ones (a
/// deployment's secrets are in one), nor in hidden directories, `target`
/// or `node_modules`, nor through symbolic links.
pub fn bundle(dir: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut files = Vec::new();
    let mut total = 0;
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(d) = dirs.pop() {
        for entry in std::fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))?.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            match entry.file_type() {
                Ok(t) if t.is_dir() => {
                    if !(name.starts_with('.') || name == "target" || name == "node_modules") {
                        dirs.push(path);
                    }
                }
                Ok(t) if t.is_file() && !name.starts_with('.') => {
                    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                    total += bytes.len();
                    if total > BUNDLE_BYTES {
                        return Err(format!("{}: more than {BUNDLE_BYTES} bytes of files", dir.display()));
                    }
                    let relative = path.strip_prefix(dir).map_err(|e| e.to_string())?.to_string_lossy().into_owned();
                    files.push((relative, bytes));
                }
                _ => {}
            }
        }
    }
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict(entries: &[(&str, Data)]) -> Data {
        Data::Dict(entries.iter().map(|(k, v)| (Data::Symbol((*k).into()), v.clone())).collect())
    }

    fn string(s: &str) -> Data {
        Data::String(s.into())
    }

    /// The store is handed what an operation has and no more: a value
    /// encoded, no symbol of the tenant's.
    #[test]
    fn a_kv_operation_is_handed_on_without_the_tenants_symbols() {
        let value = Data::List(vec![Data::Symbol("theirs".into()), Data::Keyword("a b".into())]);
        let asked = dict(&[
            ("op", Data::Symbol("put".into())),
            ("binding", string("cache")),
            ("key", string("k")),
            ("value", value.clone()),
            ("ttl", Data::Int(5)),
            ("theirs", Data::Symbol("more".into())),
        ]);
        let (binding, operation, get) = kv_operation(&asked).unwrap();
        assert_eq!((binding.as_str(), get), ("cache", false));
        let expected = dict(&[
            ("op", Data::Symbol("put".into())),
            ("key", string("k")),
            ("value", Data::Bytes(encode(&value))),
            ("ttl", Data::Int(5)),
        ]);
        assert_eq!(operation, expected);
        // What a `get` finds is what was kept.
        let found = dict(&[("found", Data::Bool(true)), ("value", Data::Bytes(encode(&value)))]);
        assert_eq!(kv_found(found).unwrap(), dict(&[("found", Data::Bool(true)), ("value", value)]));
        assert!(kv_found(dict(&[("value", Data::Bytes(vec![255, 255]))])).is_err());
        // Not operations.
        for bad in [
            dict(&[("op", Data::Symbol("drop".into())), ("binding", string("cache"))]),
            dict(&[("op", Data::Symbol("get".into())), ("key", string("k"))]),
            dict(&[("op", Data::Symbol("get".into())), ("binding", string("cache")), ("key", Data::Symbol("k".into()))]),
            dict(&[("op", string("get")), ("binding", string("cache")), ("key", string("k"))]),
            dict(&[("op", Data::Symbol("put".into())), ("binding", string("cache")), ("key", string("k"))]),
            dict(&[("op", Data::Symbol("list".into())), ("binding", string("cache")), ("limit", string("all"))]),
            string("get"),
        ] {
            assert!(kv_operation(&bad).is_err(), "{bad:?}");
        }
        let (_, listing, _) = kv_operation(&dict(&[("op", Data::Symbol("list".into())), ("binding", string("cache"))])).unwrap();
        assert_eq!(listing, dict(&[("op", Data::Symbol("list".into())), ("prefix", string("")), ("limit", Data::Int(1000))]));
    }

    /// A deployment's files are not its hidden ones: its secrets stay
    /// with the supervisor.
    #[test]
    fn a_bundle_has_no_hidden_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("lib")).unwrap();
        std::fs::create_dir_all(dir.path().join(".techne/kv")).unwrap();
        for file in ["app.scm", ".secrets.scm", "lib/a.scm", "lib/.hidden", ".techne/kv/ns.kv"] {
            std::fs::write(dir.path().join(file), "x").unwrap();
        }
        let names: Vec<String> = bundle(dir.path()).unwrap().into_iter().map(|(name, _)| name).collect();
        assert_eq!(names, ["app.scm", "lib/a.scm"]);
    }
}
