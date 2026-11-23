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
//! hold up whoever shut the scope. Every cleanup (native or Lisp) and
//! every `done?` predicate runs under the authority captured when the
//! resource was registered, never the authority of whoever shuts the
//! scope down.
//!
//! The Lisp object of a scope is a record (`SCOPE_FIELDS`) whose slot is
//! `#f` once the scope is shut or adopted: a stale record never reaches
//! the slot's next scope. A successfully adopted scope records its target
//! in `adopted` so `scope-procedure` closures captured during its
//! initialization remain live under the adopting owner.

use crate::{
    api::Root,
    builtins::type_error,
    heap::{Kind, field, header, is_kind, kind_of, len_of, set_field},
    value::Value,
    vm::{Error, SpecialObj, Vm},
};

/// Task-local keys of `current-scope`, `%staging-scope`, `authority-scope`,
/// and protected root-only parameters (`<= PROTECTED_PARAM_KEY_BASE`).
pub(crate) const CURRENT_SCOPE_KEY: i64 = -3;
pub(crate) const STAGING_SCOPE_KEY: i64 = -4;
pub(crate) const AUTHORITY_SCOPE_KEY: i64 = -5;
pub(crate) const PROTECTED_PARAM_KEY_BASE: i64 = -100;

/// The fields of a scope's record. The prelude keeps a package
/// generation's held-back registrations in `pending`, the scope it
/// succeeds in `predecessor`, and the scope that adopted it on
/// successful world-load in `adopted`.
pub(crate) const SCOPE_FIELDS: &[&str] = &["slot", "name", "parent", "pending", "predecessor", "adopted"];
const SLOT: usize = 1;
const NAME: usize = 2;
const PARENT: usize = 3;
const PENDING: usize = 4;
const PREDECESSOR: usize = 5;
const ADOPTED: usize = 6;

/// Fields of an unforgeable `ScopeCap` record (`SpecialObj::ScopeCapRtd`).
pub(crate) const SCOPE_CAP_FIELDS: &[&str] = &["current", "authority"];
const CAP_CURRENT: usize = 1;
const CAP_AUTHORITY: usize = 2;

/// Fields of an immutable applicable `ProtectedParam` record (`SpecialObj::ProtectedParamRtd`).
pub(crate) const PROTECTED_PARAM_FIELDS: &[&str] = &["proc", "key", "default", "convert", "root-only"];
const PP_KEY: usize = 2;
const PP_DEFAULT: usize = 3;
const PP_CONVERT: usize = 4;
const PP_ROOT_ONLY: usize = 5;

/// Fields of an opaque `SavedParam` token (`SpecialObj::SavedParamRtd`).
pub(crate) const SAVED_PARAM_FIELDS: &[&str] = &["handle", "present", "value"];
const SP_HANDLE: usize = 1;
const SP_PRESENT: usize = 2;
const SP_VALUE: usize = 3;

/// Fields of an opaque one-shot cleanup action (`SpecialObj::CleanupActionRtd`).
pub(crate) const CLEANUP_ACTION_FIELDS: &[&str] = &["resource", "cleanup", "done", "cap"];
const CA_RESOURCE: usize = 1;
const CA_CLEANUP: usize = 2;
const CA_DONE: usize = 3;
const CA_CAP: usize = 4;

const MAX_CLONE_DEPTH: usize = 64;
const MAX_CLONE_NODES: usize = 8192;
const MAX_CLONE_BYTES: usize = 1024 * 1024;

#[derive(Default)]
struct CloneBudget {
    nodes: usize,
    bytes: usize,
}

impl CloneBudget {
    fn charge_nodes(&mut self, count: usize) -> Result<(), Error> {
        self.nodes = self.nodes.saturating_add(count);
        if self.nodes > MAX_CLONE_NODES {
            return Err(Error::new("protected parameter: value exceeds maximum size"));
        }
        Ok(())
    }

    fn charge_bytes(&mut self, count: usize) -> Result<(), Error> {
        self.bytes = self.bytes.saturating_add(count);
        if self.bytes > MAX_CLONE_BYTES {
            return Err(Error::new("protected parameter: value exceeds maximum byte budget"));
        }
        Ok(())
    }
}

/// A live scope. The collector traces its values in place (`visit`), as
/// it does tasks': a scope may own thousands of registrations, each
/// rooted apart would cost every collection.
pub(crate) struct Scope {
    record: Value,
    /// Cached immutable `ScopeCap(record, record)` allocated lazily on first
    /// same-scope capture, avoiding per-spawn `ScopeCap` allocations.
    self_cap: Value,
    parent: Option<usize>,
    /// Newest first.
    children: Vec<usize>,
    /// Oldest first.
    owned: Vec<Owned>,
    added: u32,
}

/// Something a scope owns, with its cleanup, its test whether it ended
/// by itself (then the cleanup is not called), and the registration
/// authority capability under which both callbacks must execute.
#[derive(Clone, Copy)]
struct Owned {
    id: u64,
    value: Value,
    cleanup: Value,
    done: Value,
    cap: Value,
}

impl Scope {
    /// A copy for a world started from a template holding the scope
    /// (`Vm::instantiate`), its values passed through `copy`.
    pub(crate) fn copy_for_world(&self, copy: &mut dyn FnMut(Value) -> Value) -> Scope {
        Scope {
            record: copy(self.record),
            self_cap: copy(self.self_cap),
            parent: self.parent,
            children: self.children.clone(),
            owned: self
                .owned
                .iter()
                .map(|o| Owned { id: o.id, value: copy(o.value), cleanup: copy(o.cleanup), done: copy(o.done), cap: copy(o.cap) })
                .collect(),
            added: self.added,
        }
    }

    /// Its values, for the collector.
    pub(crate) fn visit(&mut self, f: &mut dyn FnMut(&mut Value)) {
        f(&mut self.record);
        f(&mut self.self_cap);
        for o in &mut self.owned {
            f(&mut o.value);
            f(&mut o.cleanup);
            f(&mut o.done);
            f(&mut o.cap);
        }
    }
}

/// An entry taken out of its scope, rooted while natives run.
struct Taken {
    value: Root,
    cleanup: Root,
    done: Root,
    cap: Root,
}

/// A long-lived scope forgets what ended by itself (a finished task, a
/// closed channel) every so many additions once it owns this many.
const PRUNE: usize = 64;

impl Vm {
    #[inline]
    fn is_scope_record(&self, v: Value) -> bool {
        is_kind(v, Kind::Record) && unsafe { field(v.as_ptr(), 0) } == self.special(SpecialObj::ScopeRtd)
    }

    #[inline]
    fn root_scope_record(&self) -> Value {
        self.scopes.first().and_then(|s| s.as_ref()).map_or(Value::FALSE, |s| s.record)
    }

    fn world_scope_record(&self) -> Value {
        self.scopes.get(1).and_then(|s| s.as_ref()).map_or(Value::FALSE, |s| s.record)
    }

    #[inline]
    fn current_scope_value(&self) -> Value {
        let raw = if self.locals.is_empty() {
            self.root_scope_record()
        } else {
            self.locals.get(&CURRENT_SCOPE_KEY).map(Root::get).unwrap_or_else(|| self.root_scope_record())
        };
        if self.is_scope_record(raw) && unsafe { field(raw.as_ptr(), SLOT) }.is_int() {
            return raw;
        }
        self.resolve_adopted_scope(raw).map_or(raw, |(_, r)| r)
    }

    #[inline]
    fn authority_scope_value(&self) -> Value {
        if self.locals.is_empty() {
            return self.root_scope_record();
        }
        self.locals.get(&AUTHORITY_SCOPE_KEY).map(Root::get).unwrap_or_else(|| self.root_scope_record())
    }

    /// Return the cached `ScopeCap(s, s)` when both `current-scope` and
    /// `authority-scope` refer to the same live scope `s` whose `self_cap` has
    /// already been allocated, else `Value::FALSE`.
    #[inline(always)]
    fn current_self_cap(&self) -> Value {
        if self.locals.is_empty() {
            return self.scopes.first().and_then(Option::as_ref).map_or(Value::FALSE, |s| s.self_cap);
        }
        let auth = self.authority_scope_value();
        let cur = self.current_scope_value();
        if cur != auth || !self.is_scope_record(cur) {
            return Value::FALSE;
        }
        let slot = unsafe { field(cur.as_ptr(), SLOT) };
        if !slot.is_int() {
            return Value::FALSE;
        }
        let idx = slot.as_int() as usize;
        self.scopes.get(idx).and_then(Option::as_ref).filter(|s| s.record == cur).map_or(Value::FALSE, |s| s.self_cap)
    }

    fn scope_slot(&self, v: Value, who: &str) -> Result<Option<usize>, Error> {
        if !self.is_scope_record(v) {
            return Err(type_error(who, "scope", v));
        }
        let slot = unsafe { field(v.as_ptr(), SLOT) };
        Ok(slot.is_int().then(|| slot.as_int() as usize))
    }

    fn adopted_next(&self, v: Value) -> Option<Value> {
        if !self.is_scope_record(v) {
            return None;
        }
        let slot = unsafe { field(v.as_ptr(), SLOT) };
        if slot.is_int() {
            return None;
        }
        let next = unsafe { field(v.as_ptr(), ADOPTED) };
        self.is_scope_record(next).then_some(next)
    }

    /// Follow any adoption links from `start` until a live scope is reached.
    /// Uses tortoise-and-hare cycle detection and live record identity instead
    /// of a live-slot-table bound so chains longer than the slot table remain
    /// resolvable under slot reuse. Returns `None` if shut down or cyclic.
    fn resolve_adopted_scope(&self, start: Value) -> Option<(usize, Value)> {
        if !self.is_scope_record(start) {
            return None;
        }
        let mut slow = start;
        let mut fast = start;
        loop {
            let slot = unsafe { field(slow.as_ptr(), SLOT) };
            if slot.is_int() {
                let idx = slot.as_int() as usize;
                if self.scopes.get(idx).and_then(Option::as_ref).is_some_and(|s| s.record == slow) {
                    return Some((idx, slow));
                }
                return None;
            }
            let next = self.adopted_next(slow)?;
            slow = next;
            if let Some(f1) = self.adopted_next(fast)
                && let Some(f2) = self.adopted_next(f1)
            {
                fast = f2;
                if slow == fast {
                    return None;
                }
            } else {
                while let Some(n) = self.adopted_next(slow) {
                    slow = n;
                }
            }
        }
    }

    /// Resolve `start` across adoption links and compress traversed links
    /// directly to the final live adopter so repeated lookups stay O(1).
    fn resolve_adopted_scope_compress(&mut self, start: Value) -> Option<(usize, Value)> {
        let resolved = self.resolve_adopted_scope(start)?;
        let target = resolved.1;
        let mut cur = start;
        while cur != target {
            let Some(next) = self.adopted_next(cur) else {
                break;
            };
            if next != target {
                unsafe { set_field(cur.as_ptr(), ADOPTED, target) };
                self.write_barrier(cur.as_ptr(), target);
            }
            cur = next;
        }
        Some(resolved)
    }

    fn live_scope(&self, v: Value, who: &str) -> Result<usize, Error> {
        self.scope_slot(v, who)?.ok_or_else(|| {
            let name = crate::builtins::repr(unsafe { field(v.as_ptr(), NAME) });
            Error::new(format!("{who}: the scope {name} is shut down"))
        })
    }

    fn scope_contains(&self, ancestor: usize, descendant: usize) -> bool {
        let mut cur = Some(descendant);
        let mut steps = 0;
        let limit = self.scopes.len();
        while let Some(slot) = cur {
            if slot == ancestor {
                return true;
            }
            if steps > limit {
                return false;
            }
            steps += 1;
            cur = self.scopes.get(slot).and_then(|s| s.as_ref()).and_then(|s| s.parent);
        }
        false
    }

    fn has_authority_over(&self, target_slot: usize) -> bool {
        if self.scopes.is_empty() {
            return true;
        }
        let auth = self.authority_scope_value();
        match self.scope_slot(auth, "authority-scope") {
            // Root authority (slot 0) covers every live scope, including
            // the separate `%world-scope` container and its children.
            Ok(Some(0)) => self.scopes.get(target_slot).is_some_and(Option::is_some),
            Ok(Some(auth_slot)) => self.scope_contains(auth_slot, target_slot),
            _ => false,
        }
    }

    /// Whether the current execution may register a fresh resource in
    /// `target_slot`: either full authority over `target_slot`, or its
    /// captured authority scope was adopted into `target_slot` (transferring
    /// lifetime ownership without widening its privilege ceiling).
    fn can_own_in(&self, target_slot: usize) -> bool {
        if self.has_authority_over(target_slot) {
            return true;
        }
        matches!(self.resolve_adopted_scope(self.authority_scope_value()), Some((s, _)) if s == target_slot)
    }

    fn require_authority_over(&self, target_slot: usize, who: &str) -> Result<(), Error> {
        if self.has_authority_over(target_slot) {
            Ok(())
        } else {
            Err(Error::new(format!("{who}: target scope is outside current authority")))
        }
    }

    pub(crate) fn is_root_authority(&self) -> bool {
        if self.scopes.first().and_then(|s| s.as_ref()).is_none() {
            return false;
        }
        matches!(self.scope_slot(self.authority_scope_value(), "root-authority?"), Ok(Some(0)))
    }

    fn capture_scope_cap(&mut self) -> Value {
        let fast = self.current_self_cap();
        if fast != Value::FALSE {
            return fast;
        }
        let cur = self.current_scope_value();
        let auth = self.authority_scope_value();
        let cache_slot = if cur == auth && self.is_scope_record(cur) {
            let slot = unsafe { field(cur.as_ptr(), SLOT) };
            slot.is_int()
                .then(|| slot.as_int() as usize)
                .filter(|&idx| self.scopes.get(idx).and_then(Option::as_ref).is_some_and(|s| s.record == cur))
        } else {
            None
        };
        let rtd = self.special(SpecialObj::ScopeCapRtd);
        let cap = self.make_record(rtd, &[cur, auth]);
        if let Some(idx) = cache_slot
            && let Some(scope) = self.scopes.get_mut(idx).and_then(Option::as_mut)
        {
            scope.self_cap = cap;
        }
        cap
    }

    fn check_scope_cap(&self, cap: Value, who: &str) -> Result<(), Error> {
        if !(is_kind(cap, Kind::Record) && unsafe { field(cap.as_ptr(), 0) } == self.special(SpecialObj::ScopeCapRtd)) {
            return Err(type_error(who, "scope-capability", cap));
        }
        Ok(())
    }

    fn scope_cap_is_live(&self, cap: Value, who: &str) -> Result<bool, Error> {
        self.check_scope_cap(cap, who)?;
        let cur = unsafe { field(cap.as_ptr(), CAP_CURRENT) };
        Ok(self.resolve_adopted_scope(cur).is_some())
    }

    /// Fast check whether `cap` is live and already matches the current
    /// `(current-scope, authority-scope)` pair, avoiding allocation and
    /// dynamic-wind/local-map churn on the common same-scope path.
    #[inline]
    fn scope_cap_matches_current(&self, cap: Value) -> bool {
        let fast = self.current_self_cap();
        if fast != Value::FALSE && cap == fast {
            return true;
        }
        if !(is_kind(cap, Kind::Record) && unsafe { field(cap.as_ptr(), 0) } == self.special(SpecialObj::ScopeCapRtd)) {
            return false;
        }
        let cap_auth = unsafe { field(cap.as_ptr(), CAP_AUTHORITY) };
        if cap_auth != self.authority_scope_value() {
            return false;
        }
        let raw_cur = unsafe { field(cap.as_ptr(), CAP_CURRENT) };
        let cur = self.current_scope_value();
        if raw_cur == cur {
            return self.is_scope_record(raw_cur) && unsafe { field(raw_cur.as_ptr(), SLOT) }.is_int();
        }
        self.resolve_adopted_scope(raw_cur).is_some_and(|(_, r)| r == cur)
    }

    /// Whether the current authority already covers `cap`'s captured authority,
    /// so entering `cap` cannot escalate privileges.
    fn scope_cap_within_authority(&self, cap: Value, who: &str) -> Result<bool, Error> {
        self.check_scope_cap(cap, who)?;
        if self.is_root_authority() {
            return Ok(true);
        }
        let cap_auth = unsafe { field(cap.as_ptr(), CAP_AUTHORITY) };
        if cap_auth == self.authority_scope_value() {
            return Ok(true);
        }
        match self.scope_slot(cap_auth, who)? {
            Some(slot) => Ok(self.has_authority_over(slot)),
            None => Ok(false),
        }
    }

    fn restore_scope_cap(&mut self, cap: Value, who: &str) -> Result<(), Error> {
        self.check_scope_cap(cap, who)?;
        let raw_cur = unsafe { field(cap.as_ptr(), CAP_CURRENT) };
        let raw_auth = unsafe { field(cap.as_ptr(), CAP_AUTHORITY) };
        // Adoption moves lifetime ownership (CAP_CURRENT) to the adopter,
        // but must never widen the captured privilege ceiling (CAP_AUTHORITY).
        let cur = self.resolve_adopted_scope_compress(raw_cur).map_or(raw_cur, |(_, r)| r);
        let cur_root = self.root(cur);
        let auth_root = self.root(raw_auth);
        self.locals.insert(CURRENT_SCOPE_KEY, cur_root);
        self.locals.insert(AUTHORITY_SCOPE_KEY, auth_root);
        Ok(())
    }

    fn narrow_scope(&mut self, target: Value) -> Result<(), Error> {
        let slot = self.live_scope(target, "with-scope")?;
        self.require_authority_over(slot, "with-scope")?;
        let target_root = self.root(target);
        self.locals.insert(CURRENT_SCOPE_KEY, target_root.clone());
        self.locals.insert(AUTHORITY_SCOPE_KEY, target_root);
        Ok(())
    }

    fn scope(&self, slot: usize) -> &Scope {
        self.scopes[slot].as_ref().expect("a live scope")
    }

    fn scope_mut(&mut self, slot: usize) -> &mut Scope {
        self.scopes[slot].as_mut().expect("a live scope")
    }

    fn make_scope_inner(&mut self, name: Value, parent: Value, check_authority: bool, allow_world_root: bool) -> Result<Value, Error> {
        // Only the root scope (slot 0) and the dedicated non-root world
        // dependency container (slot 1) have no parent.
        let parent_slot = match parent {
            Value::FALSE if self.scopes.is_empty() => None,
            Value::FALSE if allow_world_root && self.scopes.len() == 1 => None,
            Value::FALSE => return Err(Error::new("make-scope: a scope needs a parent scope")),
            _ => {
                let p = self.live_scope(parent, "make-scope")?;
                if check_authority {
                    self.require_authority_over(p, "make-scope")?;
                }
                Some(p)
            }
        };
        let slot = self.free_scopes.pop().unwrap_or(self.scopes.len());
        let rtd = self.special(SpecialObj::ScopeRtd);
        let record = self.make_record(rtd, &[Value::int_unchecked(slot as i64), name, parent, Value::FALSE, Value::FALSE, Value::FALSE]);
        let scope = Scope { record, self_cap: Value::FALSE, parent: parent_slot, children: Vec::new(), owned: Vec::new(), added: 0 };
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

    fn make_scope(&mut self, name: Value, parent: Value) -> Result<Value, Error> {
        self.make_scope_inner(name, parent, true, false)
    }

    fn scope_own(&mut self, slot: usize, value: Value, cleanup: Value, done: Value) -> Result<(), Error> {
        let fast_cap = self.current_self_cap();
        let id = self.next_owned_id;
        self.next_owned_id = self.next_owned_id.wrapping_add(1);
        let entry = if fast_cap != Value::FALSE {
            Owned { id, value, cleanup, done, cap: fast_cap }
        } else {
            let value = self.root(value);
            let cleanup = self.root(cleanup);
            let done = self.root(done);
            let cap = self.capture_scope_cap();
            Owned { id, value: value.get(), cleanup: cleanup.get(), done: done.get(), cap }
        };
        self.scope_own_entry(slot, entry)
    }

    fn scope_own_entry(&mut self, slot: usize, entry: Owned) -> Result<(), Error> {
        let scope = self.scope_mut(slot);
        scope.owned.push(entry);
        scope.added = scope.added.wrapping_add(1);
        if scope.owned.len() > PRUNE && scope.added.is_multiple_of(PRUNE as u32) {
            // Opportunistically drop already-ended native-checked entries without
            // allocating a Root per entry. `scope.owned` is traced by GC and
            // `call_native_value` roots arguments on the VM stack. Reconcile
            // against `scope_rec` and bail out safely if a predicate mutates
            // `scope.owned` mid-pass.
            let scope_rec = self.root(self.scope(slot).record);
            let mut dead_idx: Vec<usize> = Vec::new();
            let mut dead_ids: Vec<u64> = Vec::new();
            let mut mutated = false;
            let mut i = 0usize;
            let mut cur = self.scopes.get(slot).and_then(Option::as_ref).filter(|s| s.record == scope_rec.get());
            while let Some(scope) = cur {
                let len = scope.owned.len();
                if i >= len {
                    break;
                }
                let o = &scope.owned[i];
                let (id, value, done, cap) = (o.id, o.value, o.done, o.cap);
                if !done.is_native() {
                    i += 1;
                    continue;
                }
                let fast_cap = self.current_self_cap();
                let is_done = if fast_cap != Value::FALSE && cap == fast_cap {
                    self.call_native_value(done, &[value]).is_ok_and(|v| v.is_truthy())
                } else {
                    self.ended(value, done, cap)
                };
                if is_done {
                    dead_idx.push(i);
                    dead_ids.push(id);
                }
                cur = self.scopes.get(slot).and_then(Option::as_ref).filter(|s| s.record == scope_rec.get());
                let Some(scope) = cur else {
                    mutated = true;
                    break;
                };
                if scope.owned.len() != len || scope.owned.get(i).is_none_or(|o| o.id != id) {
                    mutated = true;
                    break;
                }
                i += 1;
            }
            if !dead_ids.is_empty()
                && let Some(scope) = self.scopes.get_mut(slot).and_then(Option::as_mut).filter(|s| s.record == scope_rec.get())
            {
                if !mutated {
                    let mut di = 0usize;
                    let mut idx = 0usize;
                    scope.owned.retain(|_| {
                        let remove = dead_idx.get(di) == Some(&idx);
                        if remove {
                            di += 1;
                        }
                        idx += 1;
                        !remove
                    });
                } else if dead_ids.len() <= 8 {
                    scope.owned.retain(|o| !dead_ids.contains(&o.id));
                } else {
                    let dead: rustc_hash::FxHashSet<u64> = dead_ids.into_iter().collect();
                    scope.owned.retain(|o| !dead.contains(&o.id));
                }
            }
        }
        Ok(())
    }

    /// Whether `value` ended by itself, as far as Rust can tell: its test
    /// `done` is a native that says so, evaluated under its registration cap.
    fn ended(&mut self, value: Value, done: Value, cap: Value) -> bool {
        done.is_native() && self.call_native_with_cap(cap, done, &[value]).is_ok_and(|v| v.is_truthy())
    }

    fn restore_saved_scope_locals(&mut self, prev_cur: Option<Root>, prev_auth: Option<Root>) {
        match prev_cur {
            Some(old) => {
                self.locals.insert(CURRENT_SCOPE_KEY, old);
            }
            None => {
                self.locals.remove(&CURRENT_SCOPE_KEY);
            }
        }
        match prev_auth {
            Some(old) => {
                self.locals.insert(AUTHORITY_SCOPE_KEY, old);
            }
            None => {
                self.locals.remove(&AUTHORITY_SCOPE_KEY);
            }
        }
    }

    /// Run `f` with any outer Scheme exception handlers masked so that an
    /// authority-changing Rust frame (`in_module_load_scope`, `call_native_with_cap`)
    /// never invokes a caller's exception handler before restoring caller scope
    /// and authority. On non-escape error, resets `searched` so outer handlers
    /// still run after restoration.
    fn with_masked_outer_handlers<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T, Error>) -> Result<T, Error> {
        let base_handlers = self.handlers.len();
        if base_handlers > 0 {
            self.handlers.push(super::Handler::Mask { lo: 0, raised: None, vm_barrier: true });
        }
        let mut out = f(self);
        self.handlers.truncate(base_handlers);
        if let Err(ref mut e) = out
            && e.escape.is_none()
        {
            e.searched = None;
        }
        out
    }

    /// Call the native `f` with `args` under the scope/authority captured in `cap`.
    fn call_native_with_cap(&mut self, cap: Value, f: Value, args: &[Value]) -> Result<Value, Error> {
        if self.scope_cap_matches_current(cap) {
            return self.call_native_value(f, args);
        }
        let prev_cur = self.locals.get(&CURRENT_SCOPE_KEY).cloned();
        let prev_auth = self.locals.get(&AUTHORITY_SCOPE_KEY).cloned();
        let f_root = self.root(f);
        let arg_roots: Vec<Root> = args.iter().map(|&a| self.root(a)).collect();
        self.restore_scope_cap(cap, "scope-cleanup")?;
        let arg_vals: Vec<Value> = arg_roots.iter().map(Root::get).collect();
        let out = self.with_masked_outer_handlers(|vm| vm.call_native_value(f_root.get(), &arg_vals));
        self.restore_saved_scope_locals(prev_cur, prev_auth);
        out
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

    /// Shut scope `root_slot` and its children iteratively (postorder worklist,
    /// O(1) Rust stack depth): free their slots and release what they own,
    /// children before parent and newest owned first, calling the cleanups that
    /// are natives under each entry's captured registration authority.
    /// Into `released` go the values owned, into `lisp` the entries whose
    /// cleanups are Lisp's to run; the first native cleanup to fail is
    /// returned.
    fn shut_scope(&mut self, root_slot: usize, released: &mut Vec<Root>, lisp: &mut Vec<Taken>) -> Option<Error> {
        enum Step {
            Enter(usize, Option<Root>),
            Finish(Vec<Taken>),
        }
        let mut stack = vec![Step::Enter(root_slot, None)];
        let mut failure = None;
        while let Some(step) = stack.pop() {
            match step {
                Step::Enter(slot, expected_rec) => {
                    if let Some(rec) = expected_rec
                        && !self.scopes.get(slot).and_then(Option::as_ref).is_some_and(|s| s.record == rec.get())
                    {
                        continue;
                    }
                    let Some(scope) = self.scopes.get_mut(slot).and_then(Option::take) else {
                        continue;
                    };
                    self.free_scopes.push(slot);
                    unsafe { set_field(scope.record.as_ptr(), SLOT, Value::FALSE) };
                    // Out of the table, which the collector traces: rooted.
                    let owned: Vec<Taken> = scope
                        .owned
                        .iter()
                        .map(|o| Taken {
                            value: self.root(o.value),
                            cleanup: self.root(o.cleanup),
                            done: self.root(o.done),
                            cap: self.root(o.cap),
                        })
                        .collect();
                    if let Some(p) = scope.parent
                        && let Some(parent) = self.scopes[p].as_mut()
                    {
                        parent.children.retain(|&c| c != slot);
                    }
                    let child_records: Vec<(usize, Root)> = scope
                        .children
                        .iter()
                        .filter_map(|&c| self.scopes.get(c).and_then(Option::as_ref).map(|s| (c, s.record)))
                        .collect::<Vec<_>>()
                        .into_iter()
                        .map(|(c, rec)| (c, self.root(rec)))
                        .collect();
                    stack.push(Step::Finish(owned));
                    // Push in reverse so children are popped in forward order
                    // and every descendant finishes before its parent's Finish.
                    for (child, rec) in child_records.into_iter().rev() {
                        stack.push(Step::Enter(child, Some(rec)));
                    }
                }
                Step::Finish(owned) => {
                    for o in owned.into_iter().rev() {
                        released.push(o.value.clone());
                        if self.ended(o.value.get(), o.done.get(), o.cap.get()) {
                            continue;
                        }
                        let cleanup = o.cleanup.get();
                        if cleanup.is_native() {
                            if let Err(e) = self.call_native_with_cap(o.cap.get(), cleanup, &[o.value.get()]) {
                                failure = worse(failure, Some(e));
                            }
                        } else {
                            lisp.push(o);
                        }
                    }
                }
            }
        }
        failure
    }

    /// Move everything owned by `child_slot` (resources and child scopes) into
    /// `parent_slot` without running cleanups, retire `child_slot`, and mark
    /// `child_slot` as adopted by `parent_slot` so `scope-procedure` closures
    /// captured during initialization stay live under `parent_slot`. Returns
    /// the moved resource values (oldest first) so Scheme can update `%owners`.
    fn adopt_scope(&mut self, child_slot: usize, parent_slot: usize) -> Result<Value, Error> {
        if child_slot == parent_slot {
            return Ok(Value::NIL);
        }
        if child_slot == 0 || child_slot == 1 {
            return Err(Error::new("%scope-adopt!: cannot adopt root or world scope"));
        }
        if self.scope_contains(child_slot, parent_slot) {
            return Err(Error::new("%scope-adopt!: cannot adopt a scope into its own descendant"));
        }
        let child = self.scopes[child_slot].take().ok_or_else(|| Error::new("%scope-adopt!: child scope is shut down"))?;
        self.free_scopes.push(child_slot);
        let parent_record = self.scope(parent_slot).record;
        unsafe {
            set_field(child.record.as_ptr(), SLOT, Value::FALSE);
            set_field(child.record.as_ptr(), ADOPTED, parent_record);
        }
        self.write_barrier(child.record.as_ptr(), parent_record);
        if let Some(p) = child.parent
            && let Some(parent) = self.scopes[p].as_mut()
        {
            parent.children.retain(|&c| c != child_slot);
        }
        for &c in &child.children {
            let c_record = {
                let cs = self.scope_mut(c);
                cs.parent = Some(parent_slot);
                cs.record
            };
            unsafe { set_field(c_record.as_ptr(), PARENT, parent_record) };
            self.write_barrier(c_record.as_ptr(), parent_record);
        }
        let moved: Vec<Root> = child.owned.iter().map(|o| self.root(o.value)).collect();
        {
            let parent = self.scope_mut(parent_slot);
            // Keep newest-first child order: child's children were created after older siblings.
            let mut children = child.children;
            children.extend(parent.children.iter().copied());
            parent.children = children;
            parent.owned.extend(child.owned);
        }
        Ok(list_of(self, moved.iter().map(Root::get).collect()))
    }

    /// Run `f` under the appropriate lifetime and authority scope when a
    /// module loads synchronously while a package is staged here:
    /// - Trusted core world modules (shipped Lisp on disk) run in a temporary
    ///   child of `%root-scope` with root authority, adopted into `%root-scope`
    ///   on success and shut down on failure.
    /// - Other world modules (files outside the package dir, provider libraries)
    ///   run in an isolated non-root child of `%world-scope` with non-root
    ///   authority (kept live on success for world lifetime, shut down on failure).
    /// - Modules belonging to the staged package run in `%staging-scope`.
    pub(crate) fn in_module_load_scope<T>(
        &mut self,
        module: u32,
        from_disk: bool,
        f: impl FnOnce(&mut Self) -> Result<T, Error>,
    ) -> Result<T, Error> {
        if !from_disk || (self.staging_here().is_none() && self.is_root_authority()) {
            return f(self);
        }
        if self.modules[module as usize].generation == 0 {
            let trusted = self.is_trusted_core_module(module);
            let parent = if trusted { self.root_scope_record() } else { self.world_scope_record() };
            if !parent.is_truthy() {
                return f(self);
            }
            let prev_cur = self.locals.get(&CURRENT_SCOPE_KEY).cloned();
            let prev_auth = self.locals.get(&AUTHORITY_SCOPE_KEY).cloned();
            let name = Value::symbol(crate::reader::intern(if trusted { "world-load" } else { "world-dep" }));
            let parent_root = self.root(parent);
            let tmp = self.make_scope_inner(name, parent_root.get(), false, false)?;
            let tmp_slot = self.live_scope(tmp, "in_module_load_scope")?;
            let tmp_root = self.root(tmp);
            let auth_root = if trusted { parent_root } else { tmp_root.clone() };
            self.locals.insert(CURRENT_SCOPE_KEY, tmp_root.clone());
            self.locals.insert(AUTHORITY_SCOPE_KEY, auth_root);
            let out = self.with_masked_outer_handlers(|vm| {
                let out = f(vm);
                if out.is_ok() {
                    if trusted {
                        let _ = vm.adopt_scope(tmp_slot, 0);
                    }
                } else {
                    let sym = crate::reader::intern("scope-shutdown!");
                    if let Some(&crate::vm::GlobalBinding::Var(g)) = vm.bindings.get(&(crate::vm::ROOT_MODULE, sym)) {
                        let proc = vm.globals[g as usize];
                        let _ = vm.call_host(proc, &[tmp_root.get()]);
                    } else {
                        let (mut released, mut lisp) = (Vec::new(), Vec::new());
                        let _ = vm.shut_scope(tmp_slot, &mut released, &mut lisp);
                    }
                }
                out
            });
            self.restore_saved_scope_locals(prev_cur, prev_auth);
            return out;
        }
        let target = self
            .locals
            .get(&STAGING_SCOPE_KEY)
            .map(Root::get)
            .filter(|v| v.is_truthy() && matches!(self.scope_slot(*v, "in_module_load_scope"), Ok(Some(s)) if s != 0));
        let Some(target) = target else {
            return f(self);
        };
        let prev_cur = self.locals.get(&CURRENT_SCOPE_KEY).cloned();
        let prev_auth = self.locals.get(&AUTHORITY_SCOPE_KEY).cloned();
        let target_root = self.root(target);
        self.locals.insert(CURRENT_SCOPE_KEY, target_root.clone());
        self.locals.insert(AUTHORITY_SCOPE_KEY, target_root);
        let out = self.with_masked_outer_handlers(f);
        self.restore_saved_scope_locals(prev_cur, prev_auth);
        out
    }

    /// Deep-clone a data value for VM-enforced protected parameter storage.
    /// Rejects unsupported mutable/opaque objects, overly deep/cyclic graphs,
    /// and values exceeding the node or byte cloning budgets.
    pub(crate) fn deep_clone_value(&mut self, v: Value, depth: usize) -> Result<Value, Error> {
        let mut budget = CloneBudget::default();
        self.deep_clone_value_budgeted(v, depth, &mut budget)
    }

    fn deep_clone_value_budgeted(&mut self, v: Value, depth: usize, budget: &mut CloneBudget) -> Result<Value, Error> {
        if depth > MAX_CLONE_DEPTH {
            return Err(Error::new("protected parameter: value exceeds maximum nesting depth or is cyclic"));
        }
        budget.charge_nodes(1)?;
        if !v.is_ptr() {
            return Ok(v);
        }
        let kind = Kind::of(unsafe { kind_of(v.as_ptr()) });
        match kind {
            Some(Kind::BigInt | Kind::Ratio | Kind::Complex) => Ok(v),
            Some(Kind::String) => {
                let bytes = self.string_bytes(v).ok_or_else(|| Error::new("protected parameter: invalid string"))?;
                budget.charge_bytes(bytes.len())?;
                let text = std::str::from_utf8(bytes).map_err(|_| Error::new("protected parameter: invalid UTF-8 string"))?.to_owned();
                Ok(self.make_string_raw(&text))
            }
            Some(Kind::Bytevector) => {
                let bytes = self.bytevector_bytes(v).ok_or_else(|| Error::new("protected parameter: invalid bytevector"))?;
                budget.charge_bytes(bytes.len())?;
                let bytes = bytes.to_vec();
                Ok(self.make_bytevector_raw(&bytes))
            }
            Some(Kind::Pair) => {
                budget.charge_bytes(16)?;
                let pair = self.root(v);
                let car = unsafe { field(pair.get().as_ptr(), 0) };
                let cloned_car = self.deep_clone_value_budgeted(car, depth + 1, budget)?;
                let cloned_car = self.root(cloned_car);
                let cdr = unsafe { field(pair.get().as_ptr(), 1) };
                let cloned_cdr = self.deep_clone_value_budgeted(cdr, depth + 1, budget)?;
                Ok(self.alloc_pair_raw(cloned_car.get(), cloned_cdr))
            }
            Some(Kind::Vector) => {
                let vec_root = self.root(v);
                let n = unsafe { len_of(vec_root.get().as_ptr()) };
                budget.charge_nodes(n)?;
                budget.charge_bytes(n.saturating_mul(8))?;
                let mut cloned = Vec::with_capacity(n);
                for i in 0..n {
                    let elem = unsafe { field(vec_root.get().as_ptr(), i) };
                    let c = self.deep_clone_value_budgeted(elem, depth + 1, budget)?;
                    cloned.push(self.root(c));
                }
                let vals: Vec<Value> = cloned.iter().map(Root::get).collect();
                Ok(self.make_vector_raw(&vals))
            }
            Some(Kind::Box) => {
                budget.charge_bytes(8)?;
                let box_root = self.root(v);
                let inner = unsafe { field(box_root.get().as_ptr(), 0) };
                let cloned = self.deep_clone_value_budgeted(inner, depth + 1, budget)?;
                self.scratch.push(cloned);
                let p = self.alloc(2);
                let cloned = self.scratch.pop().unwrap();
                unsafe {
                    *p = header(Kind::Box, 1, 0);
                    set_field(p, 0, cloned);
                }
                Ok(Value::ptr(p))
            }
            _ => Err(Error::new("protected parameter: unsupported mutable or opaque value")),
        }
    }

    fn is_protected_param(&self, v: Value) -> bool {
        is_kind(v, Kind::Record) && unsafe { field(v.as_ptr(), 0) } == self.special(SpecialObj::ProtectedParamRtd)
    }

    fn is_saved_param(&self, v: Value) -> bool {
        is_kind(v, Kind::Record) && unsafe { field(v.as_ptr(), 0) } == self.special(SpecialObj::SavedParamRtd)
    }

    fn protected_param_parts(&self, h: Value, who: &str) -> Result<(i64, Value, bool), Error> {
        if !self.is_protected_param(h) {
            return Err(type_error(who, "protected-parameter", h));
        }
        let key = unsafe { field(h.as_ptr(), PP_KEY) }.as_int();
        let default = unsafe { field(h.as_ptr(), PP_DEFAULT) };
        let root_only = unsafe { field(h.as_ptr(), PP_ROOT_ONLY) }.is_truthy();
        Ok((key, default, root_only))
    }

    fn protected_param_converter(&self, h: Value, who: &str) -> Result<Value, Error> {
        if !self.is_protected_param(h) {
            return Err(type_error(who, "protected-parameter", h));
        }
        Ok(unsafe { field(h.as_ptr(), PP_CONVERT) })
    }

    fn make_protected_param(&mut self, proc: Value, init: Value, convert: Value, root_only: bool) -> Result<Value, Error> {
        if root_only && !self.is_root_authority() {
            return Err(Error::new("make-root-parameter: requires root authority"));
        }
        if !self.is_procedure_raw(proc) {
            return Err(type_error("%make-protected-param", "procedure", proc));
        }
        if !self.is_procedure_raw(convert) {
            return Err(type_error("%make-protected-param", "procedure", convert));
        }
        let proc = self.root(proc);
        let convert = self.root(convert);
        let cloned = self.deep_clone_value(init, 0)?;
        let cloned = self.root(cloned);
        let key = self.next_protected_local_key;
        self.next_protected_local_key -= 1;
        let rtd = self.special(SpecialObj::ProtectedParamRtd);
        Ok(self.make_record(rtd, &[proc.get(), Value::int_unchecked(key), cloned.get(), convert.get(), Value::bool(root_only)]))
    }

    fn protected_param_ref(&mut self, h: Value) -> Result<Value, Error> {
        let (key, default, _) = self.protected_param_parts(h, "%protected-param-ref")?;
        let raw = self.locals.get(&key).map_or(default, Root::get);
        self.deep_clone_value(raw, 0)
    }

    fn protected_param_bound(&self, h: Value) -> Result<bool, Error> {
        let (key, _, _) = self.protected_param_parts(h, "%protected-param-bound?")?;
        Ok(self.locals.contains_key(&key))
    }

    fn protected_param_save(&mut self, h: Value) -> Result<Value, Error> {
        let (key, _, root_only) = self.protected_param_parts(h, "%protected-param-save")?;
        if root_only && !self.is_root_authority() {
            return Err(Error::new("parameterize: protected parameter requires root authority"));
        }
        let (present, val) = match self.locals.get(&key) {
            Some(r) => (true, r.get()),
            None => (false, Value::FALSE),
        };
        let rtd = self.special(SpecialObj::SavedParamRtd);
        Ok(self.make_record(rtd, &[h, Value::bool(present), val]))
    }

    fn protected_param_prepare(&mut self, h: Value, val: Value) -> Result<Value, Error> {
        let (_, _, root_only) = self.protected_param_parts(h, "%protected-param-prepare")?;
        if root_only && !self.is_root_authority() {
            return Err(Error::new("parameterize: protected parameter requires root authority"));
        }
        let h = self.root(h);
        let cloned = self.deep_clone_value(val, 0)?;
        let rtd = self.special(SpecialObj::SavedParamRtd);
        Ok(self.make_record(rtd, &[h.get(), Value::TRUE, cloned]))
    }

    fn protected_param_set(&mut self, h: Value, val: Value) -> Result<(), Error> {
        let (key, _, root_only) = self.protected_param_parts(h, "%protected-param-set!")?;
        if root_only && !self.is_root_authority() {
            return Err(Error::new("parameterize: protected parameter requires root authority"));
        }
        let cloned = self.deep_clone_value(val, 0)?;
        let r = self.root(cloned);
        self.locals.insert(key, r);
        Ok(())
    }

    fn protected_param_restore(&mut self, saved: Value) -> Result<(), Error> {
        if !self.is_saved_param(saved) {
            return Err(type_error("%protected-param-restore!", "saved-parameter", saved));
        }
        let h = unsafe { field(saved.as_ptr(), SP_HANDLE) };
        let present = unsafe { field(saved.as_ptr(), SP_PRESENT) }.is_truthy();
        let val = unsafe { field(saved.as_ptr(), SP_VALUE) };
        let (key, _, root_only) = self.protected_param_parts(h, "%protected-param-restore!")?;
        if root_only && !self.is_root_authority() {
            return Err(Error::new("parameterize: protected parameter requires root authority"));
        }
        if present {
            let r = self.root(val);
            self.locals.insert(key, r);
        } else {
            self.locals.remove(&key);
        }
        Ok(())
    }

    fn regular_param_fields(&self, p: Value, who: &str) -> Result<(i64, Value), Error> {
        if !(is_kind(p, Kind::Record) && unsafe { field(p.as_ptr(), 0) } == self.special(SpecialObj::ParameterRtd)) {
            return Err(type_error(who, "parameter", p));
        }
        let k = unsafe { field(p.as_ptr(), 2) };
        if !k.is_int() {
            return Err(type_error(who, "integer", k));
        }
        Ok((k.as_int(), unsafe { field(p.as_ptr(), 3) }))
    }

    fn parameter_converter(&self, p: Value) -> Result<Value, Error> {
        if self.is_protected_param(p) {
            self.protected_param_converter(p, "%parameter-convert")
        } else {
            Ok(self.regular_param_fields(p, "%parameter-convert")?.1)
        }
    }

    fn parameter_bound(&self, p: Value) -> Result<bool, Error> {
        if self.is_protected_param(p) {
            self.protected_param_bound(p)
        } else {
            let (key, _) = self.regular_param_fields(p, "parameter-bound?")?;
            Ok(self.locals.contains_key(&key))
        }
    }

    fn parameter_save(&mut self, p: Value) -> Result<Value, Error> {
        if self.is_protected_param(p) {
            return self.protected_param_save(p);
        }
        let (key, _) = self.regular_param_fields(p, "%parameter-save")?;
        match self.locals.get(&key) {
            Some(r) => {
                let v = r.get();
                Ok(self.alloc_pair_raw(Value::TRUE, v))
            }
            None => Ok(Value::FALSE),
        }
    }

    fn parameter_prepare(&mut self, p: Value, val: Value) -> Result<Value, Error> {
        if self.is_protected_param(p) {
            return self.protected_param_prepare(p, val);
        }
        let _ = self.regular_param_fields(p, "%parameter-prepare")?;
        Ok(val)
    }

    fn parameter_restore(&mut self, p: Value, saved: Value) -> Result<(), Error> {
        if self.is_protected_param(p) {
            return self.protected_param_restore(saved);
        }
        let (key, _) = self.regular_param_fields(p, "%parameter-restore!")?;
        if key <= CURRENT_SCOPE_KEY {
            return Err(Error::new("%parameter-restore!: cannot modify reserved or protected key"));
        }
        if is_kind(saved, Kind::Pair) {
            let v = unsafe { field(saved.as_ptr(), 1) };
            let r = self.root(v);
            self.locals.insert(key, r);
        } else {
            self.locals.remove(&key);
        }
        Ok(())
    }

    fn parameter_bind_all(&mut self, mut params: Value, mut prepared: Value, mut old: Value) -> Result<(), Error> {
        let mut bound: Vec<(Value, Value)> = Vec::new();
        while is_kind(params, Kind::Pair) {
            if !is_kind(prepared, Kind::Pair) || !is_kind(old, Kind::Pair) {
                break;
            }
            let p = unsafe { field(params.as_ptr(), 0) };
            let v = unsafe { field(prepared.as_ptr(), 0) };
            let o = unsafe { field(old.as_ptr(), 0) };
            let res = if self.is_protected_param(p) {
                self.protected_param_restore(v)
            } else {
                match self.regular_param_fields(p, "%parameter-bind-all!") {
                    Ok((key, _)) if key <= CURRENT_SCOPE_KEY => {
                        Err(Error::new("%parameter-bind-all!: cannot modify reserved or protected key"))
                    }
                    Ok((key, _)) => {
                        let r = self.root(v);
                        self.locals.insert(key, r);
                        Ok(())
                    }
                    Err(e) => Err(e),
                }
            };
            if let Err(e) = res {
                for (bp, bo) in bound.into_iter().rev() {
                    let _ = self.parameter_restore(bp, bo);
                }
                return Err(e);
            }
            bound.push((p, o));
            params = unsafe { field(params.as_ptr(), 1) };
            prepared = unsafe { field(prepared.as_ptr(), 1) };
            old = unsafe { field(old.as_ptr(), 1) };
        }
        Ok(())
    }

    fn take_cleanup_action(&mut self, action: Value) -> Result<Value, Error> {
        if !(is_kind(action, Kind::Record) && unsafe { field(action.as_ptr(), 0) } == self.special(SpecialObj::CleanupActionRtd)) {
            return Err(type_error("%cleanup-action-take!", "cleanup-action", action));
        }
        let cleanup = unsafe { field(action.as_ptr(), CA_CLEANUP) };
        if cleanup == Value::FALSE {
            return Ok(Value::FALSE);
        }
        let res = self.root(unsafe { field(action.as_ptr(), CA_RESOURCE) });
        let cleanup = self.root(cleanup);
        let done = self.root(unsafe { field(action.as_ptr(), CA_DONE) });
        let cap = self.root(unsafe { field(action.as_ptr(), CA_CAP) });
        unsafe {
            set_field(action.as_ptr(), CA_RESOURCE, Value::FALSE);
            set_field(action.as_ptr(), CA_CLEANUP, Value::FALSE);
            set_field(action.as_ptr(), CA_DONE, Value::FALSE);
            set_field(action.as_ptr(), CA_CAP, Value::FALSE);
        }
        Ok(self.make_list_raw(&[res.get(), cleanup.get(), done.get(), cap.get()]))
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
    let slot = vm.live_scope(s, who)?;
    vm.require_authority_over(slot, who)?;
    unsafe { set_field(s.as_ptr(), i, v) };
    vm.write_barrier(s.as_ptr(), v);
    Ok(Value::VOID)
}

/// The values of `items`, oldest first, as a list.
fn list_of(vm: &mut Vm, items: Vec<Value>) -> Value {
    vm.make_list_raw(&items)
}

fn scope_shutdown(vm: &mut Vm, args: usize, _: usize) -> Result<Value, Error> {
    let Some(slot) = vm.scope_slot(arg(vm, args, 0), "%scope-shutdown!")? else { return Ok(Value::FALSE) };
    vm.require_authority_over(slot, "scope-shutdown!")?;
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
                None => vm.make_error_object_of_raw(&message, &[], e.kind),
            }
        }
        None => Value::FALSE,
    };
    let failure = vm.root(failure);
    let entries: Vec<Root> = lisp
        .into_iter()
        .map(|o| {
            let action_rtd = vm.special(SpecialObj::CleanupActionRtd);
            let entry = vm.make_record(action_rtd, &[o.value.get(), o.cleanup.get(), o.done.get(), o.cap.get()]);
            vm.root(entry)
        })
        .collect();
    let entries = list_of(vm, entries.iter().map(Root::get).collect());
    let entries = vm.root(entries);
    let released = list_of(vm, released.iter().map(Root::get).collect());
    let released = vm.root(released);
    Ok(vm.make_list_raw(&[failure.get(), released.get(), entries.get()]))
}

const BOOT_SEALED_GLOBALS: &[&str] = &[
    "%world-load-enter!",
    "%staging-load-enter!",
    "%cleanup-action-take!",
    "%scope-restore!",
    "%make-world-scope",
    "%enter-forms-module",
    "%set-staging-scope!",
    "%package-begin",
    "%package-end",
    "%package-publish",
    "%package-discard",
    "%retire-generation",
    "%push-handler",
    "%push-wind",
    "%pop-handler",
    "%resume-unwind",
    "%push-mask",
    "%handler-at",
    "%continuable-handler",
    "%handler-returned",
    "%seal-vm-boot!",
];

pub(crate) fn is_boot_sealed(name: &str) -> bool {
    name.starts_with('%') && BOOT_SEALED_GLOBALS.contains(&name)
}

impl Vm {
    /// Whether the current execution has a live, root-installed staging scope.
    pub(crate) fn in_active_staging_scope(&self) -> bool {
        let Some(s) = self.locals.get(&STAGING_SCOPE_KEY).map(Root::get).filter(|v| v.is_truthy()) else {
            return false;
        };
        matches!(self.scope_slot(s, "in_active_staging_scope"), Ok(Some(slot)) if slot != 0)
    }
}

pub fn install(vm: &mut Vm) {
    crate::natives! { vm;
        "(%current-scope-key)" => |_: &mut Vm, _, _| Ok(Value::int_unchecked(CURRENT_SCOPE_KEY));
        "(%current-scope)" => |vm: &mut Vm, _, _| Ok(vm.current_scope_value());
        "(%staging-scope-key)" => |_: &mut Vm, _, _| Ok(Value::int_unchecked(STAGING_SCOPE_KEY));
        /// Return the scope representing the current authority.
        "(authority-scope)" => |vm: &mut Vm, _, _| Ok(vm.authority_scope_value());
        /// Return #t if the current execution holds root scope authority.
        "(root-authority?)" => |vm: &mut Vm, _, _| Ok(Value::bool(vm.is_root_authority()));
        "(%scope-capture)" => |vm: &mut Vm, _, _| Ok(vm.capture_scope_cap());
        "(%scope-cap-live? cap)" => |vm: &mut Vm, a, _| Ok(Value::bool(vm.scope_cap_is_live(arg(vm, a, 0), "%scope-cap-live?")?));
        "(%scope-cap-current? cap)" => |vm: &mut Vm, a, _| Ok(Value::bool(vm.scope_cap_matches_current(arg(vm, a, 0))));
        "(%scope-cap-within-authority? cap)" => |vm: &mut Vm, a, _| {
            Ok(Value::bool(vm.scope_cap_within_authority(arg(vm, a, 0), "%scope-cap-within-authority?")?))
        };
        "(%scope-resolve s)" => |vm: &mut Vm, a, _| {
            let s = arg(vm, a, 0);
            vm.scope_slot(s, "%scope-resolve")?;
            Ok(vm.resolve_adopted_scope_compress(s).map_or(Value::FALSE, |(_, r)| r))
        };
        "(%scope-restore! cap)" => |vm: &mut Vm, a, _| {
            vm.restore_scope_cap(arg(vm, a, 0), "%scope-restore!")?;
            Ok(Value::VOID)
        };
        "(%cleanup-action-take! action)" => |vm: &mut Vm, a, _| vm.take_cleanup_action(arg(vm, a, 0));
        "(%seal-vm-boot!)" => |vm: &mut Vm, _, _| {
            vm.seal_boot_globals(BOOT_SEALED_GLOBALS);
            Ok(Value::VOID)
        };
        "(%scope-narrow! s)" => |vm: &mut Vm, a, _| {
            vm.narrow_scope(arg(vm, a, 0))?;
            Ok(Value::VOID)
        };
        "(%set-staging-scope! s)" => |vm: &mut Vm, a, _| {
            if !vm.is_root_authority() {
                return Err(Error::new("%set-staging-scope!: requires root authority"));
            }
            let s = arg(vm, a, 0);
            if s == Value::FALSE {
                vm.locals.remove(&STAGING_SCOPE_KEY);
                return Ok(Value::VOID);
            }
            let slot = vm.live_scope(s, "%set-staging-scope!")?;
            vm.require_authority_over(slot, "%set-staging-scope!")?;
            let r = vm.root(s);
            vm.locals.insert(STAGING_SCOPE_KEY, r);
            Ok(Value::VOID)
        };
        "(%make-world-scope)" => |vm: &mut Vm, _, _| {
            let name = Value::symbol(crate::reader::intern("world"));
            vm.make_scope_inner(name, Value::FALSE, false, true)
        };
        "(%world-load-enter! forms)" => |vm: &mut Vm, a, _| {
            let forms = crate::stdlib::forms_arg(vm, arg(vm, a, 0), "%world-load-enter!")?;
            if !vm.forms_of_world(&forms) {
                return Err(Error::new("%world-load-enter!: forms do not belong to a world load"));
            }
            if forms.world_entered.replace(true) {
                return Err(Error::new("%world-load-enter!: forms already entered"));
            }
            let trusted = vm.is_trusted_core_module(Vm::forms_module(&forms));
            let prev = vm.capture_scope_cap();
            let prev = vm.root(prev);
            let parent = if trusted { vm.root_scope_record() } else { vm.world_scope_record() };
            let parent_root = vm.root(parent);
            let name = Value::symbol(crate::reader::intern(if trusted { "world-load" } else { "world-dep" }));
            let tmp = vm.make_scope_inner(name, parent_root.get(), false, false)?;
            let tmp_root = vm.root(tmp);
            let auth_root = if trusted { parent_root } else { tmp_root.clone() };
            vm.locals.insert(CURRENT_SCOPE_KEY, tmp_root.clone());
            vm.locals.insert(AUTHORITY_SCOPE_KEY, auth_root);
            Ok(vm.make_list_raw(&[prev.get(), tmp_root.get(), Value::bool(trusted)]))
        };
        "(%staging-load-enter! forms)" => |vm: &mut Vm, a, _| {
            let forms = crate::stdlib::forms_arg(vm, arg(vm, a, 0), "%staging-load-enter!")?;
            if !vm.forms_of_staging(&forms) {
                return Err(Error::new("%staging-load-enter!: forms do not belong to the active staged package"));
            }
            if forms.world_entered.replace(true) {
                return Err(Error::new("%staging-load-enter!: forms already entered"));
            }
            let Some(staging) = vm.locals.get(&STAGING_SCOPE_KEY).map(Root::get).filter(|v| v.is_truthy()) else {
                return Err(Error::new("%staging-load-enter!: no active staging scope"));
            };
            let slot = vm.live_scope(staging, "%staging-load-enter!")?;
            if slot == 0 {
                return Err(Error::new("%staging-load-enter!: staging scope cannot be root"));
            }
            let staging_root = vm.root(staging);
            let prev = vm.capture_scope_cap();
            vm.locals.insert(CURRENT_SCOPE_KEY, staging_root.clone());
            vm.locals.insert(AUTHORITY_SCOPE_KEY, staging_root);
            Ok(prev)
        };
        "(%make-protected-param proc init convert root-only?)" => |vm: &mut Vm, a, _| {
            vm.make_protected_param(arg(vm, a, 0), arg(vm, a, 1), arg(vm, a, 2), arg(vm, a, 3).is_truthy())
        };
        "(%protected-param-ref param)" => |vm: &mut Vm, a, _| vm.protected_param_ref(arg(vm, a, 0));
        "(%protected-param-set! param value)" => |vm: &mut Vm, a, _| {
            vm.protected_param_set(arg(vm, a, 0), arg(vm, a, 1))?;
            Ok(Value::VOID)
        };
        /// Return #t if P is explicitly bound by an enclosing `parameterize`.
        /// Only checks bindings active in the current task.
        "(parameter-bound? p)" => |vm: &mut Vm, a, _| Ok(Value::bool(vm.parameter_bound(arg(vm, a, 0))?));
        "(%parameter-converter p)" => |vm: &mut Vm, a, _| vm.parameter_converter(arg(vm, a, 0));
        "(%parameter-save p)" => |vm: &mut Vm, a, _| vm.parameter_save(arg(vm, a, 0));
        "(%parameter-prepare p value)" => |vm: &mut Vm, a, _| vm.parameter_prepare(arg(vm, a, 0), arg(vm, a, 1));
        "(%parameter-restore! p saved)" => |vm: &mut Vm, a, _| {
            vm.parameter_restore(arg(vm, a, 0), arg(vm, a, 1))?;
            Ok(Value::VOID)
        };
        "(%parameter-bind-all! params prepared old)" => |vm: &mut Vm, a, _| {
            vm.parameter_bind_all(arg(vm, a, 0), arg(vm, a, 1), arg(vm, a, 2))?;
            Ok(Value::VOID)
        };
        "(%make-scope name parent)" => |vm: &mut Vm, a, _| vm.make_scope(arg(vm, a, 0), arg(vm, a, 1));
        /// Return #t if OBJ is a scope.
        "(scope? obj)" => |vm: &mut Vm, a, _| Ok(Value::bool(vm.is_scope_record(arg(vm, a, 0))));
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
        "(%set-scope-predecessor! s v)" => |vm: &mut Vm, a, _| {
            let (s, v) = (arg(vm, a, 0), arg(vm, a, 1));
            let s_slot = vm.live_scope(s, "%set-scope-predecessor!")?;
            vm.require_authority_over(s_slot, "%set-scope-predecessor!")?;
            if v != Value::FALSE {
                let v_slot = vm.live_scope(v, "%set-scope-predecessor!")?;
                vm.require_authority_over(v_slot, "%set-scope-predecessor!")?;
            }
            unsafe { set_field(s.as_ptr(), PREDECESSOR, v) };
            vm.write_barrier(s.as_ptr(), v);
            Ok(Value::VOID)
        };
        "(%scope-own! s resource cleanup done?)" => |vm: &mut Vm, a, _| {
            let slot = vm.live_scope(arg(vm, a, 0), "scope-own!")?;
            let has_auth = vm.has_authority_over(slot);
            if !has_auth && !vm.can_own_in(slot) {
                return Err(Error::new("scope-own!: target scope is outside current authority"));
            }
            let r = arg(vm, a, 1);
            if !has_auth && vm.scope(slot).owned.iter().any(|o| o.value == r) {
                return Err(Error::new("scope-own!: resource is already owned in target scope"));
            }
            vm.scope_own(slot, r, arg(vm, a, 2), arg(vm, a, 3))?;
            Ok(arg(vm, a, 1))
        };
        "(%scope-disown! s resource)" => |vm: &mut Vm, a, _| {
            let Some(slot) = vm.scope_slot(arg(vm, a, 0), "%scope-disown!")? else { return Ok(Value::FALSE) };
            vm.require_authority_over(slot, "scope-disown!")?;
            let r = arg(vm, a, 1);
            let Some(i) = vm.scope(slot).owned.iter().position(|o| o.value == r) else { return Ok(Value::FALSE) };
            let o = vm.scope_mut(slot).owned.remove(i);
            // Rooted in the registers: allocating may collect.
            vm.regs[a] = o.cleanup;
            vm.regs[a + 1] = o.done;
            Ok(vm.alloc_pair_raw(arg(vm, a, 0), arg(vm, a, 1)))
        };
        "(%scope-transfer! from to resource)" => |vm: &mut Vm, a, _| {
            let from_slot = vm.live_scope(arg(vm, a, 0), "scope-transfer!")?;
            let to_slot = vm.live_scope(arg(vm, a, 1), "scope-transfer!")?;
            vm.require_authority_over(from_slot, "scope-transfer!")?;
            vm.require_authority_over(to_slot, "scope-transfer!")?;
            let r = arg(vm, a, 2);
            let Some(i) = vm.scope(from_slot).owned.iter().position(|o| o.value == r) else {
                return Err(Error::new("scope-transfer!: no scope owns it"));
            };
            if from_slot == to_slot {
                return Ok(r);
            }
            let r_root = vm.root(r);
            let o = vm.scope_mut(from_slot).owned.remove(i);
            vm.scope_own_entry(to_slot, o)?;
            Ok(r_root.get())
        };
        "(%scope-adopt! child parent)" => |vm: &mut Vm, a, _| {
            let child = vm.live_scope(arg(vm, a, 0), "%scope-adopt!")?;
            let parent = vm.live_scope(arg(vm, a, 1), "%scope-adopt!")?;
            vm.require_authority_over(child, "%scope-adopt!")?;
            vm.require_authority_over(parent, "%scope-adopt!")?;
            vm.adopt_scope(child, parent)
        };
        "(%scope-shutdown! s)" => scope_shutdown;
    }
}
