//! Motions and text objects over a rope (EDITOR.md, section 4). Positions are
//! byte offsets on grapheme boundaries; every function returns a position or
//! a range and never edits.
//!
//! Lines here are logical lines, ended by LF as in Emacs (a CRLF ends one
//! too, and `line_end` stops before it). Vertical motion by visual lines needs layout
//! and belongs to frontends; `line_down` serves headless use.

use std::ops::Range;

use ropey::{Rope, RopeSlice};
use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete};

pub fn next_grapheme(text: &Rope, pos: usize) -> usize {
    boundary(text.slice(..), pos, true)
}

pub fn prev_grapheme(text: &Rope, pos: usize) -> usize {
    boundary(text.slice(..), pos, false)
}

fn boundary(text: RopeSlice, pos: usize, forward: bool) -> usize {
    let len = text.len_bytes();
    let (mut chunk, mut chunk_start, _, _) = text.chunk_at_byte(pos);
    let mut cursor = GraphemeCursor::new(pos, len, true);
    loop {
        let step = if forward { cursor.next_boundary(chunk, chunk_start) } else { cursor.prev_boundary(chunk, chunk_start) };
        match step {
            Ok(None) => return if forward { len } else { 0 },
            Ok(Some(n)) => return n,
            Err(GraphemeIncomplete::NextChunk) => {
                chunk_start += chunk.len();
                chunk = text.chunk_at_byte(chunk_start).0;
            }
            Err(GraphemeIncomplete::PrevChunk) => {
                let (c, start, _, _) = text.chunk_at_byte(chunk_start - 1);
                (chunk, chunk_start) = (c, start);
            }
            Err(GraphemeIncomplete::PreContext(n)) => {
                let (c, start, _, _) = text.chunk_at_byte(n - 1);
                cursor.provide_context(c, start);
            }
            Err(e) => unreachable!("grapheme cursor: {e:?}"),
        }
    }
}

/// How characters group into words. `None` separates words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Words {
    /// Emacs: words are letters and digits; everything else separates.
    Emacs,
    /// Vim: runs of word characters (letters, digits, `_`) and runs of other
    /// non-blank characters are both words; blanks separate.
    Vim,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Word,
    Punct,
}

impl Words {
    fn class(self, c: char) -> Option<Class> {
        match self {
            Words::Emacs => c.is_alphanumeric().then_some(Class::Word),
            Words::Vim if c.is_whitespace() => None,
            Words::Vim if c.is_alphanumeric() || c == '_' => Some(Class::Word),
            Words::Vim => Some(Class::Punct),
        }
    }
}

fn char_at(text: &Rope, pos: usize) -> Option<char> {
    (pos < text.len_bytes()).then(|| text.char(text.byte_to_char(pos)))
}

fn char_before(text: &Rope, pos: usize) -> Option<char> {
    (pos > 0).then(|| text.char(text.byte_to_char(pos) - 1))
}

/// Skip forward over characters while `keep` holds.
fn skip_forward(text: &Rope, mut pos: usize, keep: impl Fn(char) -> bool) -> usize {
    for c in text.chars_at(text.byte_to_char(pos)) {
        if !keep(c) {
            break;
        }
        pos += c.len_utf8();
    }
    pos
}

fn skip_backward(text: &Rope, mut pos: usize, keep: impl Fn(char) -> bool) -> usize {
    let mut chars = text.chars_at(text.byte_to_char(pos));
    while let Some(c) = chars.prev() {
        if !keep(c) {
            break;
        }
        pos -= c.len_utf8();
    }
    pos
}

/// The end of the next word: skip separators, then the word (Emacs `M-f`).
pub fn word_end(text: &Rope, pos: usize, words: Words) -> usize {
    let start = skip_forward(text, pos, |c| words.class(c).is_none());
    match char_at(text, start).and_then(|c| words.class(c)) {
        Some(class) => skip_forward(text, start, |c| words.class(c) == Some(class)),
        None => start,
    }
}

/// The start of the next word: skip the rest of this word, then separators
/// (Vim `w`).
pub fn next_word_start(text: &Rope, pos: usize, words: Words) -> usize {
    let after = match char_at(text, pos).and_then(|c| words.class(c)) {
        Some(class) => skip_forward(text, pos, |c| words.class(c) == Some(class)),
        None => pos,
    };
    skip_forward(text, after, |c| words.class(c).is_none())
}

/// The start of this or the previous word (Emacs `M-b`, Vim `b`).
pub fn word_start(text: &Rope, pos: usize, words: Words) -> usize {
    let end = skip_backward(text, pos, |c| words.class(c).is_none());
    match char_before(text, end).and_then(|c| words.class(c)) {
        Some(class) => skip_backward(text, end, |c| words.class(c) == Some(class)),
        None => end,
    }
}

/// The word (or run of separators) at `pos`; with `around`, also the
/// separators after it, or before it when there are none after (Vim `iw`,
/// `aw`).
pub fn word_object(text: &Rope, pos: usize, words: Words, around: bool) -> Range<usize> {
    let class = char_at(text, pos).map(|c| words.class(c));
    let same = |c: char| Some(words.class(c)) == class;
    let (from, to) = (skip_backward(text, pos, same), skip_forward(text, pos, same));
    if !around || class == Some(None) {
        return from..to;
    }
    let sep = |c: char| words.class(c).is_none() && c != '\n';
    match skip_forward(text, to, sep) {
        end if end > to => from..end,
        _ => skip_backward(text, from, sep)..to,
    }
}

pub fn line_start(text: &Rope, pos: usize) -> usize {
    text.line_to_byte(text.byte_to_line(pos))
}

/// The end of the line's text, before its line break.
pub fn line_end(text: &Rope, pos: usize) -> usize {
    let line = text.byte_to_line(pos);
    let end = text.line_to_byte(line + 1);
    let slice = text.byte_slice(text.line_to_byte(line)..end);
    let n = slice.len_bytes();
    let br = if slice.bytes_at(n).prev() == Some(b'\n') { if n >= 2 && slice.byte(n - 2) == b'\r' { 2 } else { 1 } } else { 0 };
    end - br
}

/// The line that `pos` is in, from its start up to and including its line
/// break (what a linewise operator acts on).
pub fn line_span(text: &Rope, pos: usize) -> Range<usize> {
    let line = text.byte_to_line(pos);
    text.line_to_byte(line)..text.line_to_byte(line + 1)
}

/// Graphemes from the start of the line to `pos`.
pub fn column(text: &Rope, pos: usize) -> usize {
    let mut p = line_start(text, pos);
    let mut n = 0;
    while p < pos {
        p = next_grapheme(text, p);
        n += 1;
    }
    n
}

/// `count` logical lines down (negative: up), at grapheme column `goal` or
/// the end of a shorter line.
pub fn line_down(text: &Rope, pos: usize, count: isize, goal: usize) -> usize {
    let last = text.len_lines() - 1;
    let line = (text.byte_to_line(pos) as isize + count).clamp(0, last as isize) as usize;
    let (mut p, end) = (text.line_to_byte(line), line_end(text, text.line_to_byte(line)));
    for _ in 0..goal {
        if p >= end {
            break;
        }
        p = next_grapheme(text, p);
    }
    p
}

/// The first occurrence of `needle` starting at or after `from` (or, going
/// backward, ending at or before it).
/// The first match of `needle` after `from` (`forward`), or the last
/// before it. With `fold`, letters match whatever their case.
pub fn search(text: &Rope, from: usize, needle: &str, forward: bool, fold: bool) -> Option<Range<usize>> {
    if needle.is_empty() {
        return None;
    }
    // Plain text for now; regular expressions over ropes are language step 12.
    if forward {
        let hay = text.byte_slice(from..).to_string();
        matches(&hay, needle, fold).next().map(|r| from + r.start..from + r.end)
    } else {
        let hay = text.byte_slice(..from).to_string();
        matches(&hay, needle, fold).last()
    }
}

/// The matches of `needle` that start between `from` and `to`, in order,
/// not overlapping.
pub fn search_all(text: &Rope, from: usize, to: usize, needle: &str, fold: bool) -> Vec<Range<usize>> {
    if needle.is_empty() || from >= to {
        return Vec::new();
    }
    // A match starting before `to` may end past it.
    let end = text.len_bytes().min(to.saturating_add(needle.len() * 4));
    let end = text.char_to_byte(text.byte_to_char(end));
    let hay = text.byte_slice(from..end).to_string();
    matches(&hay, needle, fold).map(|r| from + r.start..from + r.end).take_while(|r| r.start < to).collect()
}

/// Where `needle` occurs in `hay`, not overlapping.
fn matches<'a>(hay: &'a str, needle: &'a str, fold: bool) -> Box<dyn Iterator<Item = Range<usize>> + 'a> {
    if !fold {
        return Box::new(hay.match_indices(needle).map(|(i, m)| i..i + m.len()));
    }
    let same = |a: char, b: char| a == b || a.to_lowercase().eq(b.to_lowercase());
    let n = needle.chars().count();
    let mut next = 0;
    Box::new(hay.char_indices().filter_map(move |(i, _)| {
        if i < next {
            return None;
        }
        let mut chars = hay[i..].char_indices();
        let matched = needle.chars().all(|c| chars.next().is_some_and(|(_, h)| same(c, h)));
        matched.then(|| {
            let end = i + hay[i..].char_indices().nth(n).map_or(hay.len() - i, |(j, _)| j);
            next = end;
            i..end
        })
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> Rope {
        Rope::from_str(s)
    }

    #[test]
    fn searching_with_and_without_case() {
        let t = r("Foo foo FOO fÖo");
        assert_eq!(search(&t, 0, "foo", true, false), Some(4..7));
        assert_eq!(search(&t, 1, "foo", true, true), Some(4..7));
        assert_eq!(search(&t, 15, "foo", false, true), Some(8..11));
        assert_eq!(search_all(&t, 0, t.len_bytes(), "foo", true), [0..3, 4..7, 8..11]);
        assert_eq!(search_all(&t, 0, t.len_bytes(), "föo", true), vec![12..16], "not only ASCII");
        assert_eq!(search_all(&t, 1, 9, "foo", true), [4..7, 8..11], "starting before the end");
    }

    #[test]
    fn graphemes_cross_combining_marks_and_chunks() {
        let t = r("ae\u{301}x");
        assert_eq!(next_grapheme(&t, 1), 4);
        assert_eq!(prev_grapheme(&t, 4), 1);
        let long = "e\u{301}".repeat(5000);
        let t = r(&long);
        let mut p = 0;
        let mut n = 0;
        while p < t.len_bytes() {
            p = next_grapheme(&t, p);
            n += 1;
        }
        assert_eq!(n, 5000);
        assert_eq!(prev_grapheme(&t, t.len_bytes()), t.len_bytes() - 3);
    }

    #[test]
    fn emacs_and_vim_words() {
        let t = r("foo.bar  baz");
        assert_eq!(word_end(&t, 0, Words::Emacs), 3);
        assert_eq!(word_end(&t, 3, Words::Emacs), 7);
        assert_eq!(next_word_start(&t, 0, Words::Vim), 3);
        assert_eq!(next_word_start(&t, 3, Words::Vim), 4);
        assert_eq!(next_word_start(&t, 4, Words::Vim), 9);
        assert_eq!(word_start(&t, 12, Words::Emacs), 9);
        assert_eq!(word_start(&t, 9, Words::Emacs), 4);
        assert_eq!(word_start(&t, 4, Words::Vim), 3);
    }

    #[test]
    fn word_objects() {
        let t = r("one two  three");
        assert_eq!(word_object(&t, 5, Words::Vim, false), 4..7);
        assert_eq!(word_object(&t, 5, Words::Vim, true), 4..9);
        assert_eq!(word_object(&t, 11, Words::Vim, true), 7..14);
        assert_eq!(word_object(&t, 7, Words::Vim, false), 7..9);
    }

    #[test]
    fn lines_keep_their_breaks() {
        let t = r("ab\r\ncdé\nlast");
        assert_eq!(line_end(&t, 0), 2);
        assert_eq!(line_span(&t, 1), 0..4);
        assert_eq!(line_start(&t, 6), 4);
        assert_eq!(line_end(&t, 4), 8);
        assert_eq!(line_end(&t, 9), 13);
        assert_eq!(column(&t, 8), 3);
        assert_eq!(line_down(&t, 2, 1, 2), 6);
        assert_eq!(line_down(&t, 8, 1, 3), 12);
        assert_eq!(line_down(&t, 9, -5, 9), 2);
    }

    #[test]
    fn plain_search_both_ways() {
        let t = r("xabyab");
        assert_eq!(search(&t, 0, "ab", true, false), Some(1..3));
        assert_eq!(search(&t, 2, "ab", true, false), Some(4..6));
        assert_eq!(search(&t, 5, "ab", false, false), Some(1..3));
        assert_eq!(search(&t, 6, "zz", true, false), None);
    }
}
