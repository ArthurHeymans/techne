//! What a deployment's requests fetch (`fetch-url`): asked of the host's
//! broker ([`Host::set_broker`](crate::Host::set_broker)), which a worker's
//! host has ask its supervisor, and which fetches within an egress policy
//! ([`crate::egress`], [`through`]). Each request may fetch a bounded
//! number of times (`Limits::fetches`); a world's code outside a request
//! fetches nothing.

use std::{cell::Cell, rc::Rc};

use techne_vm::{
    data::Data,
    vm::{Error, Vm},
};
use tokio::sync::oneshot;

use crate::{REQUEST_BYTES, body::DataValue, egress::Egress};

/// A fetch a request asks for: its deployment, the tag it was served
/// under ([`Host::fetch_as`](crate::Host::fetch_as)), and what to fetch
/// (as [`Egress::fetch`] takes it).
pub struct Outbound {
    pub deployment: String,
    pub tag: u64,
    pub request: Data,
}

/// What fetches for requests: the response (as [`Egress::fetch`] gives
/// it), or why not.
pub type Broker = Rc<dyn Fn(Outbound) -> oneshot::Receiver<Result<Data, String>>>;

/// A broker fetching with `egress` here.
pub fn through(egress: Rc<Egress>) -> Broker {
    Rc::new(move |o: Outbound| {
        let (tx, rx) = oneshot::channel();
        let egress = egress.clone();
        tokio::task::spawn_local(async move {
            let _ = tx.send(egress.fetch(&o.request).await);
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
  (%host-fetch-url (dict 'url url 'method method 'headers headers 'body (or body (bytevector)))))
"#;

/// Define `%host-fetch-url` in `vm`: the request whose task calls it asks
/// its broker.
pub(crate) fn install(vm: &mut Vm) {
    vm.register_async("%host-fetch-url", 1, |vm, args| {
        let asked = (|| {
            let context = vm.lifetime_data().and_then(|d| d.downcast::<Context>().ok());
            let context = context.ok_or_else(|| Error::new("fetch-url: only a request fetches"))?;
            let broker = context.broker.clone().ok_or_else(|| Error::new("fetch-url: this host fetches nothing"))?;
            if context.fetches.get() >= context.limit {
                return Err(Error::new(format!("fetch-url: a request fetches {} times at most", context.limit)));
            }
            context.fetches.set(context.fetches.get() + 1);
            let request = args.value(vm, 0)?;
            let request = vm.to_data(&request, REQUEST_BYTES)?;
            Ok(broker(Outbound { deployment: context.deployment.clone(), tag: context.tag, request }))
        })();
        async move {
            let fetched = asked?.await.map_err(|_| Error::new("fetch-url: the host stopped"))?;
            fetched.map(DataValue).map_err(|e| Error::new(format!("fetch-url: {e}")))
        }
    });
}
