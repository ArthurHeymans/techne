//! What a VM bug would hand a guest, as primitives, for the tests of worlds
//! as a boundary between tenants (`tests/hostile.rs`): a value made from
//! any bits, and any word of its own cage read or written. A world given
//! these may corrupt its own heap at will; nothing it does may reach
//! another world, nor end the process. Only with the `hostile` feature:
//! never in a build that runs guests.

use crate::{
    alloc, num,
    value::Value,
    vm::{Error, Vm},
};

type R = Result<Value, Error>;

fn arg(vm: &Vm, args: usize, i: usize) -> Value {
    vm.regs[args + i]
}

fn word(vm: &Vm, args: usize, i: usize, who: &str) -> Result<u64, Error> {
    Ok(num::integer(arg(vm, args, i), who)? as u64)
}

/// The word at `offset` in `vm`'s cage, whatever `offset`'s other bits.
fn cage_word(vm: &Vm, offset: u64) -> *mut u64 {
    alloc::confine(vm.heap.cage_base(), offset as usize)
}

/// Give `vm`'s guests the primitives of a VM bug: `%bits`, `%forge`,
/// `%cage-ref`, `%cage-set!` and `%cage-top`.
pub fn install(vm: &mut Vm) {
    let _entered = vm.charging();
    crate::natives! { vm;
        /// Return the bits of OBJ, as a signed integer.
        "(%bits obj)" => |vm: &mut Vm, a, _| -> R { let bits = arg(vm, a, 0).bits(); Ok(vm.make_int_raw(bits as i64)) };
        /// Return the value whose bits are BITS, whatever they are.
        "(%forge bits)" => |vm: &mut Vm, a, _| -> R { Ok(Value::from_bits(word(vm, a, 0, "%forge")?)) };
        /// Return the word at OFFSET in this world's cage.
        "(%cage-ref offset)" => |vm: &mut Vm, a, _| -> R {
            let p = cage_word(vm, word(vm, a, 0, "%cage-ref")?);
            // SAFETY: a word of the world's own cage, mapped; what it holds
            // is the guest's to corrupt.
            let w = unsafe { p.read_volatile() };
            Ok(vm.make_int_raw(w as i64)) };
        /// Set the word at OFFSET in this world's cage to WORD.
        "(%cage-set! offset word)" => |vm: &mut Vm, a, _| -> R {
            let p = cage_word(vm, word(vm, a, 0, "%cage-set!")?);
            let w = word(vm, a, 1, "%cage-set!")?;
            // SAFETY: as for `%cage-ref`.
            unsafe { p.write_volatile(w) };
            Ok(Value::VOID) };
        /// Return the offset past every span this world's heap has had.
        "(%cage-top)" => |vm: &mut Vm, _, _| -> R { let top = vm.heap.cage().top(); Ok(vm.make_int_raw(top as i64)) };
    }
}

/// Make the pages `vm`'s heap has had inaccessible (`on`), or accessible
/// again: any access to them meanwhile, from any world, faults. Nothing of
/// `vm` may run, collect or be dropped while they are.
#[cfg(target_os = "linux")]
pub fn protect(vm: &Vm, on: bool) {
    let len = vm.heap.cage().top().next_multiple_of(4096);
    let prot = if on { libc::PROT_NONE } else { libc::PROT_READ | libc::PROT_WRITE };
    // SAFETY: the pages of `vm`'s own cage, mapped; nothing uses them while
    // they are inaccessible, as the caller promises.
    let r = unsafe { libc::mprotect(vm.heap.cage_base().cast(), len, prot) };
    assert_eq!(r, 0, "mprotect: {}", std::io::Error::last_os_error());
}

/// Collect `vm` whole, checking its heap before and after: a heap broken by
/// another world's doing aborts the process with a report.
pub fn verify(vm: &mut Vm) {
    let verify = std::mem::replace(&mut vm.heap.verify, true);
    vm.full_collect_raw();
    vm.heap.verify = verify;
}
