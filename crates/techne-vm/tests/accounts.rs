//! The allocator's accounts (`techne_vm::alloc`), alone in this test
//! binary so that no other test takes their slots meanwhile.

use techne_vm::alloc::Account;

const MB: usize = 1 << 20;

/// A slot used by as many accounts as it has generations is not used
/// again: a block of its first account, freed late, changes no account.
#[test]
fn slots_retire_before_their_generation_comes_back() {
    let first = Account::new().unwrap();
    // `black_box`: an allocation nothing looks at may be left out.
    let block = {
        let _charged = first.enter();
        std::hint::black_box(vec![1u8; MB])
    };
    drop(first);
    // Each new account takes the slot given back last.
    for _ in 0..u16::MAX {
        drop(Account::new().unwrap());
    }
    let account = Account::new().unwrap();
    let kept = {
        let _charged = account.enter();
        std::hint::black_box(vec![1u8; 2 * MB])
    };
    let held = account.held();
    drop(std::hint::black_box(block));
    assert_eq!(account.held(), held);
    drop(kept);
}

/// Limits as large as can be are no limits: nothing ends the program.
#[test]
fn the_largest_limits_end_nothing() {
    let account = Account::new().unwrap();
    // Past what a count holds: not cut down to its low bits.
    let huge = (1 << 48) + MB;
    account.set_limits(huge, huge, std::sync::Arc::new(|| {}));
    let block = {
        let _charged = account.enter();
        std::hint::black_box(vec![1u8; 32 * MB])
    };
    assert!(account.held() >= 32 * MB);
    drop(block);
}

/// Replacing an account's waker drops the one before outside the lock: a
/// waker holding another account, which takes it when dropped, does not
/// deadlock.
#[test]
fn replacing_a_waker_holding_an_account() {
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let (account, held) = (Account::new().unwrap(), Account::new().unwrap());
        account.set_limits(
            MB,
            0,
            std::sync::Arc::new(move || {
                let _ = &held;
            }),
        );
        account.set_limits(MB, 0, std::sync::Arc::new(|| {}));
        done.send(()).unwrap();
    });
    assert!(finished.recv_timeout(std::time::Duration::from_secs(5)).is_ok(), "deadlocked");
}
