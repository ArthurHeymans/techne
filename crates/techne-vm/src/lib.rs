//! Prototype Lisp runtime: NaN-boxed values, a generational copying GC and a
//! register-based bytecode interpreter.
//!
//! The safe API does not reach the heap's memory. A world's heap is not
//! replaceable:
//!
//! ```compile_fail,E0616
//! let mut vm = techne_vm::vm::Vm::new();
//! let _ = &mut vm.heap;
//! ```
//!
//! nor is an object's address, or a value made from one:
//!
//! ```compile_fail,E0599
//! let mut vm = techne_vm::vm::Vm::new();
//! let v = vm.eval_source("(list 1 2)").unwrap();
//! let _ = v.as_ptr();
//! ```
//!
//! and the collector's raw entry points are the VM's own:
//!
//! ```compile_fail,E0624
//! let vm = techne_vm::vm::Vm::new();
//! let _ = vm.heap().identity_hash(std::ptr::null_mut());
//! ```
// The raw value type and the VM's internals stay crate-private: a public
// item that reaches one would let safe code outside name it.
#![warn(unnameable_types)]
// The macros refer to this crate by name, here as elsewhere.
extern crate self as techne_vm;

pub use techne_vm_macros::{document, procedures};
// Raw natives (arguments as values in registers) are the VM's own.
pub(crate) use techne_vm_macros::natives;

pub mod alloc;
pub mod api;
pub(crate) mod builtins;
pub(crate) mod bytes;
pub mod cli;
pub(crate) mod code;
pub mod compiler;
pub mod complete;
pub(crate) mod complex;
pub(crate) mod crypto;
pub(crate) mod cursors;
pub mod data;
pub(crate) mod dict;
pub mod doc;
pub(crate) mod expand;
pub mod heap;
pub(crate) mod jit;
pub(crate) mod json;
pub mod library;
pub(crate) mod num;
pub(crate) mod ports;
pub mod reader;
pub mod regexp;
pub mod repl;
pub mod stdlib;
pub mod stop;
pub mod tasks;
pub(crate) mod value;
pub mod vm;
pub(crate) mod watchdog;

pub(crate) const STACK_RED_ZONE: usize = 128 * 1024;

/// A fresh stack segment's size.
const STACK_SEGMENT: usize = 2 * 1024 * 1024;

/// Run `f`, a step of a recursion over nested data or code, on a fresh
/// stack segment when little of the current one is left: data may nest up
/// to `reader::MAX_DEPTH`, frames are large in debug builds, and expanded
/// code nests as deep as macros make it. The segment, which the allocator
/// does not see, is counted for the account charged.
#[inline]
pub(crate) fn nested<R>(f: impl FnOnce() -> R) -> R {
    match stacker::remaining_stack() {
        Some(left) if left >= STACK_RED_ZONE => f(),
        _ => {
            let _segment = alloc::take(STACK_SEGMENT);
            stacker::grow(STACK_SEGMENT, f)
        }
    }
}

/// `nested`, kept out of line for callers that need it only for deep data.
#[cold]
#[inline(never)]
pub(crate) fn deep<R>(f: impl FnOnce() -> R) -> R {
    nested(f)
}

pub const PRELUDE: &str = include_str!("prelude.scm");
/// Where the prelude is, for help and find-definition to show its source.
pub const PRELUDE_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/prelude.scm");

#[cfg(test)]
mod tests {
    use super::alloc;

    /// A stack segment `nested` grows is counted for the account charged
    /// while it is used.
    #[test]
    fn stack_segments_are_counted() {
        fn deepest(depth: usize, account: &alloc::Account) -> usize {
            let frame = std::hint::black_box([0u8; 4096]);
            if depth == 0 {
                return account.held() + frame[0] as usize;
            }
            super::nested(|| deepest(depth - 1, account))
        }
        let account = alloc::Account::new().unwrap();
        let _entered = account.enter();
        let before = account.held();
        // Some 4 MB of frames: two segments at least.
        let deep = deepest(1000, &account);
        assert!(deep >= before + 2 * super::STACK_SEGMENT, "{before} then {deep}");
        assert!(account.held() < before + super::STACK_SEGMENT, "given back");
    }
}
