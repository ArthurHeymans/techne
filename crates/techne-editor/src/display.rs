//! What frontends share about laying out a text snapshot: display segments
//! and scrolling by visual lines. Each frontend wraps segments its own way
//! (shaped glyphs, terminal cells) and reports where their visual lines
//! start; the rest is the same for all of them.
//!
//! A segment is a logical line, or for a line longer than `LONG_LINE` bytes,
//! consecutive pieces of it. That is the policy for pathological lines: a
//! megabyte line costs only the pieces on screen, at the price of a visual
//! break every `LONG_LINE` bytes.

use techne_text::{motion, ropey::Rope};

pub const LONG_LINE: usize = 4096;

/// A byte range of one line, without its line break.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Segment {
    pub start: usize,
    pub end: usize,
}

fn snap(text: &Rope, p: usize) -> usize {
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

/// How a frontend wraps segments into visual lines.
pub trait Wrapping {
    /// Where the segment's visual lines start, as positions in the text;
    /// the first is the segment's start.
    fn visual_starts(&mut self, text: &Rope, seg: Segment) -> Vec<usize>;
}

/// A segment in a frame: its first visual line is `row` lines from the top
/// (negative when it begins above), and it has `rows` of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placed {
    pub seg: Segment,
    pub row: i64,
    pub rows: usize,
}

fn line_index(starts: &[usize], pos: usize) -> usize {
    starts.iter().rposition(|&s| s <= pos).unwrap_or(0)
}

/// The segments that fill `fit` visual lines from the one the scroll anchor
/// is on.
pub fn frame(w: &mut impl Wrapping, text: &Rope, anchor: usize, fit: usize) -> Vec<Placed> {
    let anchor = snap(text, anchor.min(text.len_bytes()));
    let first = segment_at(text, anchor);
    let mut row = -(line_index(&w.visual_starts(text, first), anchor) as i64);
    let mut placed = Vec::new();
    let mut seg = Some(first);
    while let Some(s) = seg
        && row < fit as i64
    {
        let rows = w.visual_starts(text, s).len().max(1);
        placed.push(Placed { seg: s, row, rows });
        row += rows as i64;
        seg = next_segment(text, s);
    }
    placed
}

/// The scroll anchor `n` visual lines further down (up when negative), at
/// the start of that line.
pub fn scroll_lines(w: &mut impl Wrapping, text: &Rope, anchor: usize, n: i64) -> usize {
    let mut seg = segment_at(text, anchor);
    let mut starts = w.visual_starts(text, seg);
    let mut i = line_index(&starts, anchor) as i64 + n;
    loop {
        if i < 0 {
            let Some(p) = prev_segment(text, seg) else { return 0 };
            seg = p;
            starts = w.visual_starts(text, seg);
            i += starts.len() as i64;
        } else if i >= starts.len() as i64 {
            let Some(nx) = next_segment(text, seg) else { return *starts.last().expect("a visual line") };
            i -= starts.len() as i64;
            seg = nx;
            starts = w.visual_starts(text, seg);
        } else {
            return starts[i as usize];
        }
    }
}

/// A new scroll anchor that shows `head` in a frame of `fit` visual lines,
/// if it is off screen: on the first line when it is above, on the last
/// when it is below.
pub fn keep_visible(w: &mut impl Wrapping, text: &Rope, anchor: usize, head: usize, fit: usize) -> Option<usize> {
    let fit = fit.max(1) as i64;
    let starts = w.visual_starts(text, segment_at(text, head));
    let line_start = starts[line_index(&starts, head)];
    let top = scroll_lines(w, text, anchor, 0);
    if line_start < top {
        return Some(line_start);
    }
    if line_start <= scroll_lines(w, text, top, fit - 1) {
        return None;
    }
    Some(scroll_lines(w, text, line_start, -(fit - 1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wraps every `n` bytes (ASCII test text).
    struct Every(usize);

    impl Wrapping for Every {
        fn visual_starts(&mut self, _: &Rope, seg: Segment) -> Vec<usize> {
            let mut v: Vec<usize> = (seg.start..seg.end).step_by(self.0).collect();
            if v.is_empty() {
                v.push(seg.start);
            }
            v
        }
    }

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
    fn frames_scroll_and_keep_the_caret_visible() {
        let text: String = (0..100).map(|i| format!("line {i:03}\n")).collect();
        let t = Rope::from_str(&text);
        let mut w = Every(4);
        // Each line wraps into two visual lines ("line", " 042").
        let placed = frame(&mut w, &t, 0, 10);
        assert_eq!(placed.len(), 5);
        assert_eq!(placed[1], Placed { seg: Segment { start: 9, end: 17 }, row: 2, rows: 2 });
        assert_eq!(scroll_lines(&mut w, &t, 0, 3), 13);
        assert_eq!(scroll_lines(&mut w, &t, 13, -5), 0);
        let starting_mid_line = frame(&mut w, &t, 13, 10);
        assert_eq!(starting_mid_line[0].row, -1);
        // The caret on line 50: scrolled so that its visual line is the last.
        let head = t.line_to_byte(50) + 6;
        assert_eq!(keep_visible(&mut w, &t, 0, head, 10), Some(t.line_to_byte(46)));
        assert_eq!(keep_visible(&mut w, &t, t.line_to_byte(46), head, 10), None);
        assert_eq!(keep_visible(&mut w, &t, t.line_to_byte(60), head, 10), Some(t.line_to_byte(50) + 4));
    }
}
