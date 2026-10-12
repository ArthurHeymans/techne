//! Explicit reservations and standard-library allocator-aware storage.

use std::{
    alloc::{Allocator, Layout},
    sync::{Arc, Barrier},
};

use techne_vm::alloc::Budget;

#[test]
fn reservations_refuse_growth_and_release_on_drop() {
    let budget = Budget::new(64);
    let held = budget.reserve(48).unwrap();
    assert_eq!(held.bytes(), 48);
    assert!(budget.reserve(17).is_err());
    assert_eq!(budget.held(), 48);
    drop(budget.reserve(16).unwrap());
    assert_eq!(budget.held(), 48);
    budget.set_limit(32);
    assert_eq!(budget.limit(), 32);
    assert!(budget.reserve(1).is_err());
    drop(budget.reserve(0).unwrap());
    assert_eq!(budget.held(), 48);
    drop(held);
    assert_eq!(budget.held(), 0);
    assert!(budget.reserve(32).is_ok());
}

#[test]
fn reservation_overflow_is_refused_without_changing_the_budget() {
    let budget = Budget::new(usize::MAX);
    let held = budget.reserve(usize::MAX).unwrap();
    assert!(budget.reserve(1).is_err());
    assert_eq!(budget.held(), usize::MAX);
    drop(held);
    assert_eq!(budget.held(), 0);
}

#[test]
fn vectors_charge_only_the_growth_and_keep_data_on_refusal() {
    let budget = Budget::new(16);
    let mut bytes = Vec::new_in(budget.clone());
    bytes.try_reserve_exact(8).unwrap();
    bytes.extend_from_slice(b"abcdefgh");
    assert_eq!(budget.held(), 8);
    bytes.try_reserve_exact(8).unwrap();
    assert_eq!(budget.held(), 16);
    assert!(bytes.try_reserve_exact(16).is_err());
    assert_eq!(bytes.as_slice(), b"abcdefgh");
    assert_eq!(budget.held(), 16);
    budget.set_limit(1);
    bytes.truncate(4);
    bytes.shrink_to_fit();
    assert_eq!(budget.held(), 4);
    drop(bytes);
    assert_eq!(budget.held(), 0);
}

#[test]
fn allocator_preserves_alignment_and_zeroes_only_new_bytes() {
    let budget = Budget::new(512);
    let old = Layout::from_size_align(256, 4096).unwrap();
    let allocation = budget.allocate_zeroed(old).unwrap();
    let ptr = allocation.cast::<u8>();
    assert_eq!(ptr.as_ptr() as usize % old.align(), 0);
    // SAFETY: the allocation is live and holds 256 bytes.
    unsafe {
        let bytes = std::slice::from_raw_parts_mut(ptr.as_ptr(), old.size());
        assert!(bytes.iter().all(|&b| b == 0));
        bytes.fill(0xa5);
    }
    let grown = Layout::from_size_align(512, 8192).unwrap();
    // SAFETY: the live allocation has layout old, and grown is larger.
    let allocation = unsafe { budget.grow_zeroed(ptr, old, grown) }.unwrap();
    let ptr = allocation.cast::<u8>();
    assert_eq!(ptr.as_ptr() as usize % grown.align(), 0);
    assert_eq!(budget.held(), 512);
    // SAFETY: grow succeeded, returning 512 live bytes.
    unsafe {
        let bytes = std::slice::from_raw_parts(ptr.as_ptr(), grown.size());
        assert!(bytes[..256].iter().all(|&b| b == 0xa5));
        assert!(bytes[256..].iter().all(|&b| b == 0));
    }
    let smaller = Layout::from_size_align(64, 256).unwrap();
    budget.set_limit(1);
    // SAFETY: the live allocation has layout grown, and smaller is smaller.
    let allocation = unsafe { budget.shrink(ptr, grown, smaller) }.unwrap();
    assert_eq!(allocation.cast::<u8>().as_ptr() as usize % smaller.align(), 0);
    assert_eq!(budget.held(), 64);
    // SAFETY: shrink succeeded; the allocation now has layout smaller.
    unsafe { budget.clone().deallocate(allocation.cast(), smaller) };
    assert_eq!(budget.held(), 0);
}

#[test]
fn zero_sized_allocations_need_no_room() {
    let budget = Budget::new(0);
    let layout = Layout::from_size_align(0, 64).unwrap();
    let allocation = budget.allocate(layout).unwrap();
    assert_eq!(allocation.cast::<u8>().as_ptr() as usize % 64, 0);
    assert_eq!(budget.held(), 0);
    // SAFETY: this is the live zero-sized allocation with its original layout.
    unsafe { budget.deallocate(allocation.cast(), layout) };
    assert!(budget.reserve(1).is_err());
}

#[test]
fn allocator_ownership_survives_the_original_budget_and_another_thread() {
    let budget = Budget::new(1024);
    let observed = budget.clone();
    let value = Box::new_in(42_u64, budget);
    assert_eq!(observed.held(), 8);
    std::thread::spawn(move || {
        assert_eq!(*value, 42);
        drop(value);
    })
    .join()
    .unwrap();
    assert_eq!(observed.held(), 0);
}

#[test]
fn concurrent_reservations_do_not_oversubscribe() {
    let budget = Budget::new(64);
    let barrier = Arc::new(Barrier::new(9));
    let workers: Vec<_> = (0..8)
        .map(|_| {
            let (budget, barrier) = (budget.clone(), barrier.clone());
            std::thread::spawn(move || {
                let reserved = budget.reserve(16);
                let accepted = reserved.is_ok();
                barrier.wait();
                barrier.wait();
                drop(reserved);
                accepted
            })
        })
        .collect();
    barrier.wait();
    assert_eq!(budget.held(), 64);
    barrier.wait();
    assert_eq!(workers.into_iter().map(|w| w.join().unwrap()).filter(|&accepted| accepted).count(), 4);
    assert_eq!(budget.held(), 0);
}
