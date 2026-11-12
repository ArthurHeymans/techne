//! What a deployment's requests ask of the world outside: what they fetch
//! (`fetch-url`) and what they keep in the KV namespaces bound to the
//! deployment (`kv-get`, `kv-put!`, `kv-delete!`, `kv-list`). Each is
//! asked of the host's broker
//! ([`Host::set_broker`](crate::Host::set_broker)), which a worker's
//! host has ask its supervisor, and which fetches within an egress policy
//! ([`crate::egress`], [`through`]) or serves the namespace the binding
//! named stands for. Each request may fetch a bounded number of times
//! (`Limits::fetches`) and use its bindings a bounded number
//! (`Limits::store_ops`); a world's code outside a request asks nothing.
//!
//! A binding is named here as the deployment names it. What it stands
//! for (which namespace) is the broker's to know and to check: a world
//! says nothing the broker trusts.

use std::{cell::Cell, rc::Rc};

use techne_vm::{
    data::Data,
    vm::{Error, Vm},
};
use tokio::sync::oneshot;

use crate::{REQUEST_BYTES, body::DataValue, egress::Egress};

/// What a request asks a broker for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    /// A fetch, as [`Egress::fetch`] takes it.
    Fetch,
    /// An operation on a KV namespace: a dict of its `op` (`get`, `put`,
    /// `delete`, `list`), the `binding` the deployment names it by (a
    /// string) and the operation's own fields.
    Kv,
}

impl Service {
    /// Its name on the wire.
    pub fn name(self) -> &'static str {
        match self {
            Service::Fetch => "fetch",
            Service::Kv => "kv",
        }
    }

    pub fn named(name: &str) -> Option<Service> {
        [Service::Fetch, Service::Kv].into_iter().find(|s| s.name() == name)
    }
}

/// What a request asks for: its deployment, the tag it was served under
/// ([`Host::fetch_as`](crate::Host::fetch_as)), of which service, and
/// what.
pub struct Outbound {
    pub deployment: String,
    pub tag: u64,
    pub service: Service,
    pub request: Data,
}

/// What answers requests' asks: the response (for a fetch, as
/// [`Egress::fetch`] gives it), or why not.
pub type Broker = Rc<dyn Fn(Outbound) -> oneshot::Receiver<Result<Data, String>>>;

/// A broker fetching with `egress` here, and keeping nothing.
pub fn through(egress: Rc<Egress>) -> Broker {
    Rc::new(move |o: Outbound| {
        let (tx, rx) = oneshot::channel();
        let egress = egress.clone();
        tokio::task::spawn_local(async move {
            let _ = tx.send(match o.service {
                Service::Fetch => egress.fetch(&o.request).await,
                Service::Kv => Err("this host keeps nothing".into()),
            });
        });
        rx
    })
}

/// A request's own, attached to its lifetime, which its tasks find.
pub(crate) struct Context {
    pub(crate) deployment: String,
    pub(crate) tag: u64,
    pub(crate) broker: Option<Broker>,
    pub(crate) fetches: Cell<usize>,
    pub(crate) limit: usize,
    pub(crate) store_ops: Cell<usize>,
    pub(crate) store_limit: usize,
}

/// `(fetch-url url #:method #:headers #:body)` in a deployment's world,
/// over `%host-fetch-url`: the response, a dict of its `status`, `headers`
/// and `body` (a bytevector), and `url`.
pub(crate) const LISP: &str = r#"
(define (fetch-url url #:method [method "GET"] #:headers [headers (dict)] #:body [body #f])
  "Fetch URL with METHOD, HEADERS (a dict of strings) and BODY (a string or
bytevector) if given: the response, a dict of its status, headers (names
lowercase), body (a bytevector) and url (after redirects). Only a request
fetches, a bounded number of times, and only what the host allows."
  (%host-ask "fetch" (dict 'url url 'method method 'headers headers 'body (or body (bytevector)))))

(define (%kv-ask binding op . fields)
  (unless (and (dict? binding) (eq? (dict-ref binding 'kind #f) 'kv))
    (error "Not a KV binding:" binding))
  (%host-ask "kv" (apply dict 'op op 'binding (dict-ref binding 'name) fields)))

(define (kv-get binding key [default #f])
  "Return the value kept under KEY, a string, in the KV namespace BINDING.
DEFAULT if none is, or it has expired. BINDING is one of the
deployment's, from its `env`. Only a request asks, a bounded number of
times."
  (let ((found (%kv-ask binding 'get 'key key)))
    (if (dict-ref found 'found) (dict-ref found 'value) default)))

(define (kv-put! binding key value #:ttl [ttl #f])
  "Keep VALUE under KEY, a string, in the KV namespace BINDING.
VALUE is data: strings, numbers, bytevectors, lists, dicts. With TTL, it
is kept for that many seconds. The last write wins."
  (%kv-ask binding 'put 'key key 'value value 'ttl ttl)
  (void))

(define (kv-delete! binding key)
  "Keep nothing under KEY any more in the KV namespace BINDING."
  (%kv-ask binding 'delete 'key key)
  (void))

(define (kv-list binding #:prefix [prefix ""] #:limit [limit 1000])
  "Return the keys of the KV namespace BINDING that start with PREFIX.
In order, at most LIMIT of them, which is at most 1000."
  (%kv-ask binding 'list 'prefix prefix 'limit limit))
"#;

/// Define `%host-ask` in `vm`: the request whose task calls it asks its
/// broker, within its bounds.
pub(crate) fn install(vm: &mut Vm) {
    vm.register_async("%host-ask", 2, |vm, args| {
        let service = args.get::<String>(vm, 0).ok().and_then(|s| Service::named(&s));
        let what = match service {
            Some(Service::Kv) => "kv",
            _ => "fetch-url",
        };
        let asked = (|| {
            let service = service.ok_or_else(|| Error::new("no such service"))?;
            let context = vm.lifetime_data().and_then(|d| d.downcast::<Context>().ok());
            let context = context.ok_or_else(|| Error::new("only a request asks"))?;
            let broker = context.broker.clone().ok_or_else(|| Error::new("this host fetches and keeps nothing"))?;
            let (count, limit, times) = match service {
                Service::Fetch => (&context.fetches, context.limit, "fetches"),
                Service::Kv => (&context.store_ops, context.store_limit, "uses its bindings"),
            };
            if count.get() >= limit {
                return Err(Error::new(format!("a request {times} {limit} times at most")));
            }
            count.set(count.get() + 1);
            let request = args.value(vm, 1)?;
            let request = vm.to_data(&request, REQUEST_BYTES)?;
            Ok(broker(Outbound { deployment: context.deployment.clone(), tag: context.tag, service, request }))
        })();
        async move {
            let asked = asked.map_err(|e| Error::new(format!("{what}: {}", e.msg)))?;
            let answered = asked.await.map_err(|_| Error::new(format!("{what}: the host stopped")))?;
            answered.map(DataValue).map_err(|e| Error::new(format!("{what}: {e}")))
        }
    });
}
