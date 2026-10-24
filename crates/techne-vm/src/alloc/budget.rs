//! Explicit reservations and allocator-owned memory, independent of the
//! thread currently running a world. Collector budgets also charge the
//! legacy account while other allocations still use global interception.

use std::{
    alloc::{AllocError, Allocator, Layout},
    ptr::NonNull,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering::Relaxed},
    },
};

use super::{AccountRef, Entered, pages::Pages};

/// Whether the counting allocator counts ordinary Rust allocations: then
/// what is charged by hand where it does not (`Budget::charge`) is not.
pub(crate) const INTERCEPTED: bool = cfg!(feature = "counting-allocator");

thread_local! {
    /// The budget whose limit is lifted for this thread (`Budget::lifted`).
    static LIFTED: std::cell::Cell<*const State> = const { std::cell::Cell::new(std::ptr::null()) };
}

struct State {
    held: AtomicUsize,
    limit: AtomicUsize,
    account: Option<AccountRef>,
}

/// An explicit memory budget. Clones share its limit and reservations.
///
/// Tracks requested allocation sizes and reservations, not allocator
/// bookkeeping, transient realloc workspace or unrelated allocations.
/// Lowering the limit does not revoke allocations, but refuses further growth.
/// The budget remains alive while any allocator or reservation holds it.
/// Use fallible collection growth such as `Vec::try_reserve`: infallible
/// allocation operations may abort when the budget refuses them.
///
/// ```
/// use techne_vm::alloc::Budget;
/// let budget = Budget::new(1024);
/// let mut bytes = Vec::<u8, _>::new_in(budget.clone());
/// bytes.try_reserve_exact(512).unwrap();
/// assert_eq!(budget.held(), 512);
/// assert!(bytes.try_reserve_exact(1025).is_err());
/// drop(bytes);
/// assert_eq!(budget.held(), 0);
/// ```
#[derive(Clone)]
pub struct Budget(Arc<State>);

impl Budget {
    pub fn new(limit: usize) -> Self {
        Self::with_account(limit, None)
    }

    fn with_account(limit: usize, account: Option<AccountRef>) -> Self {
        Self(Arc::new(State { held: AtomicUsize::new(0), limit: AtomicUsize::new(limit), account }))
    }

    /// A world's budget (its heap, registers and globals): its buffers
    /// bypass the global allocator but charge the world's account exactly
    /// once. Unlimited until the world sets its ceiling (`set_limit`).
    pub(crate) fn for_account(account: AccountRef) -> Self {
        Self::with_account((super::COUNT >> 1) as usize, Some(account))
    }

    pub fn held(&self) -> usize {
        self.0.held.load(Relaxed)
    }

    pub fn limit(&self) -> usize {
        self.0.limit.load(Relaxed)
    }

    pub fn set_limit(&self, limit: usize) {
        self.0.limit.store(limit, Relaxed);
    }

    /// What the budget may still grow by, for what this thread reserves.
    pub fn headroom(&self) -> usize {
        if self.is_lifted() { usize::MAX } else { self.limit().saturating_sub(self.held()) }
    }

    /// Run `f` with the limit lifted for what this thread reserves, then
    /// put it back: for what must not fail halfway (a collection relocating
    /// objects), admitted before. Other threads sharing the budget (the
    /// JIT compiler's) are held to the limit meanwhile.
    pub(crate) fn lifted<R>(&self, f: impl FnOnce() -> R) -> R {
        struct Restore(*const State);
        impl Drop for Restore {
            fn drop(&mut self) {
                LIFTED.set(self.0);
            }
        }
        let _restore = Restore(LIFTED.replace(Arc::as_ptr(&self.0)));
        f()
    }

    /// Whether this thread reserves past the limit now (`lifted`).
    fn is_lifted(&self) -> bool {
        LIFTED.get() == Arc::as_ptr(&self.0)
    }

    /// Reserve before doing work; dropping the reservation gives it back.
    /// Failed reservations leave the budget unchanged. A zero-byte request
    /// is allowed even when an existing allocation exceeds a lowered limit.
    pub fn reserve(&self, bytes: usize) -> Result<Reservation, AllocError> {
        if bytes != 0 {
            self.0
                .held
                .try_update(Relaxed, Relaxed, |held| held.checked_add(bytes).filter(|&next| next <= self.limit() || self.is_lifted()))
                .map_err(|_| AllocError)?;
            if let Some(account) = self.0.account {
                super::add(account.0, bytes as isize);
            }
        }
        Ok(Reservation { budget: self.clone(), bytes })
    }

    pub(super) fn release(&self, bytes: usize) {
        if bytes != 0 {
            self.0.held.fetch_sub(bytes, Relaxed);
            if let Some(account) = self.0.account {
                super::add(account.0, -(bytes as isize));
            }
        }
    }

    /// Charge `bytes` of storage not allocated through the budget (a
    /// code's buffers) to it by hand, until the reservation returned is
    /// dropped: none where the counting allocator counts that storage
    /// itself. Refused as a reservation is.
    pub(crate) fn charge(&self, bytes: usize) -> Result<Option<Reservation>, AllocError> {
        if INTERCEPTED { Ok(None) } else { self.reserve(bytes).map(Some) }
    }

    /// Legacy metadata allocations still need the owning account entered.
    pub(crate) fn enter(&self) -> Option<Entered> {
        self.0.account.filter(|a| !a.entered()).map(AccountRef::enter)
    }
}

/// Bytes reserved in a [`Budget`], released on drop.
#[must_use = "dropping the reservation releases its bytes"]
pub struct Reservation {
    budget: Budget,
    bytes: usize,
}

impl std::fmt::Debug for Reservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Reservation({} bytes)", self.bytes)
    }
}

impl Reservation {
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Make it `bytes`, reserving or releasing the difference; refused as
    /// a reservation is, leaving it as it was.
    pub(crate) fn resize(&mut self, bytes: usize) -> Result<(), AllocError> {
        if bytes > self.bytes {
            self.budget.reserve(bytes - self.bytes)?.commit();
        } else {
            self.budget.release(self.bytes - bytes);
        }
        self.bytes = bytes;
        Ok(())
    }

    /// Take `other`'s bytes over, without giving them back meanwhile: they
    /// are of the same budget.
    pub(crate) fn absorb(&mut self, mut other: Reservation) {
        assert!(Arc::ptr_eq(&self.budget.0, &other.budget.0), "a reservation of another budget");
        self.bytes += std::mem::take(&mut other.bytes);
    }

    /// Make it `bytes` whatever the limit: for what exists already (machine
    /// code larger than reserved for), which charging short would hide.
    pub(crate) fn resize_past_limit(&mut self, bytes: usize) {
        if bytes > self.bytes {
            let more = bytes - self.bytes;
            self.budget.0.held.fetch_add(more, Relaxed);
            if let Some(account) = self.budget.0.account {
                super::add(account.0, more as isize);
            }
        } else {
            self.budget.release(self.bytes - bytes);
        }
        self.bytes = bytes;
    }

    /// The allocation now owns the charge; its allocator releases it.
    pub(super) fn commit(mut self) {
        self.bytes = 0;
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.budget.release(self.bytes);
    }
}

// SAFETY: storage comes from `Pages` with the requested layout and remains
// valid independently of moves/clones of the allocator. Allocator clones
// share accounting, so any clone can resize or free a live allocation.
unsafe impl Allocator for Budget {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let reserved = self.reserve(layout.size())?;
        let allocation = Pages.allocate(layout)?;
        reserved.commit();
        Ok(allocation)
    }

    fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let reserved = self.reserve(layout.size())?;
        let allocation = Pages.allocate_zeroed(layout)?;
        reserved.commit();
        Ok(allocation)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // SAFETY: Allocator's caller supplies a live allocation and its layout.
        unsafe { Pages.deallocate(ptr, layout) };
        self.release(layout.size());
    }

    unsafe fn grow(&self, ptr: NonNull<u8>, old: Layout, new: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let reserved = self.reserve(new.size() - old.size())?;
        // SAFETY: the caller supplies a live allocation and valid grow layouts.
        let allocation = unsafe { Pages.grow(ptr, old, new) }?;
        reserved.commit();
        Ok(allocation)
    }

    unsafe fn grow_zeroed(&self, ptr: NonNull<u8>, old: Layout, new: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let reserved = self.reserve(new.size() - old.size())?;
        // SAFETY: the caller supplies a live allocation and valid grow layouts.
        let allocation = unsafe { Pages.grow_zeroed(ptr, old, new) }?;
        reserved.commit();
        Ok(allocation)
    }

    unsafe fn shrink(&self, ptr: NonNull<u8>, old: Layout, new: Layout) -> Result<NonNull<[u8]>, AllocError> {
        // SAFETY: the caller supplies a live allocation and valid shrink layouts.
        let allocation = unsafe { Pages.shrink(ptr, old, new) }?;
        self.release(old.size() - new.size());
        Ok(allocation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alloc::Account;

    #[test]
    fn a_reservation_taken_over_is_never_given_back_meanwhile() {
        let budget = Budget::new(1024);
        let mut arena = budget.reserve(0).unwrap();
        arena.absorb(budget.reserve(1024).unwrap());
        assert_eq!(budget.held(), 1024);
        // What exists is charged though past the limit; less gives back.
        arena.resize_past_limit(2048);
        assert_eq!(budget.held(), 2048);
        arena.resize_past_limit(512);
        assert_eq!(budget.held(), 512);
        drop(arena);
        assert_eq!(budget.held(), 0);
    }

    #[test]
    fn a_lifted_limit_is_this_threads_alone() {
        let budget = Budget::new(1024);
        budget.lifted(|| {
            let _past = budget.reserve(4096).unwrap();
            let other = budget.clone();
            assert!(std::thread::spawn(move || other.reserve(1).is_err()).join().unwrap());
        });
        assert!(budget.reserve(2048).is_err());
    }

    #[test]
    fn collector_buffers_charge_their_owner_once() {
        let a = Account::new().unwrap();
        let b = Account::new().unwrap();
        let budget = Budget::for_account(a.reference());
        let (held_a, held_b) = (a.held(), b.held());
        let _other_world = b.enter();
        let mut bytes = Vec::<u8, _>::new_in(budget.clone());
        bytes.try_reserve_exact(4096).unwrap();
        assert_eq!(budget.held(), 4096);
        assert_eq!(a.held(), held_a + 4096);
        assert_eq!(b.held(), held_b);
        drop(bytes);
        assert_eq!(a.held(), held_a);
        assert_eq!(b.held(), held_b);
    }

    #[test]
    fn a_collector_budget_survives_its_legacy_account() {
        let a = Account::new().unwrap();
        let expired = a.reference();
        let budget = Budget::for_account(expired);
        let mut bytes = Vec::<u8, _>::new_in(budget.clone());
        bytes.try_reserve_exact(4096).unwrap();
        drop(a);
        let b = Account::new().unwrap();
        let other = Budget::for_account(b.reference());
        let _kept = other.reserve(1024).unwrap();
        let held_b = b.held();
        bytes.resize(4096, 0);
        bytes.try_reserve_exact(4096).unwrap();
        assert_eq!(budget.held(), 8192);
        assert_eq!(expired.held(), 0);
        assert_eq!(b.held(), held_b);
        drop(bytes);
        assert_eq!(budget.held(), 0);
        assert_eq!(b.held(), held_b);
    }
}
