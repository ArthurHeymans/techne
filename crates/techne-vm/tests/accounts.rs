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
