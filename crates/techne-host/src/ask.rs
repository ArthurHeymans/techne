//! Asking a task of a world for answers, from Rust: how a supervisor has
//! Lisp decide or keep something (what a deployment keeps, whether a
//! token holds) without Lisp being called from where the world is not
//! being driven.
//!
//! [`install`] defines two natives in the world, by a name: `(%NAME-next)`
//! waits for the next thing asked and returns it, a list of its number
//! and what was asked, data; `(%NAME-reply number ok value)` answers it,
//! with VALUE, data, if OK, else with why not, a string. A task of the
//! world loops over them. [`Asker::ask`] asks, and waits for the answer
//! a bounded time; a bounded number wait to be taken, more are refused;
//! one whose asker has stopped waiting is not handed on. What is asked
//! is data the caller made: the world reads nothing else of it.

use std::{cell::RefCell, collections::HashMap, rc::Rc, time::Duration};

use techne_vm::{
    api::Value,
    data::Data,
    vm::{Error, Vm},
};
use tokio::sync::{mpsc, oneshot};

use crate::body::DataValue;

struct Ask {
    what: Data,
    reply: oneshot::Sender<Result<Data, String>>,
}

/// What asks the task of a world ([`install`]).
#[derive(Clone)]
pub struct Asker {
    tx: mpsc::Sender<Ask>,
}

impl Asker {
    /// The answer to `what`, or why there is none: the task's own
    /// refusal, "busy" when too many wait already, or "no answer" when
    /// none came `within`.
    pub async fn ask(&self, what: Data, within: Duration) -> Result<Data, String> {
        let (reply, answer) = oneshot::channel();
        if self.tx.try_send(Ask { what, reply }).is_err() {
            return Err("busy".into());
        }
        match tokio::time::timeout(within, answer).await {
            Ok(Ok(answer)) => answer,
            _ => Err("no answer".into()),
        }
    }
}

/// Define `%NAME-next` and `%NAME-reply` in `vm`, as the module says, at
/// most `waiting` asks waiting to be taken: what asks them.
pub fn install(vm: &mut Vm, name: &str, waiting: usize) -> Asker {
    let (tx, rx) = mpsc::channel::<Ask>(waiting);
    let rx = Rc::new(tokio::sync::Mutex::new(rx));
    type Taken = (u64, HashMap<u64, oneshot::Sender<Result<Data, String>>>);
    let taken = Rc::new(RefCell::new(Taken::default()));
    let asked = taken.clone();
    vm.register_async(&format!("%{name}-next"), 0, move |_vm, _args| {
        let (rx, asked) = (rx.clone(), asked.clone());
        async move {
            let mut rx = rx.lock().await;
            let ask = loop {
                let ask = rx.recv().await.ok_or_else(|| Error::new("no one asks any more"))?;
                // No one waits for it any more: not done late.
                if !ask.reply.is_closed() {
                    break ask;
                }
            };
            let mut asked = asked.borrow_mut();
            // Those taken and never answered, no one waiting, are
            // forgotten.
            asked.1.retain(|_, reply| !reply.is_closed());
            asked.0 += 1;
            let id = asked.0;
            asked.1.insert(id, ask.reply);
            Ok::<_, Error>(DataValue(Data::List(vec![Data::Int(id as i64), ask.what])))
        }
    });
    vm.register_fn_vm(&format!("%{name}-reply"), move |vm: &mut Vm, id: i64, ok: bool, value: Value| -> Result<bool, Error> {
        let Some(reply) = taken.borrow_mut().1.remove(&(id as u64)) else { return Ok(false) };
        let answer = match (ok, vm.to_data(&value, crate::REQUEST_BYTES)) {
            (true, Ok(value)) => Ok(value),
            (true, Err(e)) => Err(format!("the answer is no data: {}", e.msg)),
            (false, Ok(Data::String(why))) => Err(why),
            (false, _) => Err("it failed".into()),
        };
        Ok(reply.send(answer).is_ok())
    });
    Asker { tx }
}
