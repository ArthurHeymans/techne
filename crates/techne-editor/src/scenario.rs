//! A scripted session that every frontend's tests run (PLAN.md, Stage 1,
//! slice 3): the same steps through any frontend, with clicks resolved by
//! its own geometry, must leave the semantic state they leave through the
//! runtime alone. Key profiles, undo units and edits all happen in the
//! runtime, so frontends agreeing with it agree with each other.

use std::time::Instant;

use techne_text::Document;

use crate::{present::Input, runtime::Runtime};

/// Text with wide characters, a tab, combining marks and a line that wraps
/// in any narrow frontend.
pub const TEXT: &str = "first line\n\tindented 日本語 text\ncombining e\u{301} and 👩\u{200d}💻\n\
a long line that wraps around when the frontend is narrow enough to need it, again and again\nlast";

#[derive(Clone, Copy, Debug)]
pub enum Step {
    /// A key in Emacs notation.
    Key(&'static str),
    /// Type the characters.
    Text(&'static str),
    /// Click on the `nth` occurrence of a string in the current text, at
    /// that many bytes into it; shift-click extends.
    Click { on: &'static str, offset: usize, extend: bool },
    /// The frontend changes size (columns, or pixels).
    Resize { width: u32, height: u32 },
}

pub const STEPS: &[Step] = &[
    Step::Key("M-f"),
    Step::Click { on: "日本語", offset: 3, extend: false },
    Step::Text("x"),
    Step::Resize { width: 30, height: 12 },
    Step::Click { on: "again and", offset: 6, extend: false },
    Step::Key("C-SPC"),
    Step::Key("M-f"),
    Step::Key("M-f"),
    Step::Key("C-w"),
    Step::Click { on: "e\u{301}", offset: 0, extend: false },
    Step::Click { on: "👩", offset: 0, extend: true },
    Step::Key("M-w"),
    Step::Key("M->"),
    Step::Key("RET"),
    Step::Key("C-y"),
    Step::Key("C-/"),
    Step::Text("end"),
];

/// What frontends must agree on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub text: String,
    pub selections: Vec<(usize, usize)>,
    pub revision: u64,
}

pub fn state(rt: &mut Runtime) -> State {
    let s = rt.snapshot();
    State { text: s.text.to_string(), selections: s.selections, revision: s.revision }
}

/// The position a click step means in `text`.
pub fn click_target(text: &str, on: &str, offset: usize) -> usize {
    text.find(on).unwrap_or_else(|| panic!("{on:?} is not in the text")) + offset
}

/// The state after the steps through the runtime alone, clicks made on the
/// positions they mean.
pub fn expected(profile: &str) -> State {
    let mut rt = Runtime::with_document(Document::new(TEXT), profile).expect("a runtime");
    for step in STEPS {
        let at = Instant::now();
        match *step {
            Step::Key(k) => {
                rt.handle(Input::Key { key: k.into(), at });
            }
            Step::Text(t) => {
                for c in t.chars() {
                    rt.handle(Input::Key { key: c.to_string(), at });
                }
            }
            Step::Click { on, offset, extend } => {
                let s = rt.snapshot();
                let pos = click_target(&s.text.to_string(), on, offset);
                rt.handle(Input::Click { revision: s.revision, pos, extend, at });
            }
            Step::Resize { .. } => {}
        }
    }
    state(&mut rt)
}
