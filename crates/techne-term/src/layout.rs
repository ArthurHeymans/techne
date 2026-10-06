//! Layout of a text snapshot in terminal cells (EDITOR.md, section 8): the
//! terminal's counterpart of the window's shaped layout. Graphemes take the
//! cells the Unicode tables give them (wide characters two), tab stops are
//! every `TAB_STOP` cells, control characters show as `^X`, and lines wrap
//! at the grapheme that does not fit. As in the window, only display
//! segments around the scroll anchor are laid out, and scrolling by anchor
//! is `techne_editor::segment`'s.

use techne_editor::segment::{self, Segment, next_segment, segment_at, snap};
use techne_text::{motion, ropey::Rope};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub const TAB_STOP: usize = 8;

/// A grapheme placed in a visual line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Glyph {
    pub pos: usize,
    pub len: usize,
    pub col: usize,
    pub width: usize,
    /// What its cells show: the grapheme, spaces for a tab, `^X` for a
    /// control character, `·` for one of no width.
    pub shown: String,
}

/// A visual line: one row of cells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub start: usize,
    pub end: usize,
    /// Cells used.
    pub width: usize,
    /// The next visual line continues this logical line (wrapped, or the
    /// next piece of a long line), so `end` is shown there.
    pub continued: bool,
    pub glyphs: Vec<Glyph>,
}

/// What a grapheme shows at `col` of a line `cols` wide, and its width.
fn cells(g: &str, col: usize, cols: usize) -> (String, usize) {
    match g.chars().next() {
        Some('\t') => {
            let w = (TAB_STOP - col % TAB_STOP).min(cols.saturating_sub(col));
            (" ".repeat(w), w)
        }
        Some(c) if c.is_ascii_control() => (format!("^{}", (c as u8 ^ 0x40) as char), 2),
        Some(c) if c.is_control() => {
            let s = format!("\\{:o}", c as u32);
            let w = s.len();
            (s, w)
        }
        _ => match g.width() {
            0 => ("·".into(), 1),
            w => (g.into(), w.min(2)),
        },
    }
}

/// The visual lines of a segment in `cols` cells.
pub fn wrap(text: &Rope, seg: Segment, cols: usize) -> Vec<Line> {
    let cols = cols.max(1);
    let s = text.byte_slice(seg.start..seg.end).to_string();
    let empty = |start| Line { start, end: start, width: 0, continued: false, glyphs: Vec::new() };
    let mut lines = s.grapheme_indices(true).fold(vec![empty(seg.start)], |mut lines, (i, g)| {
        let pos = seg.start + i;
        let used = lines.last().expect("a line").width;
        let (mut shown, mut width) = cells(g, used, cols);
        if used > 0 && (used >= cols || used + width > cols) {
            lines.last_mut().expect("a line").continued = true;
            lines.push(empty(pos));
            (shown, width) = cells(g, 0, cols);
        }
        let line = lines.last_mut().expect("a line");
        line.glyphs.push(Glyph { pos, len: g.len(), col: line.width, width, shown });
        line.width += width;
        line.end = pos + g.len();
        lines
    });
    lines.last_mut().expect("a line").continued = seg.end < motion::line_end(text, seg.start);
    lines
}

/// Where a segment's visual lines start.
pub fn starts(text: &Rope, seg: Segment, cols: usize) -> Vec<usize> {
    wrap(text, seg, cols).iter().map(|l| l.start).collect()
}

/// The visual lines shown from the scroll anchor, at most `rows`.
pub fn frame(text: &Rope, anchor: usize, rows: usize, cols: usize) -> Vec<Line> {
    let first = segment_at(text, snap(text, anchor.min(text.len_bytes())));
    let lines = wrap(text, first, cols);
    let skip = lines.iter().rposition(|l| l.start <= anchor).unwrap_or(0);
    lines
        .into_iter()
        .skip(skip)
        .chain(std::iter::successors(next_segment(text, first), |&s| next_segment(text, s)).flat_map(|s| wrap(text, s, cols)))
        .take(rows)
        .collect()
}

pub fn scroll_lines(text: &Rope, anchor: usize, n: i64, cols: usize) -> usize {
    segment::scroll_lines(text, anchor, n, &mut |seg| starts(text, seg, cols))
}

pub fn keep_visible(text: &Rope, anchor: usize, head: usize, rows: usize, cols: usize) -> Option<usize> {
    segment::keep_visible(text, anchor, head, rows, &mut |seg| starts(text, seg, cols))
}

/// The text position at a cell: the grapheme on it, or past the end of its
/// line the line's end. Below the text, the last line.
pub fn hit(lines: &[Line], col: usize, row: usize) -> Option<usize> {
    let line = lines.get(row).or(lines.last())?;
    line.glyphs.iter().find(|g| col < g.col + g.width).map(|g| g.pos).or_else(|| match line.glyphs.last() {
        // The end of a wrapped line is shown on the next one.
        Some(g) if line.continued => Some(g.pos),
        _ => Some(line.end),
    })
}

/// The cell (row, column) showing the caret at `pos`, if it is in `lines`.
pub fn caret(lines: &[Line], pos: usize) -> Option<(usize, usize)> {
    lines.iter().enumerate().find_map(|(r, l)| {
        l.glyphs
            .iter()
            .find(|g| g.pos <= pos && pos < g.pos + g.len)
            .map(|g| (r, g.col))
            .or_else(|| (pos == l.end && !l.continued).then_some((r, l.width)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| l.glyphs.iter().map(|g| g.shown.as_str()).collect()).collect()
    }

    #[test]
    fn widths_tabs_and_controls() {
        let t = Rope::from_str("a\tb名\r\u{200b}e\u{301}x\u{1F600}");
        let lines = wrap(&t, segment_at(&t, 0), 80);
        let cols: Vec<(usize, usize)> = lines[0].glyphs.iter().map(|g| (g.col, g.width)).collect();
        assert_eq!(cols, [(0, 1), (1, 7), (8, 1), (9, 2), (11, 2), (13, 1), (14, 1), (15, 1), (16, 2)]);
        assert_eq!(shown(&lines), ["a       b名^M·e\u{301}x😀"]);
    }

    #[test]
    fn wrapping_wide_characters_and_tabs() {
        let t = Rope::from_str("abc名de\tf\nxy");
        let lines = wrap(&t, segment_at(&t, 0), 4);
        // The wide character does not fit after "abc": it starts the next line.
        assert_eq!(shown(&lines), ["abc", "名de", "    ", "f"]);
        assert_eq!(lines.iter().map(|l| l.continued).collect::<Vec<_>>(), [true, true, true, false]);
        assert_eq!(starts(&t, segment_at(&t, 0), 4), [0, 3, 8, 9]);
        // Carets and clicks: on the wide character's second cell, past a
        // line's end, on the line break.
        assert_eq!(caret(&lines, 3), Some((1, 0)));
        assert_eq!(hit(&lines, 1, 1), Some(3));
        assert_eq!(hit(&lines, 3, 0), Some(2), "the end of a wrapped line is not on it");
        assert_eq!(caret(&lines, 10), Some((3, 1)));
        assert_eq!(hit(&lines, 3, 3), Some(10));
    }

    #[test]
    fn frames_scroll_by_anchor() {
        let text: String = (0..50).map(|i| format!("{i} {}\n", "ab".repeat(i % 7))).collect();
        let t = Rope::from_str(&text);
        let cols = 6;
        let anchor = scroll_lines(&t, 0, 30, cols);
        let lines = frame(&t, anchor, 10, cols);
        assert_eq!(lines.len(), 10);
        assert_eq!(lines[0].start, anchor);
        assert_eq!(scroll_lines(&t, anchor, -30, cols), 0);
        // Any position in a visual line shows that line first.
        assert_eq!(frame(&t, lines[1].end - 1, 3, cols)[0], lines[1]);
        // The caret below the frame: scrolled to be on the last row.
        let below = lines[9].end + 1;
        let a = keep_visible(&t, anchor, below, 10, cols).unwrap();
        assert_eq!(caret(&frame(&t, a, 10, cols), below).map(|c| c.0), Some(9));
    }
}
