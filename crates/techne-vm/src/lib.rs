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
pub mod doc;
pub mod expand;
pub mod heap;
pub mod jit;
pub mod library;
pub mod num;
pub mod ports;
pub mod reader;
pub mod repl;
pub mod stdlib;
pub mod tasks;
pub mod value;
pub mod vm;

pub const PRELUDE: &str = include_str!("prelude.scm");
/// Where the prelude is, for help and find-definition to show its source.
pub const PRELUDE_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/prelude.scm");
