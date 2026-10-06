//! Layout in terminal cells (EDITOR.md, section 8): graphemes take the width
//! Unicode gives them, a wide character never splits across rows, tabs go
//! to the next multiple of 8 and control characters show as ^X. As in
//! Emacs, the last column is kept for a `\` on rows that continue, so a
//! caret at the end of a row always has a cell.

use techne_editor::display::{Segment, Wrapping};
use techne_text::ropey::Rope;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

const TAB: usize = 8;

/// One grapheme on a row: its bytes in the text, its column and width, and
/// what is drawn for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Glyph {
    pub start: usize,
    pub end: usize,
    pub col: usize,
    pub width: usize,
    pub show: String,
}

/// Wraps text at a number of columns.
pub struct Cells {
    /// Columns for text, one less than the terminal's.
    pub width: usize,
}

impl Cells {
    pub fn new(columns: usize) -> Cells {
        Cells { width: columns.saturating_sub(1).max(1) }
    }

    /// The segment's rows of glyphs; an empty segment has one empty row.
    pub fn rows(&self, text: &Rope, seg: Segment) -> Vec<Vec<Glyph>> {
        let s = text.byte_slice(seg.start..seg.end).to_string();
        let mut rows = vec![Vec::new()];
        let mut col = 0;
        let width = self.width;
        for (i, g) in s.grapheme_indices(true) {
            let shown = |col: usize| -> (String, usize) {
                match g {
                    // To the next tab stop, or the end of the row.
                    "\t" => {
                        let w = (TAB - col % TAB).min(width - col).max(1);
                        (" ".repeat(w), w)
                    }
                    g if g.chars().all(char::is_control) => {
                        let c = g.chars().next().expect("a grapheme has a char") as u32;
                        (format!("^{}", char::from_u32((c + 64) % 128).unwrap_or('?')), 2)
                    }
                    // Zero-width alone (a stray joiner) still takes a cell to
                    // put the cursor on.
                    g => match g.width() {
                        0 => (" ".into(), 1),
                        w => (g.into(), w),
                    },
                }
            };
            let (mut show, mut width) = shown(col);
            if col > 0 && col + width > self.width {
                rows.push(Vec::new());
                col = 0;
                (show, width) = shown(0);
            }
            rows.last_mut().expect("a row").push(Glyph { start: seg.start + i, end: seg.start + i + g.len(), col, width, show });
            col += width;
        }
        rows
    }
}

impl Wrapping for Cells {
    fn visual_starts(&mut self, text: &Rope, seg: Segment) -> Vec<usize> {
        let rows = self.rows(text, seg);
        rows.iter().enumerate().map(|(i, r)| if i == 0 { seg.start } else { r[0].start }).collect()
    }
}

/// Where `pos` is drawn in a segment's rows: (row, column). The end of the
/// segment is after its last glyph.
pub fn locate(rows: &[Vec<Glyph>], pos: usize) -> (usize, usize) {
    for (r, row) in rows.iter().enumerate() {
        if let Some(g) = row.iter().find(|g| g.start <= pos && pos < g.end) {
            return (r, g.col);
        }
    }
    let r = rows.len() - 1;
    (r, rows[r].last().map_or(0, |g| g.col + g.width))
}

/// The text position for a click at `col` on a row: the glyph there (both
/// cells of a wide one), or the end of the row past its text. A wrapped
/// row's end is before its last glyph, since its boundary belongs to the
/// next row.
pub fn position_at(rows: &[Vec<Glyph>], row: usize, col: usize, seg_end: usize) -> usize {
    let r = &rows[row.min(rows.len() - 1)];
    if let Some(g) = r.iter().find(|g| g.col <= col && col < g.col + g.width) {
        return g.start;
    }
    match r.last() {
        Some(last) if row + 1 < rows.len() => last.start,
        _ => seg_end,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(text: &str, columns: usize) -> Vec<Vec<Glyph>> {
        let t = Rope::from_str(text);
        Cells::new(columns).rows(&t, Segment { start: 0, end: text.len() })
    }

    fn shown(rows: &[Vec<Glyph>]) -> Vec<String> {
        rows.iter().map(|r| r.iter().map(|g| g.show.as_str()).collect()).collect()
    }

    #[test]
    fn widths_wrapping_tabs_and_controls() {
        // 5 columns of text in a 6-column terminal.
        assert_eq!(shown(&layout("abcdefg", 6)), ["abcde", "fg"]);
        // A wide character that does not fit moves to the next row whole.
        let rows = layout("abcd日本", 6);
        assert_eq!(shown(&rows), ["abcd", "日本"]);
        assert_eq!(rows[1][1].col, 2);
        assert_eq!(shown(&layout("a\tb", 20)), ["a       b"]);
        assert_eq!(shown(&layout("\tb", 6)), ["     ", "b"]);
        assert_eq!(shown(&layout("a\u{1}b", 20)), ["a^Ab"]);
        // Combining marks stay with their base; an emoji sequence is one glyph.
        let rows = layout("e\u{301}👩\u{200d}💻x", 20);
        assert_eq!(rows[0].len(), 3);
        assert_eq!((rows[0][1].col, rows[0][1].width, rows[0][2].col), (1, 2, 3));
    }

    #[test]
    fn locating_and_clicking() {
        let text = "ab日本cdef";
        let rows = layout(text, 6);
        assert_eq!(shown(&rows), ["ab日", "本cde", "f"]);
        assert_eq!(locate(&rows, 2), (0, 2));
        assert_eq!(locate(&rows, 5), (1, 0));
        assert_eq!(locate(&rows, text.len()), (2, 1));
        // Either cell of a wide character is that character.
        assert_eq!(position_at(&rows, 0, 3, text.len()), 2);
        // Past the text of a wrapped row: before its last glyph.
        assert_eq!(position_at(&rows, 0, 5, text.len()), 2);
        assert_eq!(position_at(&rows, 2, 9, text.len()), text.len());
    }
}
