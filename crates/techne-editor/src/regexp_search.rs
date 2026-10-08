//! Regular expression search over the text of documents and presentations.
//!
//! The regexp (SRFI 115's, compiled by techne-vm) runs over the rope's
//! chunks as they are, so text is never copied into a string to search
//! it, and an interrupt stops a search at the next chunk.

use std::{ops::Range, sync::atomic::AtomicBool};

use regex_cursor::RopeyCursor;
use techne_text::ropey::Rope;
use techne_vm::regexp::{Regexp, Stop};

/// Why a search gave no answer, for Scheme.
fn stop_message(stop: Stop) -> String {
    match stop {
        Stop::Stopped => techne_vm::vm::INTERRUPTED.to_owned(),
        Stop::Error(e) => e,
    }
}

/// The first match of `re` in `text` starting at or after `from`, and
/// ending by `to`.
fn find(text: &Rope, re: &Regexp, from: usize, to: usize, stop: &AtomicBool) -> Result<Option<Range<usize>>, String> {
    // From the chunk of the character before `from`, for look-behind.
    let at = text.char_to_byte(text.byte_to_char(from.saturating_sub(1)));
    let caps = re.search(RopeyCursor::at(text.slice(..), at), from, to, false, stop).map_err(stop_message)?;
    Ok(caps.and_then(|c| c.get_match()).map(|m| m.range()))
}

/// The position of the character after `at` (or `at` at the end).
fn next_char(text: &Rope, at: usize) -> usize {
    if at >= text.len_bytes() { at } else { text.char_to_byte(text.byte_to_char(at) + 1) }
}

/// The match of `re` nearest `from`: forward, the first starting at or
/// after it; backward, the last of the matches found from the start of the
/// text, not overlapping, that starts before `from` and ends by it, as the
/// literal search finds it. Either takes time linear in the text searched.
pub fn search(text: &Rope, re: &Regexp, from: usize, forward: bool, stop: &AtomicBool) -> Result<Option<Range<usize>>, String> {
    if forward {
        return find(text, re, from, text.len_bytes(), stop);
    }
    let (mut at, mut last) = (0, None);
    while let Some(m) = find(text, re, at, from, stop)? {
        if m.start >= from {
            break;
        }
        let next = if m.is_empty() { next_char(text, m.end) } else { m.end };
        last = Some(m);
        if next <= at {
            break;
        }
        at = next;
    }
    Ok(last)
}

/// The matches of `re` starting from `from` to before `to`, in order, not
/// overlapping; the last may end past `to`.
pub fn search_all(text: &Rope, re: &Regexp, from: usize, to: usize, stop: &AtomicBool) -> Result<Vec<Range<usize>>, String> {
    let mut out = Vec::new();
    let mut at = from;
    while at < to {
        let Some(m) = find(text, re, at, text.len_bytes(), stop)? else { break };
        if m.start >= to {
            break;
        }
        at = if m.is_empty() { next_char(text, m.end) } else { m.end };
        if !m.is_empty() {
            out.push(m);
        }
        if at == text.len_bytes() {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use super::*;

    fn regexp(pattern: &str) -> Regexp {
        Regexp::new(pattern, String::new(), Vec::new()).unwrap()
    }

    #[test]
    fn an_interrupt_stops_a_search_at_the_next_chunk() {
        let text = Rope::from_str(&"a".repeat(1 << 20));
        let (stop, go) = (AtomicBool::new(true), AtomicBool::new(false));
        let re = regexp("b");
        assert_eq!(search(&text, &re, 0, true, &stop), Err(techne_vm::vm::INTERRUPTED.to_owned()));
        assert_eq!(search_all(&text, &re, 0, text.len_bytes(), &stop), Err(techne_vm::vm::INTERRUPTED.to_owned()));
        assert_eq!(search(&text, &re, 0, true, &go), Ok(None));
    }
}
