//! Display segments and scrolling by anchor, for any frontend (EDITOR.md,
//! section 6). How text breaks into visual lines is the frontend's (shaped
//! glyphs in a window, cells in a terminal); given where a segment's visual
//! lines start, moving the scroll anchor by visual lines and keeping the
//! caret visible are the same everywhere.
//!
//! Text is laid out in display segments: a logical line, or for a line
//! longer than `LONG_LINE` bytes, consecutive pieces of it. That is the
//! policy for pathological lines: a megabyte line costs only the pieces on
//! screen, at the price of a visual break every `LONG_LINE` bytes.

use techne_text::{motion, ropey::Rope};

use crate::present::Fold;

pub const LONG_LINE: usize = 4096;

/// A byte range of one line, without its line break.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub start: usize,
    pub end: usize,
}

/// `p` moved back to a character boundary.
pub fn snap(text: &Rope, p: usize) -> usize {
    text.char_to_byte(text.byte_to_char(p))
}

/// The segment that `pos` is in. A position on a line break belongs to the
/// end of its line.
pub fn segment_at(text: &Rope, pos: usize) -> Segment {
    let (ls, le) = (motion::line_start(text, pos), motion::line_end(text, pos));
    if le - ls <= LONG_LINE {
        return Segment { start: ls, end: le };
    }
    let pos = pos.min(le);
    let piece = |k: usize| Segment {
        start: snap(text, ls + k * LONG_LINE),
        end: if ls + (k + 1) * LONG_LINE >= le { le } else { snap(text, ls + (k + 1) * LONG_LINE) },
    };
    let mut k = (pos - ls) / LONG_LINE;
    if k > 0 && piece(k).start == le {
        k -= 1;
    }
    let s = piece(k);
    if pos >= s.end && s.end < le { piece(k + 1) } else { s }
}

pub fn next_segment(text: &Rope, seg: Segment) -> Option<Segment> {
    let le = motion::line_end(text, seg.start);
    if seg.end < le {
        return Some(segment_at(text, seg.end));
    }
    let line = text.byte_to_line(seg.start);
    (line + 1 < text.len_lines()).then(|| segment_at(text, text.line_to_byte(line + 1)))
}

pub fn prev_segment(text: &Rope, seg: Segment) -> Option<Segment> {
    let ls = motion::line_start(text, seg.start);
    if seg.start > ls {
        return Some(segment_at(text, seg.start - 1));
    }
    let line = text.byte_to_line(seg.start);
    (line > 0).then(|| segment_at(text, motion::line_end(text, text.line_to_byte(line - 1))))
}

/// Return the fold hiding `pos`, if any (`folds` sorted and non-overlapping).
pub fn hidden_fold(folds: &[Fold], pos: usize) -> Option<Fold> {
    let i = folds.partition_point(|f| f.to <= pos);
    folds.get(i).copied().filter(|f| f.from <= pos && pos < f.to)
}

/// Normalize `pos` out of any fold by snapping to the start of the line
/// immediately preceding the fold (its header line), or `0`.
pub fn visible_pos(text: &Rope, folds: &[Fold], pos: usize) -> usize {
    let mut p = snap(text, pos.min(text.len_bytes()));
    while let Some(f) = hidden_fold(folds, p) {
        if f.from == 0 {
            return f.to.min(text.len_bytes());
        }
        p = motion::line_start(text, f.from - 1);
    }
    p
}

/// Next display segment after `seg`, skipping any folded ranges.
pub fn next_visible_segment(text: &Rope, seg: Segment, folds: &[Fold]) -> Option<Segment> {
    let mut next = next_segment(text, seg)?;
    while let Some(f) = hidden_fold(folds, next.start) {
        let len = text.len_bytes();
        if f.to >= len {
            return None;
        }
        next = segment_at(text, f.to);
    }
    Some(next)
}

/// Previous display segment before `seg`, skipping any folded ranges.
pub fn prev_visible_segment(text: &Rope, seg: Segment, folds: &[Fold]) -> Option<Segment> {
    let mut prev = prev_segment(text, seg)?;
    while let Some(f) = hidden_fold(folds, prev.start) {
        if f.from == 0 {
            return None;
        }
        prev = segment_at(text, motion::line_end(text, f.from - 1));
    }
    Some(prev)
}

/// Fold-aware counterpart of `scroll_lines`.
pub fn scroll_lines_folded(text: &Rope, anchor: usize, n: i64, folds: &[Fold], starts: &mut impl FnMut(Segment) -> Vec<usize>) -> usize {
    let anchor = visible_pos(text, folds, anchor);
    let mut seg = segment_at(text, anchor);
    let mut lines = starts(seg);
    let mut i = line_index(&lines, anchor) as i64 + n;
    if n > 0 {
        while (i as usize) < lines.len() && i > 0 && lines[i as usize] == lines[i as usize - 1] {
            i += 1;
        }
    }
    loop {
        if i < 0 {
            match prev_visible_segment(text, seg, folds) {
                Some(p) => {
                    seg = p;
                    lines = starts(seg);
                    i += lines.len() as i64;
                }
                None => return visible_pos(text, folds, 0),
            }
        } else if i >= lines.len() as i64 {
            match next_visible_segment(text, seg, folds) {
                Some(nx) => {
                    i -= lines.len() as i64;
                    seg = nx;
                    lines = starts(seg);
                }
                None => return *lines.last().expect("a segment has a visual line"),
            }
        } else {
            return lines[i as usize];
        }
    }
}

/// Fold-aware counterpart of `keep_visible`.
pub fn keep_visible_folded(
    text: &Rope,
    anchor: usize,
    head: usize,
    rows: usize,
    folds: &[Fold],
    starts: &mut impl FnMut(Segment) -> Vec<usize>,
) -> Option<usize> {
    let anchor = visible_pos(text, folds, anchor);
    let head = visible_pos(text, folds, head);
    let fit = rows.max(1) as i64;
    let lines = starts(segment_at(text, head));
    let line_start = lines[line_index(&lines, head)];
    let top = scroll_lines_folded(text, anchor, 0, folds, starts);
    let centred = |starts: &mut _| scroll_lines_folded(text, line_start, -(fit / 2), folds, starts);
    if line_start < top {
        let near = scroll_lines_folded(text, line_start, SCROLL_CONSERVATIVELY, folds, starts) >= top;
        return Some(if near { line_start } else { centred(starts) });
    }
    let below = rows_down_folded(text, top, head, fit + SCROLL_CONSERVATIVELY, folds, starts);
    if below < fit {
        return None;
    }
    let near = below < fit + SCROLL_CONSERVATIVELY;
    Some(if near { scroll_lines_folded(text, line_start, -(fit - 1), folds, starts) } else { centred(starts) })
}

fn rows_down_folded(
    text: &Rope,
    top: usize,
    head: usize,
    limit: i64,
    folds: &[Fold],
    starts: &mut impl FnMut(Segment) -> Vec<usize>,
) -> i64 {
    let target = segment_at(text, head);
    let target_index = line_index(&starts(target), head);
    let mut seg = segment_at(text, top);
    let mut i = line_index(&starts(seg), top);
    let mut n = 0;
    loop {
        if seg == target {
            return n + target_index as i64 - i as i64;
        }
        n += starts(seg).len() as i64 - i as i64;
        if n > limit {
            return n;
        }
        match next_visible_segment(text, seg, folds) {
            Some(next) => (seg, i) = (next, 0),
            None => return n,
        }
    }
}

/// The scroll anchor `n` visual lines further down (up when negative).
/// `starts` gives where a segment's visual lines start, its first at the
/// segment's start.
///
/// Visual lines starting inside an inlay (text shown in the line that is
/// not the text's) start where it is, as the line it began on may: a
/// position stands for the first of the lines starting there, and
/// scrolling down past it goes past them all.
pub fn scroll_lines(text: &Rope, anchor: usize, n: i64, starts: &mut impl FnMut(Segment) -> Vec<usize>) -> usize {
    let mut seg = segment_at(text, anchor);
    let mut lines = starts(seg);
    let mut i = line_index(&lines, anchor) as i64 + n;
    if n > 0 {
        // Lines that cannot be the top (they start where the one before
        // does) are gone past.
        while (i as usize) < lines.len() && i > 0 && lines[i as usize] == lines[i as usize - 1] {
            i += 1;
        }
    }
    loop {
        if i < 0 {
            match prev_segment(text, seg) {
                Some(p) => {
                    seg = p;
                    lines = starts(seg);
                    i += lines.len() as i64;
                }
                None => return 0,
            }
        } else if i >= lines.len() as i64 {
            match next_segment(text, seg) {
                Some(nx) => {
                    i -= lines.len() as i64;
                    seg = nx;
                    lines = starts(seg);
                }
                None => return *lines.last().expect("a segment has a visual line"),
            }
        } else {
            return lines[i as usize];
        }
    }
}

/// Which of a segment's visual lines, starting at `lines`, `pos` is on:
/// the first starting there, else the last starting before it.
pub fn line_index(lines: &[usize], pos: usize) -> usize {
    lines.iter().position(|&s| s == pos).or_else(|| lines.iter().rposition(|&s| s <= pos)).unwrap_or(0)
}

/// How far off screen the caret may go and the view still scroll just
/// enough to show it; further, its line is centred. Emacs's
/// scroll-conservatively, as Arthur has it.
pub const SCROLL_CONSERVATIVELY: i64 = 10;

/// A new scroll anchor that shows `head` in a viewport `fit` visual lines
/// high, if it is off screen: as Emacs redisplays, at the top or bottom
/// when it is at most `SCROLL_CONSERVATIVELY` lines away, else centred.
pub fn keep_visible(text: &Rope, anchor: usize, head: usize, fit: usize, starts: &mut impl FnMut(Segment) -> Vec<usize>) -> Option<usize> {
    let fit = fit.max(1) as i64;
    let lines = starts(segment_at(text, head));
    let line_start = lines[line_index(&lines, head)];
    let top = scroll_lines(text, anchor, 0, starts);
    let centred = |starts: &mut _| scroll_lines(text, line_start, -(fit / 2), starts);
    if line_start < top {
        let near = scroll_lines(text, line_start, SCROLL_CONSERVATIVELY, starts) >= top;
        return Some(if near { line_start } else { centred(starts) });
    }
    // Visual lines from the top's to the caret's, counted one by one: an
    // anchor cannot name a line starting inside an inlay, a count can.
    let below = rows_down(text, top, head, fit + SCROLL_CONSERVATIVELY, starts);
    if below < fit {
        return None;
    }
    let near = below < fit + SCROLL_CONSERVATIVELY;
    Some(if near { scroll_lines(text, line_start, -(fit - 1), starts) } else { centred(starts) })
}

/// How many visual lines `head`'s is below `top`'s, counting no further
/// than `limit`.
fn rows_down(text: &Rope, top: usize, head: usize, limit: i64, starts: &mut impl FnMut(Segment) -> Vec<usize>) -> i64 {
    let target = segment_at(text, head);
    let target_index = line_index(&starts(target), head);
    let mut seg = segment_at(text, top);
    let mut i = line_index(&starts(seg), top);
    let mut n = 0;
    loop {
        if seg == target {
            return n + target_index as i64 - i as i64;
        }
        n += starts(seg).len() as i64 - i as i64;
        if n > limit {
            return n;
        }
        match next_segment(text, seg) {
            Some(next) => (seg, i) = (next, 0),
            None => return n,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_of_short_and_long_lines() {
        let t = Rope::from_str("ab\ncd");
        assert_eq!(segment_at(&t, 1), Segment { start: 0, end: 2 });
        assert_eq!(segment_at(&t, 2), Segment { start: 0, end: 2 });
        assert_eq!(next_segment(&t, segment_at(&t, 0)), Some(Segment { start: 3, end: 5 }));
        assert_eq!(prev_segment(&t, Segment { start: 3, end: 5 }), Some(Segment { start: 0, end: 2 }));

        let long = format!("x{}\nend", "é".repeat(LONG_LINE));
        let t = Rope::from_str(&long);
        let le = 1 + 2 * LONG_LINE;
        let mut seg = segment_at(&t, 0);
        let mut pieces = vec![seg];
        while let Some(n) = next_segment(&t, seg).filter(|s| s.start < le) {
            assert_eq!(n.start, seg.end, "pieces are contiguous");
            seg = n;
            pieces.push(n);
        }
        assert_eq!(pieces.last().unwrap().end, le);
        assert!(pieces.iter().all(|s| s.end - s.start <= LONG_LINE && t.char_to_byte(t.byte_to_char(s.start)) == s.start));
        for s in &pieces {
            assert_eq!(segment_at(&t, s.start), *s);
            assert_eq!(segment_at(&t, s.end - 1), *s);
        }
        assert_eq!(prev_segment(&t, segment_at(&t, le + 1)), pieces.last().copied());
    }

    #[test]
    fn folds_skip_hidden_segments_and_normalize_positions() {
        let t = Rope::from_str("* One\nbody 1\nbody 2\n* Two\nbody 3\n");
        let folds = [Fold { from: t.line_to_byte(1), to: t.line_to_byte(3) }];
        let s0 = segment_at(&t, 0);
        assert_eq!(next_visible_segment(&t, s0, &folds), Some(segment_at(&t, t.line_to_byte(3))));
        assert_eq!(prev_visible_segment(&t, segment_at(&t, t.line_to_byte(3)), &folds), Some(s0));
        assert_eq!(visible_pos(&t, &folds, t.line_to_byte(2)), 0);
        let mut starts = |s: Segment| vec![s.start];
        assert_eq!(scroll_lines_folded(&t, 0, 1, &folds, &mut starts), t.line_to_byte(3));
        assert_eq!(scroll_lines_folded(&t, t.line_to_byte(3), -1, &folds, &mut starts), 0);
    }

    #[test]
    fn scrolling_over_wrapped_lines() {
        // Each line wraps every 4 bytes.
        let t = Rope::from_str("abcdefgh\nij\nklmnop\n");
        let mut starts = |s: Segment| (s.start..s.end.max(s.start + 1)).step_by(4).collect::<Vec<_>>();
        assert_eq!(scroll_lines(&t, 0, 1, &mut starts), 4);
        assert_eq!(scroll_lines(&t, 5, 2, &mut starts), 12);
        assert_eq!(scroll_lines(&t, 12, -3, &mut starts), 0);
        assert_eq!(scroll_lines(&t, 0, 99, &mut starts), 19, "the last visual line");
        // The caret on "op", two lines high: scrolled so it is the last one.
        assert_eq!(keep_visible(&t, 0, 17, 2, &mut starts), Some(12));
        assert_eq!(keep_visible(&t, 12, 17, 2, &mut starts), None);
        assert_eq!(keep_visible(&t, 16, 1, 2, &mut starts), Some(0));
    }

    #[test]
    fn lines_starting_inside_an_inlay_count_but_are_no_anchor() {
        // "ab": a hint after "a" wraps over three more lines, all
        // starting at 1, then "b".
        let t = Rope::from_str("ab\ncd\n");
        let mut starts = |s: Segment| if s.start == 0 { vec![0, 1, 1, 1] } else { vec![s.start] };
        assert_eq!(scroll_lines(&t, 0, 1, &mut starts), 1);
        assert_eq!(scroll_lines(&t, 1, 1, &mut starts), 3, "past the lines starting at 1");
        assert_eq!(scroll_lines(&t, 3, -1, &mut starts), 1);
        // Four lines from 0: line 3 is the fourth, not the second.
        assert_eq!(keep_visible(&t, 0, 3, 3, &mut starts), Some(1));
    }
}
