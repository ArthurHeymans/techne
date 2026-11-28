//! Layout of a text snapshot in terminal cells (EDITOR.md, section 8): the
//! terminal's counterpart of the window's shaped layout. Graphemes take the
//! cells the Unicode tables give them (wide characters two), tab stops are
//! every `TAB_STOP` cells, control characters show as `^X`, and lines wrap
//! at the grapheme that does not fit. As in the window, only display
//! segments around the scroll anchor are laid out, and scrolling by anchor
//! is `techne_editor::segment`'s.

use techne_editor::{
    present::{Conceal, Fold, Hang, Inlay, Pane, StyleId},
    segment::{self, Segment, next_visible_segment, segment_at, visible_pos},
};
use techne_text::{motion, ropey::Rope};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub const TAB_STOP: usize = 8;

/// What a pane shows besides its text, which its lines are laid out with:
/// its inlays, and the lines whose wrapped lines hang under a character.
#[derive(Clone, Copy, Default)]
pub struct Projected<'a> {
    pub inlays: &'a [Inlay],
    pub hangs: &'a [Hang],
    pub folds: &'a [Fold],
    pub conceals: &'a [Conceal],
    pub word_wrap: bool,
    pub truncate_lines: bool,
}

impl<'a> Projected<'a> {
    pub fn of(p: &'a Pane) -> Projected<'a> {
        Projected {
            inlays: &p.inlays,
            hangs: &p.hangs,
            folds: &p.folds,
            conceals: &p.conceals,
            word_wrap: p.display.word_wrap,
            truncate_lines: p.display.truncate_lines,
        }
    }
}

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
    /// An inlay's, not the text's: its style. Its `pos` is where it is
    /// shown before, its `len` 0.
    pub inlay: Option<StyleId>,
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

/// Place a grapheme `g` at `pos` (an inlay's when `inlay`), wrapping
/// before it when it does not fit, the wrapped line starting `indent`
/// cells in.
fn place(
    lines: &mut Vec<Line>,
    g: &str,
    pos: usize,
    len: usize,
    cols: usize,
    inlay: Option<StyleId>,
    (lead, indent, word_wrap): (usize, usize, bool),
) {
    let mut used = lines.last().expect("a line").width;
    let (mut shown, mut width) = cells(g, used, cols);
    let start = if lines.len() == 1 { lead } else { indent };
    if word_wrap && used > start && (used >= cols || used + width > cols) {
        break_word(lines, cols, indent, width.max(1));
        used = lines.last().expect("a line").width;
        (shown, width) = cells(g, used, cols);
    }
    if used > start && (used >= cols || used + width > cols) {
        lines.last_mut().expect("a line").continued = true;
        lines.push(Line { start: pos, end: pos, width: indent, continued: false, glyphs: Vec::new() });
        (shown, width) = cells(g, indent, cols);
    }
    let line = lines.last_mut().expect("a line");
    line.glyphs.push(Glyph { pos, len, col: line.width, width, shown, inlay });
    line.width += width;
    line.end = pos + len;
}

// Cell layout, hit testing and scrolling share these exact glyph positions.
// Scheme chooses word-wrap; realizing it here avoids changing logical rows or
// guessing native Unicode cell widths in the mail/markdown Scheme renderer.
fn break_word(lines: &mut Vec<Line>, cols: usize, indent: usize, next_width: usize) {
    let line = lines.last_mut().expect("a line");
    let space = |g: &Glyph| !g.shown.is_empty() && g.shown.chars().all(|c| c == ' ' || c == '\t');
    let Some(at) = line.glyphs.iter().rposition(space) else { return };
    if at + 1 == line.glyphs.len() || !line.glyphs[..at].iter().any(|g| !space(g)) {
        return;
    }
    let width: usize = line.glyphs[at + 1..].iter().map(|g| g.width).sum();
    if width + next_width > cols.saturating_sub(indent) {
        return; // oversized token: retain grapheme fallback
    }
    let mut tail = line.glyphs.split_off(at + 1);
    let start = tail[0].pos;
    line.end = start;
    line.width = line.glyphs.last().map_or(0, |g| g.col + g.width);
    line.continued = true;
    let mut col = indent;
    for glyph in &mut tail {
        glyph.col = col;
        col += glyph.width;
    }
    let end = tail.iter().map(|g| g.pos + g.len).max().unwrap_or(start);
    lines.push(Line { start, end, width: col, continued: false, glyphs: tail });
}

/// The visual lines of a segment in `cols` cells, with the inlays in it
/// (`inlays` in order of position: those at the segment's end only if it
/// ends its line). A line that hangs (`hangs`) wraps under the column
/// its hang point is at, when that is within the first half.
pub fn wrap(text: &Rope, seg: Segment, cols: usize, shown: Projected<'_>) -> Vec<Line> {
    let (inlays, cols) = (shown.inlays, cols.max(1));
    let wrap_at = if shown.truncate_lines { usize::MAX } else { cols };
    // The hang of the line the segment is of: a later piece of a long
    // line starts under the hang point already.
    let line = text.line_to_byte(text.byte_to_line(seg.start));
    let hang = shown.hangs.binary_search_by_key(&line, |h| h.line).ok().map(|i| shown.hangs[i].at);
    // Where the hang point is drawn on the line's first piece, inlays and
    // all: a later piece's lines start there.
    let lead = match hang {
        Some(at) if line < seg.start && at <= seg.start => {
            let first = wrap(text, segment_at(text, line), cols, Projected { hangs: &[], ..shown });
            first[0].glyphs.iter().find(|g| g.pos == at && g.inlay.is_none()).map(|g| g.col).filter(|&c| c < cols / 2).unwrap_or(0)
        }
        _ => 0,
    };
    let mut indent = lead;
    let s = text.byte_slice(seg.start..seg.end).to_string();
    let ends_line = seg.end == motion::line_end(text, seg.start);
    let first = inlays.partition_point(|i| i.at < seg.start);
    let mut inlays = inlays[first..].iter().take_while(|i| i.at < seg.end || (ends_line && i.at == seg.end)).peekable();
    let mut lines = vec![Line { start: seg.start, end: seg.start, width: lead, continued: false, glyphs: Vec::new() }];
    let mut inlays_before = |lines: &mut Vec<Line>, pos: usize, indent: usize| {
        while let Some(i) = inlays.next_if(|i| i.at <= pos) {
            for g in i.text.graphemes(true) {
                place(lines, g, i.at, 0, wrap_at, Some(i.style), (lead, indent, shown.word_wrap));
            }
        }
    };
    let concealed = |pos: usize| {
        let idx = shown.conceals.partition_point(|c| c.to <= pos);
        shown.conceals.get(idx).is_some_and(|c| c.from <= pos && pos < c.to)
    };
    for (i, g) in s.grapheme_indices(true) {
        let pos = seg.start + i;
        inlays_before(&mut lines, pos, indent);
        if concealed(pos) {
            continue;
        }
        if line == seg.start && hang == Some(pos) && lines.len() == 1 && lines[0].width < cols / 2 {
            indent = lines[0].width;
        }
        place(&mut lines, g, pos, g.len(), wrap_at, None, (lead, indent, shown.word_wrap));
    }
    inlays_before(&mut lines, seg.end, indent);
    let last = lines.last_mut().expect("a line");
    // The line's end is the text's, not past an inlay there.
    last.end = last.end.max(seg.end).min(seg.end);
    last.continued = !ends_line;
    if shown.truncate_lines {
        last.glyphs.retain(|g| g.col + g.width <= cols);
        last.width = last.width.min(cols);
    }
    lines
}

#[test]
fn a_wider_terminal_grid_clips_instead_of_wrapping() {
    let t = Rope::from_str(&format!("{}\nnext\n", "x".repeat(100)));
    let lines = frame(&t, 0, 2, 80, Projected { truncate_lines: true, ..Projected::default() });
    assert_eq!((lines[0].width, lines[0].end, lines[1].start), (80, 100, 101));
    assert!(!lines[0].continued);
}

#[test]
fn conceals_omit_hidden_spans_in_terminal() {
    let t = Rope::from_str("See [[https://x.y][link]]\n");
    let conceals = [Conceal { from: 4, to: 19 }, Conceal { from: 23, to: 25 }];
    let lines = frame(&t, 0, 2, 80, Projected { conceals: &conceals, ..Projected::default() });
    let shown: String = lines[0].glyphs.iter().map(|g| g.shown.as_str()).collect();
    assert_eq!(shown, "See link");
    assert_eq!(lines[0].width, 8);
}

/// The cells a line of text takes.
pub fn width(text: &str) -> usize {
    text.graphemes(true).fold(0, |col, g| col + cells(g, col, usize::MAX).1)
}

/// Where a segment's visual lines start.
pub fn starts(text: &Rope, seg: Segment, cols: usize, inlays: Projected<'_>) -> Vec<usize> {
    // A line that starts in an inlay starts at the text it is before:
    // several can start there, and each counts (`segment::scroll_lines`).
    wrap(text, seg, cols, inlays).iter().map(|l| l.start).collect()
}

/// The visual lines shown from the scroll anchor, at most `rows`.
pub fn frame(text: &Rope, anchor: usize, rows: usize, cols: usize, inlays: Projected<'_>) -> Vec<Line> {
    let anchor = visible_pos(text, inlays.folds, anchor);
    let first = segment_at(text, anchor);
    let lines = wrap(text, first, cols, inlays);
    let skip = segment::line_index(&lines.iter().map(|l| l.start).collect::<Vec<_>>(), anchor);
    lines
        .into_iter()
        .skip(skip)
        .chain(
            std::iter::successors(next_visible_segment(text, first, inlays.folds), |&s| next_visible_segment(text, s, inlays.folds))
                .flat_map(|s| wrap(text, s, cols, inlays)),
        )
        .take(rows)
        .collect()
}

pub fn scroll_lines(text: &Rope, anchor: usize, n: i64, cols: usize, inlays: Projected<'_>) -> usize {
    segment::scroll_lines_folded(text, anchor, n, inlays.folds, &mut |seg| starts(text, seg, cols, inlays))
}

pub fn keep_visible(text: &Rope, anchor: usize, head: usize, rows: usize, cols: usize, inlays: Projected<'_>) -> Option<usize> {
    segment::keep_visible_folded(text, anchor, head, rows, inlays.folds, &mut |seg| starts(text, seg, cols, inlays))
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

/// The cell (row, column) showing the caret at `pos`, if it is in `lines`:
/// before an inlay there.
pub fn caret(lines: &[Line], pos: usize) -> Option<(usize, usize)> {
    lines.iter().enumerate().find_map(|(r, l)| {
        l.glyphs
            .iter()
            .find(|g| g.pos <= pos && pos < g.pos + g.len.max(g.inlay.is_some() as usize))
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
    fn word_wrap_preserves_text_and_positions_and_long_tokens_fall_back() {
        let t = Rope::from_str("alpha beta gamma");
        let prose = Projected { word_wrap: true, ..Projected::default() };
        let lines = wrap(&t, segment_at(&t, 0), 10, prose);
        assert_eq!(shown(&lines), ["alpha ", "beta gamma"]);
        assert_eq!(shown(&lines).concat(), t.to_string());
        assert_eq!(caret(&lines, 11), Some((1, 5)));
        assert_eq!(hit(&lines, 7, 1), Some(13));
        assert_eq!(scroll_lines(&t, 0, 1, 10, prose), 6);
        assert_eq!(frame(&t, 6, 1, 10, prose), lines[1..]);
        let long = Rope::from_str("abcdefghijk z");
        assert_eq!(shown(&wrap(&long, segment_at(&long, 0), 5, prose)), ["abcde", "fghij", "k z"]);
        let quote = Rope::from_str("- alpha beta gamma");
        let hangs = [Hang { line: 0, at: 2 }];
        let lines = wrap(&quote, segment_at(&quote, 0), 10, Projected { hangs: &hangs, ..prose });
        assert_eq!(shown(&lines), ["- alpha ", "beta ", "gamma"]);
        assert!(lines[1..].iter().all(|l| l.glyphs[0].col == 2));
        assert_eq!(shown(&lines).concat(), quote.to_string());
    }

    #[test]
    fn widths_tabs_and_controls() {
        let t = Rope::from_str("a\tb名\r\u{200b}e\u{301}x\u{1F600}");
        let lines = wrap(&t, segment_at(&t, 0), 80, Projected::default());
        let cols: Vec<(usize, usize)> = lines[0].glyphs.iter().map(|g| (g.col, g.width)).collect();
        assert_eq!(cols, [(0, 1), (1, 7), (8, 1), (9, 2), (11, 2), (13, 1), (14, 1), (15, 1), (16, 2)]);
        assert_eq!(shown(&lines), ["a       b名^M·e\u{301}x😀"]);
    }

    #[test]
    fn wrapping_wide_characters_and_tabs() {
        let t = Rope::from_str("abc名de\tf\nxy");
        let lines = wrap(&t, segment_at(&t, 0), 4, Projected::default());
        // The wide character does not fit after "abc": it starts the next line.
        assert_eq!(shown(&lines), ["abc", "名de", "    ", "f"]);
        assert_eq!(lines.iter().map(|l| l.continued).collect::<Vec<_>>(), [true, true, true, false]);
        assert_eq!(starts(&t, segment_at(&t, 0), 4, Projected::default()), [0, 3, 8, 9]);
        // Carets and clicks: on the wide character's second cell, past a
        // line's end, on the line break.
        assert_eq!(caret(&lines, 3), Some((1, 0)));
        assert_eq!(hit(&lines, 1, 1), Some(3));
        assert_eq!(hit(&lines, 3, 0), Some(2), "the end of a wrapped line is not on it");
        assert_eq!(caret(&lines, 10), Some((3, 1)));
        assert_eq!(hit(&lines, 3, 3), Some(10));
    }

    #[test]
    fn an_inlay_wrapping_over_lines_scrolls_as_it_is_drawn() {
        let t = Rope::from_str("ab\n");
        let inlays = [Inlay { at: 1, text: "123456789012".into(), style: 1 }];
        let lines = frame(&t, 0, 3, 4, Projected { inlays: &inlays, ..Projected::default() });
        let shown: Vec<String> = lines.iter().map(|l| l.glyphs.iter().map(|g| g.shown.as_str()).collect()).collect();
        assert_eq!(shown, ["a123", "4567", "8901"]);
        // "b" is on the fourth line, below: scrolled to.
        assert!(keep_visible(&t, 0, 2, 3, 4, Projected { inlays: &inlays, ..Projected::default() }).is_some());
    }

    #[test]
    fn frames_scroll_by_anchor() {
        let text: String = (0..50).map(|i| format!("{i} {}\n", "ab".repeat(i % 7))).collect();
        let t = Rope::from_str(&text);
        let cols = 6;
        let anchor = scroll_lines(&t, 0, 30, cols, Projected::default());
        let lines = frame(&t, anchor, 10, cols, Projected::default());
        assert_eq!(lines.len(), 10);
        assert_eq!(lines[0].start, anchor);
        assert_eq!(scroll_lines(&t, anchor, -30, cols, Projected::default()), 0);
        // Any position in a visual line shows that line first.
        assert_eq!(frame(&t, lines[1].end - 1, 3, cols, Projected::default())[0], lines[1]);
        // The caret below the frame: scrolled to be on the last row.
        let below = lines[9].end + 1;
        let a = keep_visible(&t, anchor, below, 10, cols, Projected::default()).unwrap();
        assert_eq!(caret(&frame(&t, a, 10, cols, Projected::default()), below).map(|c| c.0), Some(9));
    }

    #[test]
    fn a_hanging_line_wraps_under_its_hang_point() {
        let t = Rope::from_str("- one two three four five\nnext\n");
        let hangs = [Hang { line: 0, at: 2 }];
        let lines = wrap(&t, segment_at(&t, 0), 10, Projected { hangs: &hangs, ..Projected::default() });
        assert!(lines.len() > 2);
        assert!(lines[1..].iter().all(|l| l.glyphs[0].col == 2), "{lines:?}");
        // A click left of the hang point is on the line's first character.
        assert_eq!(hit(&lines, 0, 1), Some(lines[1].start));
        assert_eq!(caret(&lines, lines[1].start), Some((1, 2)));
    }

    #[test]
    fn every_piece_of_a_long_line_hangs() {
        let t = Rope::from_str(&format!("- {}\n", "word ".repeat(2000)));
        let hangs = [Hang { line: 0, at: 2 }];
        let shown = Projected { hangs: &hangs, ..Projected::default() };
        let later = segment_at(&t, 6000);
        assert!(later.start > 0, "a later piece");
        let lines = wrap(&t, later, 20, shown);
        assert!(lines.iter().all(|l| l.glyphs[0].col == 2), "{:?}", &lines[..2]);
    }

    #[test]
    fn a_later_piece_hangs_where_the_first_does_with_its_inlays() {
        let t = Rope::from_str(&format!("- {}\n", "word ".repeat(2000)));
        let hangs = [Hang { line: 0, at: 2 }];
        let inlays = [Inlay { at: 1, text: "hint".into(), style: 1 }];
        let shown = Projected { inlays: &inlays, hangs: &hangs, ..Projected::default() };
        let first = wrap(&t, segment_at(&t, 0), 20, shown);
        let indent = first[1].glyphs[0].col;
        assert_eq!(indent, 6, "under the first word, past the inlay");
        let later = wrap(&t, segment_at(&t, 6000), 20, shown);
        assert!(later.iter().all(|l| l.glyphs[0].col == indent), "{:?}", later[0]);
    }

    #[test]
    fn folded_lines_are_omitted_from_terminal_frame() {
        let t = Rope::from_str("* One\nhidden 1\nhidden 2\n* Two\n");
        let folds = [Fold { from: t.line_to_byte(1), to: t.line_to_byte(3) }];
        let lines = frame(&t, 0, 10, 40, Projected { folds: &folds, ..Projected::default() });
        let shown: Vec<String> = lines.iter().map(|l| l.glyphs.iter().map(|g| g.shown.as_str()).collect()).collect();
        assert_eq!(shown, ["* One", "* Two", ""]);
    }
}
