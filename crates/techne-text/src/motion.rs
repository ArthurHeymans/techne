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

/// Whether the `n` bytes of text from `from` are ASCII, where each byte is
/// a grapheme: counting them one by one took a second on a 1 MB line.
/// `from + n` need not be a character's boundary.
fn ascii(text: &Rope, from: usize, n: usize) -> bool {
    let mut left = n;
    for chunk in text.byte_slice(from..).chunks() {
        let part = &chunk.as_bytes()[..left.min(chunk.len())];
        if !part.is_ascii() {
            return false;
        }
        left -= part.len();
        if left == 0 {
            break;
        }
    }
    true
}

/// Graphemes from the start of the line to `pos`.
pub fn column(text: &Rope, pos: usize) -> usize {
    let start = line_start(text, pos);
    if ascii(text, start, pos - start) {
        return pos - start;
    }
    let (mut p, mut n) = (start, 0);
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
    let at = p.saturating_add(goal).min(end);
    // What follows must be ASCII too, or a mark may combine with the last.
    if ascii(text, p, at - p) && (at == end || text.byte(at).is_ascii()) {
        return at;
    }
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

/// The starts of the lines containing `needle`, in order, at most `limit`.
pub fn lines_containing(text: &Rope, needle: &str, fold: bool, limit: usize) -> Vec<usize> {
    if needle.is_empty() {
        return Vec::new();
    }
    let hay = text.to_string();
    let mut starts: Vec<usize> = Vec::new();
    for r in matches(&hay, needle, fold) {
        let start = text.line_to_byte(text.byte_to_line(r.start));
        if starts.last() != Some(&start) {
            if starts.len() == limit {
                break;
            }
            starts.push(start);
        }
    }
    starts
}

/// Where `needle` occurs in `hay`, not overlapping.
fn matches<'a>(hay: &'a str, needle: &'a str, fold: bool) -> Box<dyn Iterator<Item = Range<usize>> + 'a> {
    if !fold {
        return Box::new(hay.match_indices(needle).map(|(i, m)| i..i + m.len()));
    }
    // Both lowered, then searched with memchr: comparing characters folded
    // one by one took some 150 instructions a character, half a second for
    // a search failing in a 100k-line file.
    let (low, shifts) = lowered(hay);
    let finder = memchr::memmem::Finder::new(needle.to_lowercase().as_bytes()).into_owned();
    // A position in `low` where it is in `hay`: lowering a character may
    // change its length, rarely (shifts are where it did).
    let back = move |p: usize| {
        let (at, from) = shifts[shifts.partition_point(|&(at, _)| at <= p) - 1];
        hay.floor_char_boundary(from + (p - at))
    };
    let mut at = 0;
    Box::new(std::iter::from_fn(move || {
        if finder.needle().is_empty() {
            return None;
        }
        let i = at + finder.find(&low.as_bytes()[at.min(low.len())..])?;
        at = i + finder.needle().len();
        // To the end of the character the match's last byte lowers from.
        let last = back(at - 1);
        Some(back(i)..last + hay[last..].chars().next().map_or(0, char::len_utf8))
    }))
}

/// `text` in lower case, as `str::to_lowercase` lowers it, and where
/// lowering shifted what follows: from each position of the lowered text,
/// the position in `text`. ASCII, most text, is lowered at once.
pub fn lowered(text: &str) -> (String, Vec<(usize, usize)>) {
    // Mostly, characters lower to one of the same length: in place.
    let mut low = text.to_ascii_lowercase();
    let mut i = 0;
    while let Some(n) = first_non_ascii(&low.as_bytes()[i..]) {
        let c = low[i + n..].chars().next().expect("a character");
        let mut lower = c.to_lowercase();
        match (lower.next(), lower.next()) {
            // Final or not by its context, which `str::to_lowercase` reads.
            _ if c == 'Σ' => return lowered_shifting(text),
            (Some(l), None) if l.len_utf8() == c.len_utf8() => {
                let mut buf = [0; 4];
                low.replace_range(i + n..i + n + c.len_utf8(), l.encode_utf8(&mut buf));
            }
            _ => return lowered_shifting(text),
        }
        i += n + c.len_utf8();
    }
    (low, vec![(0, 0)])
}

/// Where the first byte of `bytes` that is not ASCII is, found a block at
/// a time.
fn first_non_ascii(bytes: &[u8]) -> Option<usize> {
    let blocks = bytes.chunks(64);
    let mut at = 0;
    for block in blocks {
        if !block.is_ascii() {
            return block.iter().position(|b| !b.is_ascii()).map(|n| at + n);
        }
        at += block.len();
    }
    None
}

/// `lowered`, where lowering changes the length of some character, or
/// depends on what is around it: shifts where a character lowers to one
/// of another length (both sigmas are as long as the capital).
fn lowered_shifting(text: &str) -> (String, Vec<(usize, usize)>) {
    let low = text.to_lowercase();
    let mut shifts = vec![(0, 0)];
    let (mut at, mut shift) = (0, 0isize);
    for (i, c) in text.char_indices() {
        let n = if c == 'Σ' { c.len_utf8() } else { c.to_lowercase().map(char::len_utf8).sum() };
        at += n;
        let now = at as isize - (i + c.len_utf8()) as isize;
        if now != shift {
            shifts.push((at, i + c.len_utf8()));
            shift = now;
        }
    }
    (low, shifts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> Rope {
        Rope::from_str(s)
    }

    #[test]
    fn columns_and_lines_beyond_ascii() {
        let t = r("ab\né日x\nabcdef");
        assert_eq!((column(&t, 3 + 5), line_down(&t, 1, 1, 2)), (2, 3 + 5));
        assert_eq!(line_down(&t, 3 + 5, 1, 2), 3 + 7 + 2);
        // Not between a letter and the mark combining with it.
        let t = r("x\na\u{301}b");
        assert_eq!(line_down(&t, 0, 1, 1), 5);
    }

    #[test]
    fn searching_with_and_without_case() {
        let t = r("Foo foo FOO fÖo");
        assert_eq!(search(&t, 0, "foo", true, false), Some(4..7));
        assert_eq!(search(&t, 1, "foo", true, true), Some(4..7));
        assert_eq!(search(&t, 15, "foo", false, true), Some(8..11));
        assert_eq!(search_all(&t, 0, t.len_bytes(), "foo", true), [0..3, 4..7, 8..11]);
        assert_eq!(search_all(&t, 0, t.len_bytes(), "föo", true), vec![12..16], "not only ASCII");
        // Where lowering changes a character's length (the Kelvin sign
        // lowers to k), matches are where they are in the text.
        let k = r("\u{212a}åb Kåb kÅB");
        assert_eq!(search_all(&k, 0, k.len_bytes(), "kåb", true), [0..6, 7..11, 12..16]);
        let k = r("\u{212a}");
        assert_eq!(search_all(&k, 0, k.len_bytes(), "k", true), vec![0..3], "an ASCII needle too");
        // A match takes all of the character its last byte lowers from (a
        // dotted capital I lowers to two), and final sigma is as words fold.
        let i = r("åİx");
        assert_eq!(search_all(&i, 0, i.len_bytes(), "åi", true), vec![0..4]);
        assert_eq!(lowered("\u{212a}ΟΣ").0, "kος");
        assert_eq!(search_all(&t, 1, 9, "foo", true), [4..7, 8..11], "starting before the end");
        let t = r("a a\nb\na\na");
        assert_eq!((lines_containing(&t, "a", false, 9), lines_containing(&t, "a", false, 2)), (vec![0, 6, 8], vec![0, 6]));
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
