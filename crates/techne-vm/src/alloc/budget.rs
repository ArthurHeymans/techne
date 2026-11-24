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
    static LIFTED: std::cell::Cell<*const ()> = const { std::cell::Cell::new(std::ptr::null()) };
}

/// Whether a budget's limit is lifted for this thread now (`lifted`).
pub(super) fn lifting() -> bool {
    !LIFTED.get().is_null()
}

#[inline(always)]
fn charge_delta(bytes: usize) -> isize {
    bytes as isize
}

#[inline(always)]
fn credit_delta(bytes: usize) -> isize {
    (bytes as isize).wrapping_neg()
}

trait AccountSink {
    fn charge(&self, bytes: usize);
    fn credit(&self, bytes: usize);
}

impl AccountSink for Option<AccountRef> {
    fn charge(&self, bytes: usize) {
        if let Some(account) = *self {
            super::add(account.0, charge_delta(bytes));
        }
    }

    fn credit(&self, bytes: usize) {
        if let Some(account) = *self {
            super::add(account.0, credit_delta(bytes));
        }
    }
}

struct State<A = Option<AccountRef>> {
    held: AtomicUsize,
    limit: AtomicUsize,
    account: A,
}

struct BudgetCore<A = Option<AccountRef>>(Arc<State<A>>);

impl<A> Clone for BudgetCore<A> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<A: AccountSink> BudgetCore<A> {
    fn new(limit: usize, account: A) -> Self {
        Self(Arc::new(State { held: AtomicUsize::new(0), limit: AtomicUsize::new(limit), account }))
    }

    fn held(&self) -> usize {
        self.0.held.load(Relaxed)
    }

    fn limit(&self) -> usize {
        self.0.limit.load(Relaxed)
    }

    fn set_limit(&self, limit: usize) {
        self.0.limit.store(limit, Relaxed);
    }

    fn headroom(&self) -> usize {
        if self.is_lifted() { usize::MAX } else { self.limit().saturating_sub(self.held()) }
    }

    fn lifted<R>(&self, f: impl FnOnce() -> R) -> R {
        struct Restore(*const ());
        impl Drop for Restore {
            fn drop(&mut self) {
                LIFTED.set(self.0);
            }
        }
        let _restore = Restore(LIFTED.replace(Arc::as_ptr(&self.0).cast()));
        f()
    }

    fn is_lifted(&self) -> bool {
        LIFTED.get() == Arc::as_ptr(&self.0).cast()
    }

    fn reserve(&self, bytes: usize) -> Result<ReservationCore<A>, AllocError> {
        if bytes != 0 {
            let mut held = self.0.held.load(Relaxed);
            loop {
                let next = held.checked_add(bytes).filter(|&next| next <= self.limit() || self.is_lifted()).ok_or(AllocError)?;
                match self.0.held.compare_exchange(held, next, Relaxed, Relaxed) {
                    Ok(_) => break,
                    Err(cur) => held = cur,
                }
            }
            self.0.account.charge(bytes);
        }
        Ok(ReservationCore { budget: self.clone(), bytes })
    }

    fn release(&self, bytes: usize) {
        if bytes != 0 {
            self.0.held.fetch_sub(bytes, Relaxed);
            self.0.account.credit(bytes);
        }
    }
}

struct ReservationCore<A: AccountSink = Option<AccountRef>> {
    budget: BudgetCore<A>,
    bytes: usize,
}

impl<A: AccountSink> ReservationCore<A> {
    fn bytes(&self) -> usize {
        self.bytes
    }

    fn resize(&mut self, bytes: usize) -> Result<(), AllocError> {
        if bytes > self.bytes {
            self.budget.reserve(bytes - self.bytes)?.commit();
        } else {
            self.budget.release(self.bytes - bytes);
        }
        self.bytes = bytes;
        Ok(())
    }

    fn absorb(&mut self, mut other: ReservationCore<A>) {
        assert!(Arc::ptr_eq(&self.budget.0, &other.budget.0), "a reservation of another budget");
        self.bytes += std::mem::take(&mut other.bytes);
    }

    fn resize_past_limit(&mut self, bytes: usize) {
        if bytes > self.bytes {
            let more = bytes - self.bytes;
            self.budget.0.held.fetch_add(more, Relaxed);
            self.budget.0.account.charge(more);
        } else {
            self.budget.release(self.bytes - bytes);
        }
        self.bytes = bytes;
    }

    fn commit(mut self) {
        self.bytes = 0;
    }
}

impl<A: AccountSink> Drop for ReservationCore<A> {
    fn drop(&mut self) {
        self.budget.release(self.bytes);
    }
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
pub struct Budget(BudgetCore<Option<AccountRef>>);

impl Budget {
    pub fn new(limit: usize) -> Self {
        Self::with_account(limit, None)
    }

    fn with_account(limit: usize, account: Option<AccountRef>) -> Self {
        Self(BudgetCore::new(limit, account))
    }

    /// A world's budget (its heap, registers and globals): its buffers
    /// bypass the global allocator but charge the world's account exactly
    /// once. Unlimited until the world sets its ceiling (`set_limit`).
    pub(crate) fn for_account(account: AccountRef) -> Self {
        Self::with_account((super::COUNT >> 1) as usize, Some(account))
    }

    pub fn held(&self) -> usize {
        self.0.held()
    }

    /// Diagnostic owner total, including globally intercepted allocations.
    /// Reading flushes this thread's delta, not other threads' pending ones.
    pub(crate) fn accounted(&self) -> usize {
        self.0.0.account.map_or_else(|| self.held(), AccountRef::held)
    }

    pub fn limit(&self) -> usize {
        self.0.limit()
    }

    pub fn set_limit(&self, limit: usize) {
        self.0.set_limit(limit);
    }

    /// What the budget may still grow by, for what this thread reserves.
    pub fn headroom(&self) -> usize {
        self.0.headroom()
    }

    /// Run `f` with the limit lifted for what this thread reserves, and
    /// its account's ceiling for what it allocates, then put them back:
    /// for what must not fail halfway (a collection relocating objects),
    /// admitted before. Other threads sharing the budget (the JIT
    /// compiler's) are held to both meanwhile.
    pub(crate) fn lifted<R>(&self, f: impl FnOnce() -> R) -> R {
        self.0.lifted(f)
    }

    /// Reserve before doing work; dropping the reservation gives it back.
    /// Failed reservations leave the budget unchanged. A zero-byte request
    /// is allowed even when an existing allocation exceeds a lowered limit.
    pub fn reserve(&self, bytes: usize) -> Result<Reservation, AllocError> {
        self.0.reserve(bytes).map(Reservation)
    }

    pub(super) fn release(&self, bytes: usize) {
        self.0.release(bytes);
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
        self.0.0.account.filter(|a| !a.entered()).map(AccountRef::enter)
    }
}

/// Bytes reserved in a [`Budget`], released on drop.
#[must_use = "dropping the reservation releases its bytes"]
pub struct Reservation(ReservationCore<Option<AccountRef>>);

impl std::fmt::Debug for Reservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Reservation({} bytes)", self.bytes())
    }
}

impl Reservation {
    pub fn bytes(&self) -> usize {
        self.0.bytes()
    }

    /// Make it `bytes`, reserving or releasing the difference; refused as
    /// a reservation is, leaving it as it was.
    pub(crate) fn resize(&mut self, bytes: usize) -> Result<(), AllocError> {
        self.0.resize(bytes)
    }

    /// Take `other`'s bytes over, without giving them back meanwhile: they
    /// are of the same budget.
    pub(crate) fn absorb(&mut self, other: Reservation) {
        self.0.absorb(other.0);
    }

    /// Make it `bytes` whatever the limit: for what exists already (machine
    /// code larger than reserved for), which charging short would hide.
    pub(crate) fn resize_past_limit(&mut self, bytes: usize) {
        self.0.resize_past_limit(bytes);
    }

    /// The allocation now owns the charge; its allocator releases it.
    pub(super) fn commit(self) {
        self.0.commit();
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

    /// What a collection allocates, its budget lifted, is not refused past
    /// its account's ceiling, however far; the first allocation after it
    /// is, and only that one.
    #[cfg(all(feature = "counting-allocator", any(unix, windows), not(miri)))]
    #[test]
    fn a_lifted_budget_is_not_refused_past_the_ceiling() {
        let account = Account::new().unwrap();
        let budget = Budget::for_account(account.reference());
        let _entered = account.enter();
        account.set_limits(0, account.held() + (1 << 20), Arc::new(|| {}));
        // Past a chunk, which the thread counts at once.
        let held: Vec<Vec<u8>> = budget.lifted(|| (0..32).map(|_| std::hint::black_box(vec![1; 1 << 20])).collect());
        // Seen, else the allocations are elided.
        let small = std::alloc::Layout::new::<u64>();
        let allocated = || std::hint::black_box(unsafe { std::alloc::alloc(small) });
        assert!(allocated().is_null());
        let p = allocated();
        assert!(!p.is_null());
        unsafe { std::alloc::dealloc(p, small) };
        drop(held);
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

    /// A credit of the sign bit reaches the account as `isize::MIN`, which
    /// its packed count takes wrapping rather than overflowing.
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn an_account_takes_back_a_sign_bit_reservation() {
        let account = Account::new().unwrap();
        let held = account.held();
        let budget = Budget::for_account(account.reference());
        budget.set_limit(usize::MAX);
        let sign_bit = budget.reserve(1 << (usize::BITS - 1)).unwrap();
        let more = budget.reserve(1 << 47).unwrap();
        drop(sign_bit);
        drop(more);
        assert_eq!(budget.held(), 0);
        assert_eq!(account.held(), held);
    }

    #[test]
    fn credit_delta_handles_sign_bit_without_negation_overflow() {
        let sign_bit = 1usize << (usize::BITS - 1);
        assert_eq!(charge_delta(sign_bit), isize::MIN);
        assert_eq!(credit_delta(sign_bit), isize::MIN);
        assert_eq!(charge_delta(sign_bit).wrapping_add(credit_delta(sign_bit)), 0);
        assert_eq!(credit_delta(0), 0);
        assert_eq!(credit_delta(4096), -4096);
    }
}

#[cfg(kani)]
mod kani_proofs {
    use std::sync::atomic::AtomicIsize;

    use super::*;

    /// Records every `charge`/`credit` call and accumulates the exact signed
    /// two's-complement delta produced by production `charge_delta`/`credit_delta`
    /// (matching `super::add`'s `AtomicIsize::fetch_add(..., Relaxed)`).
    struct RecordingAccount {
        charges: AtomicUsize,
        credits: AtomicUsize,
        net_delta: AtomicIsize,
    }

    impl RecordingAccount {
        fn new() -> Self {
            Self { charges: AtomicUsize::new(0), credits: AtomicUsize::new(0), net_delta: AtomicIsize::new(0) }
        }

        fn snapshot(&self) -> (usize, usize, isize) {
            (self.charges.load(Relaxed), self.credits.load(Relaxed), self.net_delta.load(Relaxed))
        }

        fn net(&self) -> isize {
            self.net_delta.load(Relaxed)
        }
    }

    impl AccountSink for RecordingAccount {
        fn charge(&self, bytes: usize) {
            self.charges.fetch_add(1, Relaxed);
            self.net_delta.fetch_add(charge_delta(bytes), Relaxed);
        }

        fn credit(&self, bytes: usize) {
            self.credits.fetch_add(1, Relaxed);
            self.net_delta.fetch_add(credit_delta(bytes), Relaxed);
        }
    }

    /// Sequential exhaustive proof over arbitrary limits and byte requests for production
    /// `BudgetCore` with `RecordingAccount` (exercising production `charge_delta` and
    /// `credit_delta`, including `1usize << (usize::BITS - 1)`): `reserve`, `set_limit`,
    /// `absorb`, `resize`, and `Drop` conserve held bytes and signed account deltas exactly,
    /// leave both `held()` and account counters untouched on refusal, always permit zero-byte
    /// reservations even past a lowered limit, and restore `held() == 0` and `net_delta == 0`
    /// once all reservations drop.
    ///
    /// Note: Kani checks the sequential execution model and does not verify multi-threaded
    /// contention (e.g., a concurrent `set_limit` racing with `reserve`'s CAS).
    #[kani::proof]
    #[kani::unwind(2)]
    fn budget_reservation_conservation_and_refusal_atomicity() {
        let limit1: usize = kani::any();
        let limit2: usize = kani::any();
        let a: usize = kani::any();
        let b: usize = kani::any();
        let c: usize = kani::any();

        // Production signed-delta conversion never panics (including at `1 << (BITS - 1)`)
        // and cancels in two's complement for every `usize`.
        let sign_bit = 1usize << (usize::BITS - 1);
        assert!(charge_delta(sign_bit).wrapping_add(credit_delta(sign_bit)) == 0);
        assert!(charge_delta(a).wrapping_add(credit_delta(a)) == 0);

        let budget = BudgetCore::new(limit1, RecordingAccount::new());
        assert!(budget.held() == 0);
        assert!(budget.limit() == limit1);
        assert!(budget.headroom() == limit1);
        assert!(budget.0.account.snapshot() == (0, 0, 0));

        let snap0 = budget.0.account.snapshot();
        let mut r1 = match budget.reserve(a) {
            Ok(r) => {
                assert!(a == 0 || a <= limit1);
                assert!(r.bytes() == a);
                assert!(budget.held() == a);
                assert!(budget.headroom() == limit1.saturating_sub(a));
                assert!(budget.0.account.net() == a as isize);
                assert!(budget.0.account.charges.load(Relaxed) == snap0.0 + usize::from(a != 0));
                r
            }
            Err(_) => {
                assert!(a != 0 && a > limit1);
                assert!(budget.held() == 0);
                assert!(budget.0.account.snapshot() == snap0);
                budget.reserve(0).unwrap()
            }
        };

        // Changing limit never revokes existing reservations, and 0-byte reservations still succeed
        // without charging or crediting the account sink.
        budget.set_limit(limit2);
        assert!(budget.limit() == limit2);
        let snap_zero = budget.0.account.snapshot();
        let zero = budget.reserve(0).unwrap();
        assert!(zero.bytes() == 0);
        assert!(budget.held() == r1.bytes());
        drop(zero);
        assert!(budget.held() == r1.bytes());
        assert!(budget.0.account.snapshot() == snap_zero);

        let before = budget.held();
        let snap1 = budget.0.account.snapshot();
        let r2 = match budget.reserve(b) {
            Ok(r) => {
                assert!(b == 0 || before.checked_add(b).is_some_and(|n| n <= limit2));
                assert!(r.bytes() == b);
                assert!(budget.held() == before + b);
                assert!(budget.0.account.net() == (before + b) as isize);
                assert!(budget.0.account.charges.load(Relaxed) == snap1.0 + usize::from(b != 0));
                r
            }
            Err(_) => {
                assert!(b != 0 && before.checked_add(b).is_none_or(|n| n > limit2));
                assert!(budget.held() == before);
                assert!(budget.0.account.snapshot() == snap1);
                budget.reserve(0).unwrap()
            }
        };

        // `absorb` transfers bytes atomically without touching the account sink.
        let combined = r1.bytes() + r2.bytes();
        let snap_abs = budget.0.account.snapshot();
        r1.absorb(r2);
        assert!(r1.bytes() == combined);
        assert!(budget.held() == combined);
        assert!(budget.0.account.snapshot() == snap_abs);

        // `resize` shrinks unconditionally (crediting the sink) and grows iff within limit
        // without overflow (charging the sink); refusal leaves state and sink untouched.
        let prev = r1.bytes();
        let snap_res = budget.0.account.snapshot();
        match r1.resize(c) {
            Ok(()) => {
                assert!(c <= prev || c <= limit2);
                assert!(r1.bytes() == c);
                assert!(budget.held() == c);
                assert!(budget.0.account.net() == c as isize);
                if c > prev {
                    assert!(budget.0.account.charges.load(Relaxed) == snap_res.0 + 1);
                    assert!(budget.0.account.credits.load(Relaxed) == snap_res.1);
                } else if c < prev {
                    assert!(budget.0.account.charges.load(Relaxed) == snap_res.0);
                    assert!(budget.0.account.credits.load(Relaxed) == snap_res.1 + 1);
                } else {
                    assert!(budget.0.account.snapshot() == snap_res);
                }
            }
            Err(_) => {
                assert!(c > prev && c > limit2);
                assert!(r1.bytes() == prev);
                assert!(budget.held() == prev);
                assert!(budget.0.account.snapshot() == snap_res);
            }
        }

        drop(r1);
        assert!(budget.held() == 0);
        assert!(budget.0.account.net() == 0);
    }

    /// Sequential exhaustive proof over arbitrary limits and non-overflowing sizes for
    /// production `BudgetCore` with `RecordingAccount`: `lifted` lifts only the target
    /// budget (preserving isolation across distinct nested budgets and restoring outer
    /// state on drop), still refuses `usize` addition overflow while lifted,
    /// `resize_past_limit` charges on growth and credits on shrink while conserving
    /// `held()` and signed account deltas, and dropping all reservations restores
    /// `held() == 0` and `net_delta == 0`.
    ///
    /// Precondition / scope: Kani checks the sequential model (no concurrent threads),
    /// and `resize_past_limit` assumes the caller's growth does not wrap `usize::MAX`
    /// (`a + c <= usize::MAX`).
    #[kani::proof]
    #[kani::unwind(2)]
    fn budget_lifted_and_resize_past_limit_invariants() {
        let limit: usize = kani::any();
        let other_limit: usize = kani::any();
        let a: usize = kani::any();
        let b: usize = kani::any();
        let c: usize = kani::any();
        kani::assume(a <= limit);
        kani::assume(a.checked_add(b).is_some());
        kani::assume(a.checked_add(c).is_some());

        let budget = BudgetCore::new(limit, RecordingAccount::new());
        let other = BudgetCore::new(other_limit, RecordingAccount::new());
        let r1 = budget.reserve(a).unwrap();
        assert!(!budget.is_lifted() && !other.is_lifted());

        let mut r2 = budget.lifted(|| {
            assert!(budget.is_lifted());
            assert!(budget.headroom() == usize::MAX);
            // Lifting `budget` does NOT lift a distinct `other` budget.
            assert!(!other.is_lifted());
            assert!(other.headroom() == other_limit);
            if other_limit < usize::MAX {
                assert!(other.reserve(other_limit + 1).is_err());
                assert!(other.held() == 0);
                assert!(other.0.account.snapshot() == (0, 0, 0));
            }
            // Nesting `other.lifted` switches the active lifted budget and restores `budget` on exit.
            other.lifted(|| {
                assert!(other.is_lifted());
                assert!(!budget.is_lifted());
            });
            assert!(budget.is_lifted() && !other.is_lifted());

            // Even while lifted, `usize` overflow in `reserve` must still be refused atomically.
            if a > 0 {
                let snap_ov = budget.0.account.snapshot();
                assert!(budget.reserve(usize::MAX - a + 1).is_err());
                assert!(budget.held() == a);
                assert!(budget.0.account.snapshot() == snap_ov);
            }

            budget.reserve(b).unwrap()
        });
        assert!(!budget.is_lifted() && !other.is_lifted());
        assert!(r2.bytes() == b);
        assert!(budget.held() == a + b);
        assert!(budget.0.account.net() == (a + b) as isize);
        assert!(budget.headroom() == limit.saturating_sub(a + b));

        // After leaving `lifted`, non-zero reservations past the limit are refused again.
        let extra: usize = kani::any();
        kani::assume(extra > 0);
        if (a + b).checked_add(extra).is_none_or(|n| n > limit) {
            let snap_ref = budget.0.account.snapshot();
            assert!(budget.reserve(extra).is_err());
            assert!(budget.held() == a + b);
            assert!(budget.0.account.snapshot() == snap_ref);
        }

        // `resize_past_limit` charges on growth and credits on shrink while conserving `a + c`.
        let snap_rpl = budget.0.account.snapshot();
        r2.resize_past_limit(c);
        assert!(r2.bytes() == c);
        assert!(budget.held() == a + c);
        assert!(budget.0.account.net() == (a + c) as isize);
        if c > b {
            assert!(budget.0.account.charges.load(Relaxed) == snap_rpl.0 + 1);
            assert!(budget.0.account.credits.load(Relaxed) == snap_rpl.1);
        } else if c < b {
            assert!(budget.0.account.charges.load(Relaxed) == snap_rpl.0);
            assert!(budget.0.account.credits.load(Relaxed) == snap_rpl.1 + 1);
        } else {
            assert!(budget.0.account.snapshot() == snap_rpl);
        }

        drop(r2);
        assert!(budget.held() == a);
        assert!(budget.0.account.net() == a as isize);
        drop(r1);
        assert!(budget.held() == 0);
        assert!(budget.0.account.net() == 0);
    }
}
