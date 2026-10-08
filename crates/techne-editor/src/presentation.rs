//! Presentations (EDITOR.md, section 2): text made of keyed logical rows,
//! for buffers whose content is not a document of their own: structured
//! views (the inspector, a list of TODOs, a status), lenses of excerpts of
//! documents, a REPL's transcript.
//!
//! A row has a key and runs of text, each with an optional face; a run may
//! be an excerpt of a document, which is edited through to it
//! (`crate::lens`). Columns of rows are aligned when the rows are set, in
//! characters. The text is the rows' texts between line breaks, so a view
//! moves, searches, selects and copies in it as in any text, and a frontend
//! draws it as text; a row's text may hold line breaks of its own (an
//! excerpt of several lines).
//!
//! A row's identity is its key, not its line. Setting new rows changes the
//! text only where rows differ, as one change of the text: carets and scroll
//! anchors on a row that stays follow it, whatever was added or removed
//! around it. A row moved among the others is shown again, not followed.
//!
//! A presentation has no history of its own: its revisions are changes of
//! what it shows, kept a bounded number back to map positions of earlier
//! snapshots. What is edited through it is in its sources' histories.

use std::{cell::RefCell, collections::HashMap, ops::Range, rc::Rc};

use techne_text::{Assoc, ChangeSet, Document, Revision, ropey::Rope};

use crate::present::Highlight;

/// How many revisions back positions are mapped.
const KEPT: usize = 1024;

/// What an excerpt shows of its document: `range` at revision `base`.
pub(crate) struct Source {
    pub doc: Rc<RefCell<Document>>,
    pub range: Range<usize>,
    pub base: Revision,
}

pub(crate) struct Run {
    pub text: String,
    pub face: Option<String>,
    pub source: Option<Source>,
}

pub(crate) struct Row {
    pub key: String,
    pub runs: Vec<Run>,
}

impl Row {
    fn text(&self) -> String {
        self.runs.iter().map(|r| r.text.as_str()).collect()
    }

    fn len(&self) -> usize {
        self.runs.iter().map(|r| r.text.len()).sum()
    }
}

/// A run as it is given: text, or an excerpt of a document.
pub enum Content {
    Text(String),
    Excerpt(Rc<RefCell<Document>>, Range<usize>),
}

pub struct RunSpec {
    pub content: Content,
    pub face: Option<String>,
}

/// A row as it is given: its key and columns of runs.
pub struct RowSpec {
    pub key: String,
    pub columns: Vec<Vec<RunSpec>>,
}

pub struct Presentation {
    pub(crate) rows: Vec<Row>,
    /// Where each row starts in the text.
    starts: Vec<usize>,
    text: Rope,
    /// The revision before `log[0]`.
    first: Revision,
    log: Vec<ChangeSet>,
    /// Edits made through it, for undo (`crate::lens`).
    pub(crate) undo: Vec<crate::lens::Op>,
    pub(crate) redo: Vec<crate::lens::Op>,
}

impl Default for Presentation {
    fn default() -> Self {
        Presentation::new()
    }
}

impl Presentation {
    pub fn new() -> Presentation {
        Presentation {
            rows: Vec::new(),
            starts: Vec::new(),
            text: Rope::new(),
            first: 0,
            log: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub fn text(&self) -> &Rope {
        &self.text
    }

    pub fn len(&self) -> usize {
        self.text.len_bytes()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn revision(&self) -> Revision {
        self.first + self.log.len() as u64
    }

    /// The changes after `rev`, oldest first; `None` when that revision is
    /// too far back.
    pub fn changes_since(&self, rev: Revision) -> Option<&[ChangeSet]> {
        let i = rev.checked_sub(self.first)? as usize;
        self.log.get(i..)
    }

    /// Where `pos` at revision `rev` is now, and whether the text around it
    /// was replaced since (as `Document::map_pos`).
    pub fn map_pos(&self, pos: usize, assoc: Assoc, rev: Revision) -> Option<(usize, bool)> {
        let changes = self.changes_since(rev)?;
        if pos > changes.first().map_or(self.len(), |c| c.len_before()) {
            return None;
        }
        changes.iter().try_fold((pos, false), |(p, deleted), c| {
            let (q, d) = c.map(p, assoc);
            Some((q, deleted || d))
        })
    }

    /// Show `rows`: the text changes where they differ from the rows shown,
    /// row by key. Keys must differ.
    pub fn set_rows(&mut self, rows: Vec<RowSpec>) -> Result<(), String> {
        let rows = render(rows)?;
        let changes = self.diff(&rows);
        self.rows = rows;
        self.commit(changes);
        Ok(())
    }

    /// Apply `changes`, which take the text to what the rows say now.
    pub(crate) fn commit(&mut self, changes: ChangeSet) {
        changes.apply(&mut self.text);
        self.starts = self
            .rows
            .iter()
            .scan(0, |at, r| {
                let start = *at;
                *at += r.len() + 1;
                Some(start)
            })
            .collect();
        debug_assert_eq!(self.text.len_bytes(), self.rows.iter().map(|r| r.len() + 1).sum::<usize>().saturating_sub(1));
        if changes.is_identity() {
            return;
        }
        self.log.push(changes);
        if self.log.len() > KEPT {
            let drop = self.log.len() - KEPT;
            self.log.drain(..drop);
            self.first += drop as u64;
        }
    }

    /// The change from the text shown to that of `rows`: rows that stay, in
    /// the longest order they keep, are changed in place; the others are
    /// replaced around them.
    fn diff(&self, rows: &[Row]) -> ChangeSet {
        let old: HashMap<&str, usize> = self.rows.iter().enumerate().map(|(i, r)| (r.key.as_str(), i)).collect();
        let pairs: Vec<(usize, usize)> = rows.iter().enumerate().filter_map(|(n, r)| old.get(r.key.as_str()).map(|&o| (n, o))).collect();
        let kept = longest_increasing(&pairs);
        let joined = |rows: &[Row]| rows.iter().map(Row::text).collect::<Vec<_>>().join("\n");
        let end = |o: usize| self.starts[o] + self.rows[o].len();
        let mut edits: Vec<(Range<usize>, String)> = Vec::new();
        // Between the previous row kept (old index, new index) and the next.
        let mut prev: Option<(usize, usize)> = None;
        for &(n, o) in &kept {
            let between = &rows[prev.map_or(0, |(pn, _)| pn + 1)..n];
            let text = match prev {
                Some(_) => format!("\n{}", between.iter().map(|r| r.text() + "\n").collect::<String>()),
                None => between.iter().map(|r| r.text() + "\n").collect(),
            };
            edits.push((prev.map_or(0, |(_, po)| end(po))..self.starts[o], text));
            edits.push((self.starts[o]..end(o), rows[n].text()));
            prev = Some((n, o));
        }
        let rest = &rows[prev.map_or(0, |(pn, _)| pn + 1)..];
        let text = match prev {
            Some(_) if rest.is_empty() => String::new(),
            Some(_) => format!("\n{}", joined(rest)),
            None => joined(rest),
        };
        edits.push((prev.map_or(0, |(_, po)| end(po))..self.len(), text));
        self.change(edits)
    }

    /// The change making `edits` (in order) of the text, each without what
    /// its old and new text share at either end, so that a row changed in
    /// place keeps the positions around what changed in it.
    pub(crate) fn change(&self, edits: Vec<(Range<usize>, String)>) -> ChangeSet {
        let edits = edits.into_iter().map(|(r, new)| {
            let old = self.text.byte_slice(r.clone()).to_string();
            let prefix: usize = old.chars().zip(new.chars()).take_while(|(a, b)| a == b).map(|(a, _)| a.len_utf8()).sum();
            let suffix: usize =
                old[prefix..].chars().rev().zip(new[prefix..].chars().rev()).take_while(|(a, b)| a == b).map(|(a, _)| a.len_utf8()).sum();
            (r.start + prefix..r.end - suffix, new[prefix..new.len() - suffix].to_string())
        });
        ChangeSet::from_edits(self.len(), edits.filter(|(r, t)| !(r.is_empty() && t.is_empty()))).expect("edits are in order")
    }

    /// The row a position is in, by index: a position on a line break
    /// between rows is the end of the row before.
    fn row_index(&self, pos: usize) -> Option<usize> {
        if self.rows.is_empty() || pos > self.len() {
            return None;
        }
        Some(self.starts.partition_point(|&s| s <= pos) - 1)
    }

    /// The key of the row at `pos`.
    pub fn key_at(&self, pos: usize) -> Option<&str> {
        self.row_index(pos).map(|i| self.rows[i].key.as_str())
    }

    /// Where the row of `key` is: from its start to its end, without the
    /// line break.
    pub fn row_span(&self, key: &str) -> Option<Range<usize>> {
        let i = self.rows.iter().position(|r| r.key == key)?;
        Some(self.starts[i]..self.starts[i] + self.rows[i].len())
    }

    /// Each run with where it is in the text, in order.
    pub(crate) fn placed(&self) -> impl Iterator<Item = (usize, usize, Range<usize>)> + '_ {
        self.rows.iter().enumerate().flat_map(move |(i, row)| {
            row.runs.iter().enumerate().scan(self.starts[i], move |at, (j, run)| {
                let span = *at..*at + run.text.len();
                *at = span.end;
                Some((i, j, span))
            })
        })
    }

    /// The runs' faces between `from` and `to`, and excerpts whose source
    /// changed under them since they were shown, as warnings.
    pub fn highlights(&self, from: usize, to: usize) -> Vec<Highlight> {
        let first = self.row_index(from).unwrap_or(0);
        self.rows
            .iter()
            .enumerate()
            .skip(first)
            .take_while(|(i, _)| self.starts[*i] < to)
            .flat_map(|(i, row)| {
                row.runs.iter().scan(self.starts[i], |at, run| {
                    let span = *at..*at + run.text.len();
                    *at = span.end;
                    let face = match &run.source {
                        Some(s) if crate::lens::now(s).is_err() => Some("warning".to_string()),
                        _ => run.face.clone(),
                    };
                    Some(face.map(|face| Highlight { from: span.start, to: span.end, face }))
                })
            })
            .flatten()
            .filter(|h| h.from < h.to && h.from < to && h.to > from)
            .collect()
    }
}

/// The rows' runs with excerpts read and columns aligned: each column but
/// a row's last is padded to the widest of its kind, plus two spaces.
fn render(rows: Vec<RowSpec>) -> Result<Vec<Row>, String> {
    let run = |spec: RunSpec| -> Result<Run, String> {
        Ok(match spec.content {
            Content::Text(text) => Run { text, face: spec.face, source: None },
            Content::Excerpt(doc, range) => {
                let (text, base) = {
                    let d = doc.borrow();
                    let t = d.text();
                    let ok = |p: usize| p <= t.len_bytes() && t.char_to_byte(t.byte_to_char(p)) == p;
                    if range.start > range.end || !ok(range.start) || !ok(range.end) {
                        return Err(format!("{}..{} is not in the document", range.start, range.end));
                    }
                    (t.byte_slice(range.clone()).to_string(), d.revision())
                };
                Run { text, face: spec.face, source: Some(Source { doc, range, base }) }
            }
        })
    };
    let rows = rows
        .into_iter()
        .map(|r| Ok((r.key, r.columns.into_iter().map(|c| c.into_iter().map(run).collect()).collect::<Result<Vec<Vec<Run>>, String>>()?)))
        .collect::<Result<Vec<_>, String>>()?;
    let width = |c: &[Run]| c.iter().map(|r| r.text.chars().count()).sum::<usize>();
    let widths = rows.iter().fold(Vec::<usize>::new(), |mut w, (_, columns)| {
        for (i, c) in columns.iter().enumerate().take(columns.len().saturating_sub(1)) {
            if w.len() <= i {
                w.push(0);
            }
            w[i] = w[i].max(width(c));
        }
        w
    });
    let mut seen = std::collections::HashSet::new();
    rows.into_iter()
        .map(|(key, columns)| {
            if !seen.insert(key.clone()) {
                return Err(format!("two rows have the key {key:?}"));
            }
            let last = columns.len().saturating_sub(1);
            let runs = columns
                .into_iter()
                .enumerate()
                .flat_map(|(i, c)| {
                    let pad = if i < last { widths[i] + 2 - width(&c) } else { 0 };
                    c.into_iter().chain((pad > 0).then(|| Run { text: " ".repeat(pad), face: None, source: None }))
                })
                .collect();
            Ok(Row { key, runs })
        })
        .collect()
}

/// The longest run of `(new, old)` pairs (in new order) whose old indices
/// increase.
fn longest_increasing(pairs: &[(usize, usize)]) -> Vec<(usize, usize)> {
    // For each length, the index of the pair ending the best run so far.
    let mut tails: Vec<usize> = Vec::new();
    let mut before = vec![None; pairs.len()];
    for (i, &(_, o)) in pairs.iter().enumerate() {
        let k = tails.partition_point(|&t| pairs[t].1 < o);
        before[i] = k.checked_sub(1).map(|k| tails[k]);
        if k == tails.len() {
            tails.push(i);
        } else {
            tails[k] = i;
        }
    }
    let mut out = Vec::new();
    let mut at = tails.last().copied();
    while let Some(i) = at {
        out.push(pairs[i]);
        at = before[i];
    }
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(key: &str, columns: &[&str]) -> RowSpec {
        RowSpec {
            key: key.into(),
            columns: columns.iter().map(|c| vec![RunSpec { content: Content::Text(c.to_string()), face: None }]).collect(),
        }
    }

    fn text(p: &Presentation) -> String {
        p.text().to_string()
    }

    #[test]
    fn columns_align_and_rows_keep_their_identity() {
        let mut p = Presentation::new();
        p.set_rows(vec![row("a", &["a", "one"]), row("b", &["bbb", "two"])]).unwrap();
        assert_eq!(text(&p), "a    one\nbbb  two");
        let rev = p.revision();
        // A caret on the start of b and one inside it.
        let (start, inside) = (9, 14);
        p.set_rows(vec![row("z", &["z", "new"]), row("b", &["bbb", "two!"]), row("c", &["c", "three"])]).unwrap();
        assert_eq!(text(&p), "z    new\nbbb  two!\nc    three");
        assert_eq!(p.map_pos(start, Assoc::After, rev), Some((9, false)), "b's start follows b");
        assert_eq!(p.map_pos(inside, Assoc::After, rev), Some((14, false)));
        assert_eq!(p.key_at(14), Some("b"));
        assert_eq!(p.key_at(18), Some("b"), "the end of a row is in it");
        assert_eq!(p.row_span("c"), Some(19..29));
    }

    #[test]
    fn rows_that_stay_are_followed_where_others_come_and_go() {
        let mut p = Presentation::new();
        let rows = |keys: &[&str]| keys.iter().map(|k| row(k, &[k])).collect::<Vec<_>>();
        p.set_rows(rows(&["a", "b", "c", "d"])).unwrap();
        let rev = p.revision();
        p.set_rows(rows(&["x", "c", "a", "d", "y"])).unwrap();
        assert_eq!(text(&p), "x\nc\na\nd\ny");
        // a and d keep their order (as long a run as c and d); c moved.
        assert_eq!(p.map_pos(0, Assoc::After, rev), Some((4, false)), "a");
        assert_eq!(p.map_pos(6, Assoc::After, rev), Some((6, false)), "d");
        assert!(p.map_pos(4, Assoc::After, rev).unwrap().1, "c was shown again");
        let first = p.revision();
        p.set_rows(rows(&["x", "c", "a", "d", "y"])).unwrap();
        assert_eq!(p.revision(), first, "the same rows change nothing");
        assert!(p.set_rows(rows(&["a", "a"])).is_err(), "keys differ");
    }

    #[test]
    fn faces_of_runs() {
        let mut p = Presentation::new();
        let face = |t: &str, f: &str| RunSpec { content: Content::Text(t.into()), face: Some(f.into()) };
        p.set_rows(vec![RowSpec { key: "k".into(), columns: vec![vec![face("label", "comment")], vec![face("value", "string")]] }])
            .unwrap();
        let h = p.highlights(0, p.len());
        assert_eq!(h.iter().map(|h| (h.from, h.to, h.face.as_str())).collect::<Vec<_>>(), [(0, 5, "comment"), (7, 12, "string")]);
    }
}
