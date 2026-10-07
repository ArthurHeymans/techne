//! which-key's layout, shared by the frontends: the keys that can follow a
//! prefix arranged in columns to fit a width, in characters (cells, or the
//! text font's advance in a window).

use crate::present::KeyHint;

/// Cells (characters) between the columns.
pub const GAP: usize = 2;

/// The hints in columns, in order down each column, in as few rows (up to
/// `max_rows`) as fit `width`; each column as wide as its widest key,
/// " : " and its widest description. What does not fit even then is left
/// out.
pub fn columns<'a>(
    hints: &'a [KeyHint],
    max_rows: usize,
    width: usize,
    key: impl Fn(&KeyHint) -> usize,
    description: impl Fn(&KeyHint) -> usize,
) -> Vec<Vec<&'a KeyHint>> {
    if hints.is_empty() || max_rows == 0 {
        return Vec::new();
    }
    let layout = |rows: usize| -> (Vec<Vec<&'a KeyHint>>, usize) {
        let columns: Vec<Vec<&KeyHint>> = hints.chunks(rows).map(|c| c.iter().collect()).collect();
        let used = columns
            .iter()
            .map(|c| c.iter().map(|h| key(h)).max().unwrap_or(0) + 3 + c.iter().map(|h| description(h)).max().unwrap_or(0))
            .sum::<usize>()
            + GAP * (columns.len() - 1);
        (columns, used)
    };
    let fits = (1..=max_rows).map(layout).find(|(_, used)| *used <= width);
    match fits {
        Some((columns, _)) => columns,
        // Too many: the columns that fit, at the most rows.
        None => {
            let (columns, _) = layout(max_rows);
            columns
                .into_iter()
                .scan(0, |used, c| {
                    *used += c.iter().map(|h| key(h)).max().unwrap_or(0) + 3 + c.iter().map(|h| description(h)).max().unwrap_or(0) + GAP;
                    (*used <= width + GAP).then_some(c)
                })
                .collect()
        }
    }
}
