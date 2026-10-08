//! Prototype Lisp runtime: NaN-boxed values, a generational copying GC and a
//! register-based bytecode interpreter.
// The macros refer to this crate by name, here as elsewhere.
extern crate self as techne_vm;

pub use techne_vm_macros::{document, natives, procedures};

pub mod api;
pub mod builtins;
pub mod bytes;
pub mod code;
pub mod compiler;
pub mod complete;
pub mod complex;
pub mod cursors;
pub mod doc;
pub mod expand;
pub mod heap;
pub mod jit;
pub mod library;
pub mod num;
pub mod ports;
pub mod reader;
pub mod regexp;
pub mod repl;
pub mod stdlib;
pub mod tasks;
pub mod value;
pub mod vm;

pub(crate) const STACK_RED_ZONE: usize = 128 * 1024;

/// Run `f`, a step of a recursion over nested data or code, on a fresh
/// stack segment when little of the current one is left: data may nest up
/// to `reader::MAX_DEPTH`, and frames are large in debug builds.
#[inline]
pub(crate) fn nested<R>(f: impl FnOnce() -> R) -> R {
    stacker::maybe_grow(STACK_RED_ZONE, 2 * 1024 * 1024, f)
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
