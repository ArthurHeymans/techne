//! Fallback allocator metadata after Box/Vec narrow the payload pointer.
//! Run under default Miri; do not require permissive provenance or Tree Borrows.
//! These exercise the global interception itself, so they only exist
//! with the `counting-allocator` feature.
#![cfg(feature = "counting-allocator")]

use std::alloc::{Layout, alloc, alloc_zeroed, dealloc, realloc};
use techne_vm::alloc::Account;

#[test]
fn boxed_payloads_leave_the_trailer_accessible_to_the_allocator() {
    let account = Account::new().unwrap();
    let _entered = account.enter();
    let before = account.held();
    // Small sizes place the tag outside the range of Box's unique retag.
    let mut tiny = std::hint::black_box(Box::new(1_u8));
    let mut odd = std::hint::black_box(Box::new([2_u8; 7]));
    *tiny = 3;
    odd[6] = 4;
    assert_eq!(*tiny, 3);
    assert_eq!(odd[6], 4);
    assert!(account.held() > before);
    drop((tiny, odd));
    assert_eq!(account.held(), before);
}

#[test]
fn vector_growth_and_shrink_preserve_the_original_account() {
    let (owner, other) = (Account::new().unwrap(), Account::new().unwrap());
    let mut bytes = {
        let _entered = owner.enter();
        let mut bytes = Vec::with_capacity(7);
        bytes.extend_from_slice(b"payload");
        bytes
    };
    let before = owner.held();
    let _other = other.enter();
    let other_before = other.held();
    bytes.reserve_exact(128);
    assert_eq!(bytes.as_slice(), b"payload");
    assert!(owner.held() > before);
    assert_eq!(other.held(), other_before);
    bytes.truncate(1);
    bytes.shrink_to_fit();
    assert_eq!(bytes.as_slice(), b"p");
    assert!(owner.held() < before);
    drop(bytes);
    assert_eq!(owner.held(), 0);
    assert_eq!(other.held(), other_before);
}

#[test]
fn boxes_freed_on_another_thread_or_after_retirement_keep_their_owner() {
    let owner = Account::new().unwrap();
    let boxed = {
        let _entered = owner.enter();
        std::hint::black_box(Box::new([0_u8; 7]))
    };
    assert!(owner.held() > 0);
    std::thread::spawn(move || drop(boxed)).join().unwrap();
    assert_eq!(owner.held(), 0);

    let boxed = {
        let _entered = owner.enter();
        std::hint::black_box(Box::new([0_u8; 7]))
    };
    drop(owner);
    let replacement = Account::new().unwrap();
    let _entered = replacement.enter();
    let kept = std::hint::black_box(Box::new([1_u8; 17]));
    let before = replacement.held();
    drop(boxed);
    assert_eq!(replacement.held(), before);
    drop(kept);
    assert_eq!(replacement.held(), 0);
}

#[test]
fn zeroing_alignment_and_failed_resize_preserve_metadata() {
    let account = Account::new().unwrap();
    let _entered = account.enter();
    let before = account.held();
    for zeroed in [false, true] {
        for align in [1, 4, 8, 64] {
            let layout = Layout::from_size_align(7, align).unwrap();
            // SAFETY: nonzero valid layout. Deallocated once below.
            let pointer = unsafe { if zeroed { alloc_zeroed(layout) } else { alloc(layout) } };
            assert!(!pointer.is_null());
            assert_eq!(pointer.addr() % align, 0);
            // SAFETY: the requested payload holds seven initialized bytes
            // after fill; this exclusive slice excludes the private trailer.
            unsafe {
                let payload = std::slice::from_raw_parts_mut(pointer, 7);
                if zeroed {
                    assert_eq!(payload, &[0; 7]);
                }
                payload.fill(0xa5);
            }
            let held = account.held();
            // Even aligned to 1, no room for the private trailer: the
            // allocator must refuse without consuming the old allocation.
            let impossible = (isize::MAX as usize) & !(align - 1);
            // SAFETY: original live layout; new size rounded to align fits
            // isize::MAX, as required by realloc. Null leaves pointer valid.
            assert!(unsafe { realloc(pointer, layout, impossible) }.is_null());
            assert_eq!(account.held(), held);
            // SAFETY: failed realloc preserved the live payload and layout.
            unsafe {
                assert_eq!(std::slice::from_raw_parts(pointer, 7), &[0xa5; 7]);
                dealloc(pointer, layout);
            }
            assert_eq!(account.held(), before);
        }
    }
}
