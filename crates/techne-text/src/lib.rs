//! Text documents for Techne (EDITOR.md, sections 4 and 5): byte-offset
//! changes, revision-checked transactions by actors, undo that refuses when
//! other actors' edits conflict, and an edit journal that recovers every
//! acknowledged transaction after a crash.

pub mod change;
pub mod document;
pub mod journal;
pub mod motion;
pub mod selection;

pub use change::{Assoc, ChangeSet, Conflict, EditError, Op};
pub use document::{Actor, ApplyError, Document, Group, Kind, OpenError, RebaseError, Recovery, Revision, Transaction, UndoError};
pub use selection::{Range, Selection};
