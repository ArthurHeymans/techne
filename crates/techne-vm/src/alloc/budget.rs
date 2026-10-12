//! Explicit reservations and allocator-owned memory, independent of the
//! thread currently running a world. Collector budgets also charge the
//! legacy account while other allocations still use global interception.

use std::{
    alloc::{AllocError, Allocator, Layout, System},
    ptr::NonNull,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering::Relaxed},
    },
};

use super::{AccountRef, Entered};

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

    /// Collector allocation is still admitted by the VM's pressure policy:
    /// it cannot fail halfway through relocating nursery objects. Its buffers
    /// bypass the global allocator but charge the same account exactly once.
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

    /// Reserve before doing work; dropping the reservation gives it back.
    /// Failed reservations leave the budget unchanged. A zero-byte request
    /// is allowed even when an existing allocation exceeds a lowered limit.
    pub fn reserve(&self, bytes: usize) -> Result<Reservation, AllocError> {
        if bytes != 0 {
            self.0
                .held
                .try_update(Relaxed, Relaxed, |held| held.checked_add(bytes).filter(|&next| next <= self.limit()))
                .map_err(|_| AllocError)?;
            if let Some(account) = self.0.account {
                super::add(account.0, bytes as isize);
            }
        }
        Ok(Reservation { budget: self.clone(), bytes })
    }

    fn release(&self, bytes: usize) {
        if bytes != 0 {
            self.0.held.fetch_sub(bytes, Relaxed);
            if let Some(account) = self.0.account {
                super::add(account.0, -(bytes as isize));
            }
        }
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

impl Reservation {
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// The allocation now owns the charge; its allocator releases it.
    fn commit(mut self) {
        self.bytes = 0;
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.budget.release(self.bytes);
    }
}

// SAFETY: storage comes from System with the requested layout and remains
// valid independently of moves/clones of the allocator. Allocator clones
// share accounting, so any clone can resize or free a live allocation.
unsafe impl Allocator for Budget {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let reserved = self.reserve(layout.size())?;
        let allocation = System.allocate(layout)?;
        reserved.commit();
        Ok(allocation)
    }

    fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let reserved = self.reserve(layout.size())?;
        let allocation = System.allocate_zeroed(layout)?;
        reserved.commit();
        Ok(allocation)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // SAFETY: Allocator's caller supplies a live allocation and its layout.
        unsafe { System.deallocate(ptr, layout) };
        self.release(layout.size());
    }

    unsafe fn grow(&self, ptr: NonNull<u8>, old: Layout, new: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let reserved = self.reserve(new.size() - old.size())?;
        // SAFETY: the caller supplies a live allocation and valid grow layouts.
        let allocation = unsafe { System.grow(ptr, old, new) }?;
        reserved.commit();
        Ok(allocation)
    }

    unsafe fn grow_zeroed(&self, ptr: NonNull<u8>, old: Layout, new: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let reserved = self.reserve(new.size() - old.size())?;
        // SAFETY: the caller supplies a live allocation and valid grow layouts.
        let allocation = unsafe { System.grow_zeroed(ptr, old, new) }?;
        reserved.commit();
        Ok(allocation)
    }

    unsafe fn shrink(&self, ptr: NonNull<u8>, old: Layout, new: Layout) -> Result<NonNull<[u8]>, AllocError> {
        // SAFETY: the caller supplies a live allocation and valid shrink layouts.
        let allocation = unsafe { System.shrink(ptr, old, new) }?;
        self.release(old.size() - new.size());
        Ok(allocation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alloc::Account;

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
