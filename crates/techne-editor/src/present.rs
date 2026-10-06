//! The presentation protocol between the runtime and a frontend (EDITOR.md,
//! section 6), for one view of a text document.
//!
//! Everything here is plain data: no handles to runtime objects and no
//! callbacks, even with frontend and runtime in one process. A snapshot is
//! an immutable text (a rope shares its chunks, so it is cheap to send) at a
//! document revision; the frontend lays out only what it shows. Input that
//! the frontend resolved against a snapshot (a click, a scroll) names that
//! snapshot's revision, and the runtime maps it to the current text or
//! refuses it.
//!
//! Rows of a text document are its lines, addressed by source position;
//! keyed rows and deltas come with structured views and lenses (PLAN.md,
//! Stage 1, slice 5).

use std::time::Instant;

use techne_text::ropey::Rope;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorShape {
    Bar,
    Block,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    /// Increases with every snapshot of the view.
    pub id: u64,
    pub revision: u64,
    pub text: Rope,
    /// (anchor, head) byte positions; carets are empty ranges.
    pub selections: Vec<(usize, usize)>,
    pub primary: usize,
    pub cursor: CursorShape,
    /// The scroll anchor: a position on the first visible visual line.
    pub scroll: usize,
    /// The status line, as Lisp composed it.
    pub status: String,
    /// When the inputs this snapshot answers were made, so the frontend can
    /// measure input to frame.
    pub answers: Vec<Instant>,
}

impl Snapshot {
    pub fn head(&self) -> usize {
        self.selections[self.primary].1
    }
}

#[derive(Clone, Debug)]
pub enum Input {
    /// A key in Emacs notation ("a", "C-x", "M-<", "RET", "<left>").
    Key { key: String, at: Instant },
    /// Put the caret at `pos` of the text at `revision` (a click), or extend
    /// the selection to it.
    Click { revision: u64, pos: usize, extend: bool, at: Instant },
    /// Scroll so that `anchor` of the text at `revision` is on the first
    /// visible line.
    Scroll { revision: u64, anchor: usize },
    /// The frontend is closing.
    Close,
}

#[derive(Debug)]
pub enum Output {
    Snapshot(Box<Snapshot>),
    /// The session asked to quit.
    Quit,
}
