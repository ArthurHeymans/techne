//! Scopes (PLAN.md, Stage 1, step 8.4): what code running in a scope
//! creates or registers (tasks, channels, processes, registry entries,
//! whatever is handed to `scope-own!` with a cleanup) is held in a table
//! in Rust, with the scope's children.
//!
//! Shutting a scope shuts its children, then releases what it owns,
//! newest first. A cleanup that is a native (`task-cancel`,
//! `channel-close`, `process-kill`) is called here, without running
//! Lisp; the Lisp cleanups left are handed back, for the prelude to run
//! as an execution of their own with a budget, so one that loops cannot
//! hold up whoever shut the scope.
//!
//! The Lisp object of a scope is a record (`SCOPE_FIELDS`) whose slot is
//! `#f` once the scope is shut: a stale record never reaches the slot's
//! next scope.

use crate::{
    api::Root,
    builtins::type_error,
    heap::{Kind, field, is_kind, set_field},
    value::Value,
    vm::{Error, SpecialObj, Vm},
};

/// The fields of a scope's record. The prelude keeps a package
/// generation's held-back registrations in `pending`, and the scope it
/// succeeds in `predecessor`.
pub(crate) const SCOPE_FIELDS: &[&str] = &["slot", "name", "parent", "pending", "predecessor"];
const SLOT: usize = 1;
const NAME: usize = 2;
const PARENT: usize = 3;
const PENDING: usize = 4;
const PREDECESSOR: usize = 5;

/// A live scope. The collector traces its values in place (`visit`), as
/// it does tasks': a scope may own thousands of registrations, each
/// rooted apart would cost every collection.
pub(crate) struct Scope {
    record: Value,
    parent: Option<usize>,
    /// Newest first.
    children: Vec<usize>,
    /// Oldest first.
    owned: Vec<Owned>,
    added: u32,
}

/// Something a scope owns, with its cleanup and the test whether it ended
/// by itself (then the cleanup is not called).
#[derive(Clone, Copy)]
struct Owned {
    value: Value,
    cleanup: Value,
    done: Value,
}

impl Scope {
    /// A copy for a world started from a template holding the scope
    /// (`Vm::instantiate`), its values passed through `copy`.
    pub(crate) fn copy_for_world(&self, copy: &mut dyn FnMut(Value) -> Value) -> Scope {
        Scope {
            record: copy(self.record),
            parent: self.parent,
            children: self.children.clone(),
            owned: self.owned.iter().map(|o| Owned { value: copy(o.value), cleanup: copy(o.cleanup), done: copy(o.done) }).collect(),
            added: self.added,
        }
    }

    /// Its values, for the collector.
    pub(crate) fn visit(&mut self, f: &mut dyn FnMut(&mut Value)) {
        f(&mut self.record);
        for o in &mut self.owned {
            f(&mut o.value);
            f(&mut o.cleanup);
            f(&mut o.done);
        }
    }
}

/// An entry taken out of its scope, rooted while natives run.
struct Taken {
    value: Root,
    cleanup: Root,
    done: Root,
}

/// A long-lived scope forgets what ended by itself (a finished task, a
/// closed channel) every so many additions once it owns this many.
const PRUNE: usize = 64;

impl Vm {
    fn scope_slot(&self, v: Value, who: &str) -> Result<Option<usize>, Error> {
        if !(is_kind(v, Kind::Record) && unsafe { field(v.as_ptr(), 0) } == self.special(SpecialObj::ScopeRtd)) {
            return Err(type_error(who, "scope", v));
        }
        let slot = unsafe { field(v.as_ptr(), SLOT) };
        Ok(slot.is_int().then(|| slot.as_int() as usize))
    }

    fn live_scope(&self, v: Value, who: &str) -> Result<usize, Error> {
        self.scope_slot(v, who)?.ok_or_else(|| {
            let name = crate::builtins::repr(unsafe { field(v.as_ptr(), NAME) });
            Error::new(format!("{who}: the scope {name} is shut down"))
        })
    }

    fn scope(&self, slot: usize) -> &Scope {
        self.scopes[slot].as_ref().expect("a live scope")
    }

    fn scope_mut(&mut self, slot: usize) -> &mut Scope {
        self.scopes[slot].as_mut().expect("a live scope")
    }

    fn make_scope(&mut self, name: Value, parent: Value) -> Result<Value, Error> {
        // Only the root scope has none: nothing would shut another.
        let parent_slot = match parent {
            Value::FALSE if self.scopes.is_empty() => None,
            Value::FALSE => return Err(Error::new("make-scope: a scope needs a parent scope")),
            _ => Some(self.live_scope(parent, "make-scope")?),
        };
        let slot = self.free_scopes.pop().unwrap_or(self.scopes.len());
        let rtd = self.special(SpecialObj::ScopeRtd);
        let record = self.make_record(rtd, &[Value::int_unchecked(slot as i64), name, parent, Value::FALSE, Value::FALSE]);
        let scope = Scope { record, parent: parent_slot, children: Vec::new(), owned: Vec::new(), added: 0 };
        if slot == self.scopes.len() {
            self.scopes.push(Some(scope));
        } else {
            self.scopes[slot] = Some(scope);
        }
        if let Some(p) = parent_slot {
            self.scope_mut(p).children.insert(0, slot);
        }
        Ok(record)
    }

    fn scope_own(&mut self, slot: usize, value: Value, cleanup: Value, done: Value) -> Result<(), Error> {
        let scope = self.scope_mut(slot);
        scope.owned.push(Owned { value, cleanup, done });
        scope.added = scope.added.wrapping_add(1);
        if scope.owned.len() > PRUNE && scope.added.is_multiple_of(PRUNE as u32) {
            // Each entry read anew: a test may collect, which moves them.
            let ended: Vec<bool> = (0..self.scope(slot).owned.len())
                .map(|i| {
                    let o = self.scope(slot).owned[i];
                    self.ended(o.value, o.done)
                })
                .collect();
            let mut ended = ended.into_iter();
            self.scope_mut(slot).owned.retain(|_| !ended.next().unwrap_or(false));
        }
        Ok(())
    }

    /// Whether `value` ended by itself, as far as Rust can tell: its test
    /// `done` is a native that says so.
    fn ended(&mut self, value: Value, done: Value) -> bool {
        done.is_native() && self.call_native_value(done, &[value]).is_ok_and(|v| v.is_truthy())
    }

    /// Call the native `f` with `args` (from a native: nothing in Scheme
    /// runs).
    fn call_native_value(&mut self, f: Value, args: &[Value]) -> Result<Value, Error> {
        let saved = self.stack_top;
        self.ensure_regs(saved + args.len() + 1)?;
        self.regs[saved..saved + args.len()].copy_from_slice(args);
        self.stack_top = saved + args.len();
        let result = self.call_native(f.as_native(), saved, args.len());
        self.stack_top = saved;
        match result {
            Err(e) if e.wait.is_some() => Err(Error::new("a scope's cleanup cannot wait")),
            other => other,
        }
    }

    /// Shut scope `slot` and its children: free their slots and release
    /// what they own, newest first, calling the cleanups that are natives.
    /// Into `released` go the values owned, into `lisp` the entries whose
    /// cleanups are Lisp's to run; the first native cleanup to fail is
    /// returned.
    fn shut_scope(&mut self, slot: usize, released: &mut Vec<Root>, lisp: &mut Vec<Taken>) -> Option<Error> {
        let scope = self.scopes[slot].take()?;
        self.free_scopes.push(slot);
        unsafe { set_field(scope.record.as_ptr(), SLOT, Value::FALSE) };
        // Out of the table, which the collector traces: rooted.
        let owned: Vec<Taken> = scope
            .owned
            .iter()
            .map(|o| Taken { value: self.root(o.value), cleanup: self.root(o.cleanup), done: self.root(o.done) })
            .collect();
        if let Some(p) = scope.parent
            && let Some(parent) = self.scopes[p].as_mut()
        {
            parent.children.retain(|&c| c != slot);
        }
        let mut failure = None;
        for child in scope.children {
            failure = worse(failure, self.shut_scope(child, released, lisp));
        }
        for o in owned.into_iter().rev() {
            released.push(o.value.clone());
            if self.ended(o.value.get(), o.done.get()) {
                continue;
            }
            let cleanup = o.cleanup.get();
            if cleanup.is_native() {
                if let Err(e) = self.call_native_value(cleanup, &[o.value.get()]) {
                    failure = worse(failure, Some(e));
                }
            } else {
                lisp.push(o);
            }
        }
        failure
    }
}

/// The failure to report of `first` and `later`: the first, unless only
/// the later one reaches the host (an exit, a kill), which must go on.
fn worse(first: Option<Error>, later: Option<Error>) -> Option<Error> {
    match (first, later) {
        (Some(f), Some(l)) if !f.reaches_host() && l.reaches_host() => Some(l),
        (first, later) => first.or(later),
    }
}

fn arg(vm: &Vm, args: usize, i: usize) -> Value {
    vm.regs[args + i]
}

/// Field `i` of the scope argument `0`.
fn scope_field(vm: &Vm, args: usize, i: usize, who: &str) -> Result<Value, Error> {
    let s = arg(vm, args, 0);
    vm.scope_slot(s, who)?;
    Ok(unsafe { field(s.as_ptr(), i) })
}

fn set_scope_field(vm: &mut Vm, args: usize, i: usize, who: &str) -> Result<Value, Error> {
    let (s, v) = (arg(vm, args, 0), arg(vm, args, 1));
    vm.scope_slot(s, who)?;
    unsafe { set_field(s.as_ptr(), i, v) };
    vm.write_barrier(s.as_ptr(), v);
    Ok(Value::VOID)
}

/// The values of `items`, oldest first, as a list.
fn list_of(vm: &mut Vm, items: Vec<Value>) -> Value {
    vm.make_list(&items)
}

fn scope_shutdown(vm: &mut Vm, args: usize, _: usize) -> Result<Value, Error> {
    let Some(slot) = vm.scope_slot(arg(vm, args, 0), "%scope-shutdown!")? else { return Ok(Value::FALSE) };
    let (mut released, mut lisp) = (Vec::new(), Vec::new());
    let failure = vm.shut_scope(slot, &mut released, &mut lisp);
    // An exit or a kill goes on to the host as itself (the table is
    // consistent; the Lisp cleanups left are not run, nor would they be).
    if failure.as_ref().is_some_and(Error::reaches_host) {
        return Err(failure.expect("a failure"));
    }
    let failure = match failure {
        Some(e) => {
            let message = e.msg.clone();
            match &e.payload {
                Some(p) => p.get(),
                None => vm.make_error_object_of(&message, &[], e.kind),
            }
        }
        None => Value::FALSE,
    };
    let failure = vm.root(failure);
    let entries: Vec<Root> = lisp
        .into_iter()
        .map(|o| {
            let rest = vm.alloc_pair(o.cleanup.get(), o.done.get());
            let entry = vm.alloc_pair(o.value.get(), rest);
            vm.root(entry)
        })
        .collect();
    let entries = list_of(vm, entries.iter().map(Root::get).collect());
    let entries = vm.root(entries);
    let released = list_of(vm, released.iter().map(Root::get).collect());
    let released = vm.root(released);
    Ok(vm.make_list(&[failure.get(), released.get(), entries.get()]))
}

pub fn install(vm: &mut Vm) {
    crate::natives! { vm;
        "(%make-scope name parent)" => |vm: &mut Vm, a, _| vm.make_scope(arg(vm, a, 0), arg(vm, a, 1));
        /// Return #t if OBJ is a scope.
        "(scope? obj)" => |vm: &mut Vm, a, _| {
            let v = arg(vm, a, 0);
            Ok(Value::bool(is_kind(v, Kind::Record) && unsafe { field(v.as_ptr(), 0) } == vm.special(SpecialObj::ScopeRtd)))
        };
        /// Return #t if the scope S is not shut down.
        "(scope-live? s)" => |vm: &mut Vm, a, _| Ok(Value::bool(vm.scope_slot(arg(vm, a, 0), "scope-live?")?.is_some()));
        /// Return the name the scope S was made with.
        "(scope-name s)" => |vm: &mut Vm, a, _| scope_field(vm, a, NAME, "scope-name");
        /// Return the scope that owns the scope S, or #f for the root scope.
        "(scope-parent s)" => |vm: &mut Vm, a, _| scope_field(vm, a, PARENT, "scope-parent");
        /// Return the scopes S owns, newest first.
        "(scope-children s)" => |vm: &mut Vm, a, _| {
            let Some(slot) = vm.scope_slot(arg(vm, a, 0), "scope-children")? else { return Ok(Value::NIL) };
            let children: Vec<Value> = vm.scope(slot).children.iter().map(|&c| vm.scope(c).record).collect();
            Ok(list_of(vm, children))
        };
        /// Return what the scope S owns, oldest first.
        "(scope-resources s)" => |vm: &mut Vm, a, _| {
            let Some(slot) = vm.scope_slot(arg(vm, a, 0), "scope-resources")? else { return Ok(Value::NIL) };
            let owned: Vec<Value> = vm.scope(slot).owned.iter().map(|o| o.value).collect();
            Ok(list_of(vm, owned))
        };
        "(%scope-pending s)" => |vm: &mut Vm, a, _| scope_field(vm, a, PENDING, "%scope-pending");
        "(%set-scope-pending! s v)" => |vm: &mut Vm, a, _| set_scope_field(vm, a, PENDING, "%set-scope-pending!");
        "(%scope-predecessor s)" => |vm: &mut Vm, a, _| scope_field(vm, a, PREDECESSOR, "%scope-predecessor");
        "(%set-scope-predecessor! s v)" => |vm: &mut Vm, a, _| set_scope_field(vm, a, PREDECESSOR, "%set-scope-predecessor!");
        "(%scope-own! s resource cleanup done?)" => |vm: &mut Vm, a, _| {
            let slot = vm.live_scope(arg(vm, a, 0), "scope-own!")?;
            vm.scope_own(slot, arg(vm, a, 1), arg(vm, a, 2), arg(vm, a, 3))?;
            Ok(arg(vm, a, 1))
        };
        "(%scope-disown! s resource)" => |vm: &mut Vm, a, _| {
            let Some(slot) = vm.scope_slot(arg(vm, a, 0), "scope-disown!")? else { return Ok(Value::FALSE) };
            let r = arg(vm, a, 1);
            let Some(i) = vm.scope(slot).owned.iter().position(|o| o.value == r) else { return Ok(Value::FALSE) };
            let o = vm.scope_mut(slot).owned.remove(i);
            // Rooted in the registers: allocating may collect.
            vm.regs[a] = o.cleanup;
            vm.regs[a + 1] = o.done;
            Ok(vm.alloc_pair(arg(vm, a, 0), arg(vm, a, 1)))
        };
        "(%scope-shutdown! s)" => scope_shutdown;
    }
}
