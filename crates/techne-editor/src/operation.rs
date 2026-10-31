//! Operations (doc/operations.md): one run of a command, with the work it
//! holds, until that settles. Which runs are operations, what holds one
//! and how it ends is Lisp's (`lisp/editor/operations.scm`); this is what
//! the natives that commit need of one, read from the running task's
//! `current-operation`, so that no caller passes it: its id, which every
//! transaction made in it records, and whether it may still commit. While
//! an operation is ending (failed or cancelled, the work it holds being
//! cancelled), nothing commits in it, so that work cannot land after it
//! failed. Once it has ended, work it started without holding it (a
//! language server's session, a service) goes on, and what that commits
//! is made in no operation.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

use techne_text::OperationId;
use techne_vm::{
    api::Foreign,
    vm::{Error, Vm},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Open,
    Ending,
    Ended,
}

pub struct Operation {
    id: OperationId,
    parent: Option<u64>,
    command: String,
    phase: Cell<Phase>,
    committed: Cell<bool>,
    outcome: RefCell<Option<String>>,
}

/// This runtime's id: operations are numbered within it, so together they
/// name one in a journal read back by another.
pub fn runtime_id() -> u64 {
    static ID: OnceLock<u64> = OnceLock::new();
    *ID.get_or_init(|| {
        use std::hash::{BuildHasher, Hasher};
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u32(std::process::id());
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
        // Fits a fixnum (47 bits), so Lisp sees it as an immediate number.
        h.finish() >> 17
    })
}

impl Operation {
    fn new(command: String, parent: Option<&Operation>) -> Operation {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Operation {
            id: OperationId { runtime: runtime_id(), serial: NEXT.fetch_add(1, Ordering::Relaxed) },
            parent: parent.map(|p| p.id.serial),
            command,
            phase: Cell::new(Phase::Open),
            committed: Cell::new(false),
            outcome: RefCell::new(None),
        }
    }

    pub fn id(&self) -> OperationId {
        self.id
    }

    /// Whether a change may be made in it now, or why not: it is ending.
    pub(crate) fn check_open(&self, who: &str) -> Result<(), String> {
        match self.phase.get() {
            Phase::Ending => Err(format!("{who}: refused: its operation ({} {}) is ending", self.command, self.id.serial)),
            _ => Ok(()),
        }
    }

    /// Whether it has ended: what is made in it after that is made in none.
    pub(crate) fn ended(&self) -> bool {
        self.phase.get() == Phase::Ended
    }

    /// Record that a change was made in it.
    pub(crate) fn committed(&self) {
        self.committed.set(true);
    }
}

/// The key of the `current-operation` parameter, set by the Lisp that
/// makes it (`%set-operation-key!`).
#[derive(Clone, Default)]
pub(crate) struct Current(Rc<Cell<Option<i64>>>);

impl Current {
    /// The running task's current operation, if any.
    pub(crate) fn get(&self, vm: &Vm) -> Option<Rc<Operation>> {
        vm.task_local_foreign::<Operation>(self.0.get()?)
    }

    /// The operation a native about to commit commits in, or why it may
    /// not: its operation is ending. `who` names the native.
    pub(crate) fn committing(&self, vm: &Vm, who: &str) -> Result<Option<Rc<Operation>>, String> {
        let op = self.get(vm).filter(|op| op.phase.get() != Phase::Ended);
        if let Some(op) = &op {
            op.check_open(who)?;
        }
        Ok(op)
    }

    /// Commit with `commit` in the current operation, recording that it
    /// committed when it says it changed something: also when it failed
    /// part way (a lens's edit of several documents), having changed some.
    pub(crate) fn commit<T>(
        &self,
        vm: &Vm,
        who: &str,
        commit: impl FnOnce(Option<OperationId>) -> (Result<T, String>, bool),
    ) -> Result<T, String> {
        let op = self.committing(vm, who)?;
        let (done, changed) = commit(op.as_ref().map(|o| o.id));
        if let Some(op) = op
            && changed
        {
            op.committed();
        }
        done
    }
}

type Op = Foreign<Operation>;

pub(crate) fn install(vm: &mut Vm, current: Current) {
    vm.name_foreign_type::<Operation>("operation");
    techne_vm::procedures! { vm;
        /// Make KEY, a parameter's key, the key of the current operation,
        /// which the natives that commit read.
        "(%set-operation-key! key)" => move |key: i64| current.0.set(Some(key));
    }
    techne_vm::procedures! { vm;
        /// Return a new operation of COMMAND (a symbol's name), a child of
        /// PARENT, an operation or #f. It is open: changes may be made in it.
        "(%make-operation command parent)" => |command: String, parent: Option<Op>| {
            Foreign::new(Operation::new(command, parent.as_deref()))
        };
    }
    techne_vm::procedures! { vm;
        /// Return the number of OPERATION, never reused in this runtime.
        "(operation-serial operation)" => |op: Op| op.id.serial as i64;
    }
    techne_vm::procedures! { vm;
        /// Return the number of this runtime, which with an operation's
        /// serial names it in a document's history after a restart.
        "(operation-runtime)" => || runtime_id() as i64;
    }
    techne_vm::procedures! { vm;
        /// Return the serial of OPERATION's parent, or #f if it has none.
        "(operation-parent-serial operation)" => |op: Op| op.parent.map(|p| p as i64);
    }
    techne_vm::procedures! { vm;
        /// Return the name of the command OPERATION is a run of.
        "(%operation-command operation)" => |op: Op| op.command.clone();
    }
    techne_vm::procedures! { vm;
        /// Return #t if a change was made in OPERATION.
        "(operation-committed? operation)" => |op: Op| op.committed.get();
    }
    techne_vm::procedures! { vm;
        /// Return whether OPERATION is \"open\", \"ending\" (failed or
        /// cancelled, waiting for its work to end) or \"ended\".
        "(%operation-phase operation)" => |op: Op| match op.phase.get() {
            Phase::Open => "open",
            Phase::Ending => "ending",
            Phase::Ended => "ended",
        };
    }
    techne_vm::procedures! { vm;
        /// Make OPERATION ending: nothing commits in it any more.
        "(%operation-ending! operation)" => |op: Op| {
            if op.phase.get() == Phase::Open {
                op.phase.set(Phase::Ending);
            }
        };
    }
    techne_vm::procedures! { vm;
        /// End OPERATION with OUTCOME, the name of one.
        "(%operation-end! operation outcome)" => |op: Op, outcome: String| {
            op.phase.set(Phase::Ended);
            *op.outcome.borrow_mut() = Some(outcome);
        };
    }
    techne_vm::procedures! { vm;
        /// Return the name of OPERATION's outcome, or #f while it has not
        /// ended.
        "(%operation-outcome operation)" => |op: Op| op.outcome.borrow().clone();
    }
}

/// A transaction's operation, for Lisp: (runtime serial), or #f.
pub(crate) fn id_value(vm: &mut Vm, id: Option<OperationId>) -> Result<techne_vm::api::Value, Error> {
    use techne_vm::api::IntoValue;
    match id {
        None => false.into_value(vm),
        Some(id) => {
            let parts = [(id.runtime as i64).into_value(vm)?, (id.serial as i64).into_value(vm)?];
            vm.make_list(&parts)
        }
    }
}
