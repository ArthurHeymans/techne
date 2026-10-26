//! Documents granted to services, and the changes they propose (PLAN.md,
//! Stage 1, step 8.6). A service never holds a document: it is given
//! snapshots, and through a grant, a number, may propose changes of the
//! text, which the editor's world keeps until someone accepts or rejects
//! them. The authority is this table's, in the editor's world: a grant
//! means something only from the service it was made to, and only to the
//! world that service ran when it was made, so a service started again
//! (after a panic) holds none; grant numbers are never reused.
//!
//! A proposal is a transaction against the revision the service names
//! (its snapshot's, presumably: which snapshots a request carried is not
//! checked yet), one still in the history. It is accepted as an edit by
//! the grant's actor, moved past what changed
//! since (`Document::rebase`): refused, naming who, when that touched the
//! same text. So the journal says who changed what, and undo is the
//! actor's, as for any edit. Accepted with the revision a preview showed
//! (`#:seen`), it is refused if the text changed since. A revoked grant's
//! proposals stay, to read: accepting one takes adopting it.

use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap, VecDeque},
    rc::{Rc, Weak},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use techne_text::{ChangeSet, Document, Group, Revision, Transaction, document::Actor};
use techne_vm::{
    api::{Arg, Foreign, FromValue, IntoValue, Value},
    vm::{Error, Vm},
};

use crate::service::{Service, Snapshot};

/// What a grant allows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Right {
    /// Snapshots of the text.
    Read,
    /// Snapshots, and proposing changes.
    Stage,
}

struct Grant {
    doc: Weak<RefCell<Document>>,
    /// The service it was made to (`Service::id`).
    service: u64,
    /// That service's generation as it goes on (`Service::generation`),
    /// and what it was when the grant was made: it holds while the two
    /// are the same.
    world: Arc<AtomicU64>,
    generation: u64,
    right: Right,
    actor: Actor,
    revoked: bool,
}

impl Grant {
    /// Whether it holds: not revoked, and its service runs the world it
    /// was made to.
    fn holds(&self) -> bool {
        !self.revoked && self.world.load(Ordering::SeqCst) == self.generation
    }
}

/// A proposal waiting to be decided.
struct Proposal {
    grant: u64,
    doc: Weak<RefCell<Document>>,
    tx: Transaction,
    note: String,
}

/// The proposals a grant keeps waiting at most.
const PENDING: usize = 256;

/// The decided proposals whose state is kept, the latest.
const DECIDED: usize = 1024;

#[derive(Default)]
pub(crate) struct Access {
    next: u64,
    grants: HashMap<u64, Grant>,
    /// Those waiting; decided, a proposal goes, but for its state in
    /// `decided`, until later ones push it out.
    proposals: BTreeMap<u64, Proposal>,
    decided: VecDeque<(u64, &'static str)>,
}

pub(crate) type Shared = Rc<RefCell<Access>>;

impl Access {
    fn fresh(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    /// Grant `id` as `service`, the one it was made to, holds it now:
    /// neither revoked nor made to an earlier world of it.
    fn held(&self, id: u64, service: &Service) -> Result<&Grant, String> {
        let g = self.grants.get(&id).filter(|g| g.service == service.id()).ok_or("no such grant")?;
        if g.holds() { Ok(g) } else { Err("the grant is revoked".into()) }
    }

    /// How many grants `service` holds.
    pub(crate) fn grants_of(&self, service: &Service) -> usize {
        self.grants.values().filter(|g| g.service == service.id() && g.holds()).count()
    }

    /// Whether grant `id` holds.
    fn holds(&self, id: u64) -> bool {
        self.grants.get(&id).is_some_and(Grant::holds)
    }

    /// The snapshot of the text of grant `id`, for `service` to read.
    pub(crate) fn snapshot(&self, id: u64, service: &Service) -> Result<Snapshot, String> {
        let g = self.held(id, service)?;
        let doc = g.doc.upgrade().ok_or("the document is gone")?;
        let doc = doc.borrow();
        Ok(Snapshot { id: doc.id(), revision: doc.revision(), text: doc.text().clone(), grant: Some(id) })
    }

    /// File what `service` proposes through grant `id`: `edits` of the
    /// text at revision `base`.
    fn file(&mut self, service: &Service, id: u64, base: Revision, edits: Vec<crate::EditArg>, note: String) -> Result<u64, String> {
        let g = self.held(id, service)?;
        if g.right != Right::Stage {
            return Err("the grant allows reading, not proposing".into());
        }
        let pending = self.proposals.values().filter(|p| p.grant == id).count();
        if pending >= PENDING {
            return Err(format!("{PENDING} proposals of this grant wait already"));
        }
        let doc = g.doc.upgrade().ok_or("the document is gone")?;
        // Its length at `base`, which the edits are of.
        let len = {
            let doc = doc.borrow();
            if base > doc.revision() {
                return Err(format!("revision {base} is not the document's yet"));
            }
            let later = doc.entries_since(base).ok_or("the revision proposed against is no longer in the history")?;
            later.first().map_or(doc.len(), |e| e.changes.len_before())
        };
        let changes = ChangeSet::from_edits(len, crate::edit_args(edits)).map_err(|e| e.to_string())?;
        let tx = Transaction { base, actor: g.actor.clone(), changes, group: Group::New };
        let p = Proposal { grant: id, doc: Rc::downgrade(&doc), tx, note };
        let pid = self.fresh();
        // Those of documents gone are decided by nobody.
        self.proposals.retain(|_, p| p.doc.strong_count() > 0);
        self.proposals.insert(pid, p);
        Ok(pid)
    }

    fn proposal(&self, id: u64) -> Result<&Proposal, String> {
        match self.proposals.get(&id) {
            Some(p) => Ok(p),
            None if self.decided.iter().any(|&(d, _)| d == id) => Err("the proposal was decided".into()),
            None => Err("no such proposal".into()),
        }
    }

    /// Proposal `id` decided, `how`: it goes, its state kept a while.
    fn decide(&mut self, id: u64, how: &'static str) {
        self.proposals.remove(&id);
        if self.decided.len() == DECIDED {
            self.decided.pop_front();
        }
        self.decided.push_back((id, how));
    }

    /// Proposal `id` moved past what changed since: the document's head,
    /// and the transaction against it.
    fn rebased(&self, id: u64) -> Result<(Rc<RefCell<Document>>, Transaction), String> {
        let p = self.proposal(id)?;
        let doc = p.doc.upgrade().ok_or("the document is gone")?;
        let tx = doc.borrow().rebase(p.tx.clone()).map_err(|e| format!("Refused: {e} since"))?;
        Ok((doc, tx))
    }
}

/// A text or a grant, as `service-call` is given them.
pub(crate) enum Given {
    Text(crate::TextArg),
    Grant(u64),
}

impl FromValue for Given {
    fn from_arg(v: Arg<'_>) -> Result<Self, Error> {
        match usize::from_arg(v) {
            Ok(id) => Ok(Given::Grant(id as u64)),
            Err(_) => crate::TextArg::from_arg(v).map(Given::Text),
        }
    }
}

const PROCEDURES: &str = r#"
(define (document-grant document right #:to service #:actor [actor #f])
  "Grant SERVICE DOCUMENT with RIGHT; return the grant, a number.
RIGHT is read, to have snapshots of its text, or stage, to propose
changes too, which wait for someone to accept them as edits by ACTOR
(service: and the service's name if not given). Pass the grant to
`service-call` where a document would go: the service's snapshot names
it (`snapshot-grant`). It holds while the service's world does, until
`grant-revoke!`."
  (%document-grant document (symbol->string right) service (or actor (string-append "service:" (service-name service)))))

(define (proposal-accept! proposal #:seen [seen #f] #:adopt [adopt #f])
  "Make PROPOSAL's changes as one edit; return the new revision.
They are moved past what changed since, and made by the grant's actor:
refused, saying by whom, if that touched the same text, and refused if
the text changed since revision SEEN, given (what `proposal-preview`
showed). A revoked grant's proposal is accepted only to ADOPT it, on
this world's authority."
  (%proposal-accept! proposal seen adopt))
"#;

pub(crate) fn install(vm: &mut Vm, access: Shared) {
    let a = access.clone();
    techne_vm::procedures! { vm;
        /// A grant of DOCUMENT to SERVICE, RIGHT read or stage, its
        /// changes by ACTOR (`document-grant`).
        "(%document-grant document right service actor)" => move |d: Foreign<RefCell<Document>>, right: String, service: Foreign<Service>, actor: String| -> Result<i64, String> {
            let right = match right.as_str() {
                "read" => Right::Read,
                "stage" => Right::Stage,
                r => return Err(format!("document-grant: the right is read or stage, not {r}")),
            };
            let mut access = a.borrow_mut();
            let id = access.fresh();
            let world = service.0.generation();
            let generation = world.load(Ordering::SeqCst);
            let grant = Grant { doc: Rc::downgrade(&d.0), service: service.0.id(), world, generation, right, actor: actor.into(), revoked: false };
            access.grants.insert(id, grant);
            Ok(id as i64)
        };
    }
    let a = access.clone();
    techne_vm::procedures! { vm;
        /// Revoke GRANT: its service has no more snapshots through it, nor
        /// proposes through it, and what it proposed is accepted only to
        /// adopt it.
        "(grant-revoke! grant)" => move |id: usize| -> Result<(), String> {
            a.borrow_mut().grants.get_mut(&(id as u64)).map(|g| g.revoked = true).ok_or_else(|| "grant-revoke!: no such grant".to_string())
        };
    }
    let a = access.clone();
    techne_vm::procedures! { vm;
        /// File what SERVICE proposes through GRANT, held by it: EDITS, each
        /// (from to text), of the text at revision BASE, with a NOTE saying
        /// what for. Return the proposal, a number.
        "(proposal-file! service grant base edits note)" => move |service: Foreign<Service>, grant: usize, base: usize, edits: Vec<crate::EditArg>, note: String| -> Result<i64, String> {
            a.borrow_mut().file(&service.0, grant as u64, base as Revision, edits, note).map(|p| p as i64).map_err(|e| format!("proposal-file!: {e}"))
        };
    }
    let a = access.clone();
    techne_vm::procedures! { vm;
        /// Return the proposals for DOCUMENT waiting to be decided, oldest
        /// first.
        "(proposals document)" => move |d: Foreign<RefCell<Document>>| -> Vec<i64> {
            let id = d.0.borrow().id();
            let access = a.borrow();
            access.proposals.iter().filter(|(_, p)| p.doc.upgrade().is_some_and(|d| d.borrow().id() == id)).map(|(&k, _)| k as i64).collect()
        };
    }
    let a = access.clone();
    techne_vm::procedures! { vm;
        /// Return what PROPOSAL is: pending, revoked (its grant was, or its
        /// service's world ended), applied or rejected; decided long ago,
        /// it is no longer known.
        "(proposal-state proposal)" => move |id: usize| -> Result<&'static str, String> {
            let access = a.borrow();
            let id = id as u64;
            if let Some(&(_, how)) = access.decided.iter().find(|&&(d, _)| d == id) {
                return Ok(how);
            }
            let p = access.proposal(id)?;
            Ok(if access.holds(p.grant) { "pending" } else { "revoked" })
        };
    }
    let a = access.clone();
    techne_vm::procedures! { vm;
        /// Return PROPOSAL's note.
        "(proposal-note proposal)" => move |id: usize| -> Result<String, String> { Ok(a.borrow().proposal(id as u64)?.note.clone()) };
    }
    let a = access.clone();
    techne_vm::procedures! { vm;
        /// Return PROPOSAL's changes as they would be made now, as
        /// (revision ((from to text) ...)): the revision they are of, and
        /// each change of the text at it. Refused, saying by whom, if what
        /// changed since touched the same text.
        #[vm]
        "(proposal-preview proposal)" => move |vm: &mut Vm, id: usize| -> Result<Value, Error> {
            let (_, tx) = a.borrow().rebased(id as u64).map_err(Error::new)?;
            let edits = tx
                .changes
                .edits()
                .map(|(r, t)| {
                    let parts = [r.start.into_value(vm)?, r.end.into_value(vm)?, t.into_value(vm)?];
                    vm.make_list(&parts)
                })
                .collect::<Result<Vec<_>, Error>>()?;
            let edits = vm.make_list(&edits)?;
            let base = (tx.base as usize).into_value(vm)?;
            vm.make_list(&[base, edits])
        };
    }
    let a = access.clone();
    techne_vm::procedures! { vm;
        /// Accept PROPOSAL (`proposal-accept!`).
        "(%proposal-accept! proposal seen adopt)" => move |id: usize, seen: Option<usize>, adopt: bool| -> Result<i64, String> {
            let id = id as u64;
            let (doc, tx) = {
                let access = a.borrow();
                let p = access.proposal(id)?;
                if !adopt && !access.holds(p.grant) {
                    return Err("proposal-accept!: the grant is revoked; adopt the proposal to accept it".into());
                }
                access.rebased(id)?
            };
            if seen.is_some_and(|seen| seen as Revision != doc.borrow().revision()) {
                return Err("proposal-accept!: the text changed since the preview".into());
            }
            let rev = doc.borrow_mut().apply(tx).map_err(|e| format!("proposal-accept!: {e}"))?;
            a.borrow_mut().decide(id, "applied");
            Ok(rev as i64)
        };
    }
    let a = access.clone();
    techne_vm::procedures! { vm;
        /// Reject PROPOSAL: it is not made, nor offered any more.
        "(proposal-reject! proposal)" => move |id: usize| -> Result<(), String> {
            let mut access = a.borrow_mut();
            access.proposal(id as u64).map_err(|e| format!("proposal-reject!: {e}"))?;
            access.decide(id as u64, "rejected");
            Ok(())
        };
    }
    vm.eval_in(techne_vm::vm::ROOT_MODULE, "<techne-editor access>", PROCEDURES).expect("the access procedures");
}
