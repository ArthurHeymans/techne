//! Positions and changes as a language server counts them: lines from 0,
//! columns in the position encoding the two sides agreed on (UTF-16 code
//! units unless the server takes UTF-8 bytes), over the rope without
//! making the text one string.

use techne_text::{ChangeSet, ropey::Rope};

use crate::edits;

/// How columns are counted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf16,
}

impl Encoding {
    /// The encoding LSP names `name`: "utf-8", else UTF-16, the default.
    pub fn named(name: Option<&str>) -> Encoding {
        match name {
            Some("utf-8") => Encoding::Utf8,
            _ => Encoding::Utf16,
        }
    }

    fn width(self, c: char) -> usize {
        match self {
            Encoding::Utf8 => c.len_utf8(),
            Encoding::Utf16 => c.len_utf16(),
        }
    }
}

/// Byte `pos` of `rope` as (line, column).
pub fn position(rope: &Rope, pos: usize, encoding: Encoding) -> (usize, usize) {
    // Inside a character is at its start.
    let pos = rope.char_to_byte(rope.byte_to_char(pos.min(rope.len_bytes())));
    let line = rope.byte_to_line(pos);
    let start = rope.line_to_byte(line);
    let column = match encoding {
        Encoding::Utf8 => pos - start,
        Encoding::Utf16 => rope.byte_slice(start..pos).chars().map(char::len_utf16).sum(),
    };
    (line, column)
}

/// The byte of `rope` at `line` and `column`: a column past its line's
/// end is the end, one inside a character before it, a line past the
/// last the text's end.
pub fn offset(rope: &Rope, line: usize, column: usize, encoding: Encoding) -> usize {
    if line >= rope.len_lines() {
        return rope.len_bytes();
    }
    let mut units = 0;
    let mut bytes = rope.line_to_byte(line);
    for c in rope.line(line).chars() {
        if units + encoding.width(c) > column || c == '\n' || c == '\r' {
            break;
        }
        units += encoding.width(c);
        bytes += c.len_utf8();
    }
    bytes
}

/// One content change: the range it replaces, (line, column) at each end,
/// and its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub start: (usize, usize),
    pub end: (usize, usize),
    pub text: String,
}

/// `changes` made to `text`, as content changes: each range in the text
/// the ones before it left, as a server applies them.
pub fn changes(mut text: Rope, changes: &[ChangeSet], encoding: Encoding) -> Vec<Change> {
    let mut out = Vec::new();
    for c in changes {
        for step in edits::steps(c) {
            // An end between a CR and its LF is no position LSP has
            // (columns end before the line's break): the change takes
            // in the whole break, and gives back the half it kept.
            let between = |at: usize| at > 0 && at < text.len_bytes() && text.byte(at - 1) == b'\r' && text.byte(at) == b'\n';
            let (mut start, mut end, mut replacement) = (step.start, step.end, step.text.to_string());
            if between(start) {
                start -= 1;
                replacement.insert(0, '\r');
            }
            if between(end) {
                end += 1;
                replacement.push('\n');
            }
            out.push(Change { start: position(&text, start, encoding), end: position(&text, end, encoding), text: replacement });
            edits::apply(&mut text, &step);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_both_ways() {
        let rope = Rope::from_str("ab\ncdéf\na😀b\r\n");
        for (enc, at_f, after_smiley) in [(Encoding::Utf16, 3, 3), (Encoding::Utf8, 4, 5)] {
            assert_eq!(position(&rope, 7, enc), (1, at_f));
            assert_eq!(offset(&rope, 1, at_f, enc), 7);
            assert_eq!(position(&rope, 14, enc), (2, after_smiley));
            assert_eq!(offset(&rope, 2, after_smiley, enc), 14);
            // Inside the smiley: before it.
            assert_eq!(offset(&rope, 2, 2, enc), 10);
            // Past a CRLF line's text: its end, before the CR.
            assert_eq!(offset(&rope, 2, 99, enc), 15);
            assert_eq!(offset(&rope, 9, 0, enc), rope.len_bytes());
        }
    }

    #[test]
    fn a_change_inside_a_line_break_takes_in_all_of_it() {
        let before = Rope::from_str("a\r\nb");
        // The CR alone deleted: the server is told the break became LF.
        let c = ChangeSet::from_edits(4, [(1..2, "")]).unwrap();
        assert_eq!(changes(before.clone(), &[c], Encoding::Utf16), [Change { start: (0, 1), end: (1, 0), text: "\n".into() }]);
        // Typed between the CR and the LF.
        let c = ChangeSet::from_edits(4, [(2..2, "x")]).unwrap();
        assert_eq!(changes(before.clone(), &[c], Encoding::Utf16), [Change { start: (0, 1), end: (1, 0), text: "\rx\n".into() }]);
        // A position inside a character is its start, not a panic.
        assert_eq!(position(&Rope::from_str("é"), 1, Encoding::Utf16), (0, 0));
    }

    #[test]
    fn changes_in_the_text_the_ones_before_left() {
        let before = Rope::from_str("one\ntwo\n");
        let first = ChangeSet::from_edits(8, [(0..3, "uno"), (4..4, "é")]).unwrap();
        let mut mid = before.clone();
        first.apply(&mut mid);
        let second = ChangeSet::from_edits(mid.len_bytes(), [(4..6, "")]).unwrap();
        let got = changes(before, &[first, second], Encoding::Utf16);
        assert_eq!(
            got,
            [
                Change { start: (0, 0), end: (0, 3), text: "uno".into() },
                Change { start: (1, 0), end: (1, 0), text: "é".into() },
                Change { start: (1, 0), end: (1, 1), text: String::new() },
            ]
        );
    }
}
