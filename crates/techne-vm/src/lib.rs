//! Prototype Lisp runtime: NaN-boxed values, a generational copying GC and a
//! register-based bytecode interpreter, using Steel's parser as front end.
pub mod api;
pub mod builtins;
pub mod code;
pub mod compiler;
pub mod expand;
pub mod heap;
pub mod num;
pub mod reader;
pub mod repl;
pub mod stdlib;
pub mod tasks;
pub mod value;
pub mod vm;

pub const PRELUDE: &str = include_str!("prelude.scm");
