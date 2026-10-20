//! Matching the minibuffer's candidates (`lisp/editor/minibuffer.scm`):
//! every key filters them all, up to the lines of a large file, which Lisp
//! comparing strings one by one cannot do within the keystroke budget.
//!
//! A `Matcher` holds the texts of its entries, folded to lower case once,
//! and the entries matching each pattern typed since the input last
//! shrank: a pattern extending the last narrows its matches, and deleting
//! goes back to those of the shorter pattern. A pattern's parts (its
//! words) must each occur in an entry, without case (both folded) unless
//! the part has an upper-case letter. Copies share the texts and filter
//! on their own, one per minibuffer.

use std::{cell::OnceCell, rc::Rc};

use memchr::memmem::Finder;

/// A folded text, with spans of its own unless it has the original's.
type Folded = (String, Option<Vec<(u32, u32)>>);

/// What copies of a matcher share.
struct Entries {
    /// The entries' texts, at `spans` of `text`.
    text: String,
    spans: Vec<(u32, u32)>,
    /// The same, folded to lower case, once a pattern needs it: at the
    /// same spans when the text is ASCII (folding keeps its length), else
    /// each entry folded on its own, as Lisp's `string-downcase` folds it.
    folded: OnceCell<Folded>,
    /// For documents' lines: each entry's start and line number in its
    /// document, and which document it is.
    lines: Vec<(usize, usize, usize)>,
}

pub struct Matcher {
    entries: Rc<Entries>,
    /// Patterns, each extending the one before, with their matches; the
    /// first is the empty pattern, which every entry matches.
    stack: Vec<(String, Vec<u32>)>,
}

impl Matcher {
    pub fn new(texts: Vec<String>) -> Matcher {
        let text = texts.concat();
        let spans = texts
            .iter()
            .scan(0, |at, t| {
                *at += t.len();
                Some(((*at - t.len()) as u32, *at as u32))
            })
            .collect();
        Matcher::of(Entries { text, spans, folded: OnceCell::new(), lines: Vec::new() })
    }

    /// The non-empty lines of `rope`, each with its start and number,
    /// without its line break.
    pub fn of_lines(rope: &techne_text::ropey::Rope) -> Matcher {
        Matcher::of_lines_of([rope])
    }

    /// The non-empty lines of each of `ropes`, in order, each with its start
    /// and number in its rope, and which rope it is.
    pub fn of_lines_of<'a>(ropes: impl IntoIterator<Item = &'a techne_text::ropey::Rope>) -> Matcher {
        let (mut text, mut spans, mut lines) = (String::new(), Vec::new(), Vec::new());
        for (doc, rope) in ropes.into_iter().enumerate() {
            // Documents' ropes break lines only at line feeds (techne-text
            // builds ropey without its Unicode and CR line breaks).
            let base = text.len();
            text.extend(rope.chunks());
            let mut start = base;
            for (i, end) in memchr::memchr_iter(b'\n', &text.as_bytes()[base..]).map(|e| base + e).chain([text.len()]).enumerate() {
                let to = if text[start..end].ends_with('\r') { end - 1 } else { end };
                if to > start {
                    spans.push((start as u32, to as u32));
                    lines.push((start - base, i + 1, doc));
                }
                start = end + 1;
            }
        }
        Matcher::of(Entries { text, spans, folded: OnceCell::new(), lines })
    }

    fn of(entries: Entries) -> Matcher {
        Matcher::sharing(Rc::new(entries))
    }

    fn sharing(entries: Rc<Entries>) -> Matcher {
        let all = (0..entries.spans.len() as u32).collect();
        Matcher { entries, stack: vec![(String::new(), all)] }
    }

    /// A matcher of the same entries, filtering on its own.
    pub fn copy(&self) -> Matcher {
        Matcher::sharing(self.entries.clone())
    }

    pub fn len(&self) -> usize {
        self.entries.spans.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.spans.is_empty()
    }

    pub fn text(&self, entry: usize) -> Option<&str> {
        self.entries.spans.get(entry).map(|&(from, to)| &self.entries.text[from as usize..to as usize])
    }

    /// The first entry whose text is `text`.
    pub fn find(&self, text: &str) -> Option<usize> {
        self.entries.spans.iter().position(|&(from, to)| &self.entries.text[from as usize..to as usize] == text)
    }

    /// A line entry's start and line number in its document, and which
    /// document it is.
    pub fn line(&self, entry: usize) -> Option<(usize, usize, usize)> {
        self.entries.lines.get(entry).copied()
    }

    /// Match `pattern`; return how many entries match.
    pub fn filter(&mut self, pattern: &str) -> usize {
        while !pattern.starts_with(self.stack.last().expect("the empty pattern").0.as_str()) {
            self.stack.pop();
        }
        let (last, within) = self.stack.last().expect("the empty pattern");
        if last != pattern {
            let parts: Vec<(bool, Finder)> = pattern
                .split(' ')
                .filter(|p| !p.is_empty())
                .map(|p| match p.chars().any(char::is_uppercase) {
                    true => (false, Finder::new(p.as_bytes()).into_owned()),
                    false => (true, Finder::new(p.to_lowercase().as_bytes()).into_owned()),
                })
                .collect();
            let e = &self.entries;
            // From every entry, the longest part, if rare, is found in all
            // the text at once, not in each entry on its own, some 100
            // instructions an entry: a lot for the lines of a 100k-line
            // file. A common one gives up early, and each entry is searched.
            let longest = parts.iter().max_by_key(|(_, f)| f.needle().len());
            let scanned = (within.len() == e.spans.len()).then_some(longest).flatten().and_then(|(fold, f)| {
                let (text, spans) = if *fold { e.folded_in_place()? } else { (e.text.as_str(), e.spans.as_slice()) };
                entries_with(text, spans, f)
            });
            let found = scanned
                .as_deref()
                .unwrap_or(within)
                .iter()
                .copied()
                .filter(|&i| {
                    parts.iter().all(|(fold, f)| {
                        let (text, spans) = if *fold { e.folded() } else { (e.text.as_str(), e.spans.as_slice()) };
                        let (from, to) = spans[i as usize];
                        f.find(&text.as_bytes()[from as usize..to as usize]).is_some()
                    })
                })
                .collect();
            self.stack.push((pattern.to_string(), found));
        }
        self.matches().len()
    }

    /// The entries matching the last pattern, in order.
    pub fn matches(&self) -> &[u32] {
        &self.stack.last().expect("the empty pattern").1
    }
}

/// The entries, at `spans` of `text` in order, that `finder`'s needle occurs
/// in; `None` once more than one in 16 of those scanned has it, past the
/// first few: each entry is then searched as cheaply.
fn entries_with(text: &str, spans: &[(u32, u32)], finder: &Finder) -> Option<Vec<u32>> {
    let (text, n) = (text.as_bytes(), finder.needle().len());
    let mut found = Vec::new();
    let (mut at, mut entry) = (0, 0);
    while let Some(i) = finder.find(&text[at.min(text.len())..]).map(|i| at + i) {
        entry += spans[entry..].partition_point(|&(_, to)| to as usize <= i);
        match spans.get(entry) {
            Some(&(from, to)) if from as usize <= i && i + n <= to as usize => {
                if found.len() >= 64 && found.len() * 16 > entry {
                    return None;
                }
                found.push(entry as u32);
                at = to as usize;
            }
            _ => at = i + 1,
        }
    }
    Some(found)
}

impl Entries {
    fn folded(&self) -> (&str, &[(u32, u32)]) {
        let (folded, spans) = self.folded.get_or_init(|| {
            if self.text.is_ascii() {
                return (self.text.to_ascii_lowercase(), None);
            }
            // Most entries are ASCII even where one is not: they keep their
            // spans in the text folded as ASCII, and the others, which
            // folding may lengthen, are folded after it.
            let mut folded = self.text.to_ascii_lowercase();
            let spans = self
                .spans
                .iter()
                .map(|&(from, to)| {
                    let entry = &self.text[from as usize..to as usize];
                    if entry.is_ascii() {
                        return (from, to);
                    }
                    let at = folded.len();
                    folded.push_str(&techne_text::motion::lowered(entry).0);
                    (at as u32, folded.len() as u32)
                })
                .collect();
            (folded, Some(spans))
        });
        (folded, spans.as_deref().unwrap_or(&self.spans))
    }

    /// The text folded, if its entries are where they are in the text, in
    /// order: when it is ASCII.
    fn folded_in_place(&self) -> Option<(&str, &[(u32, u32)])> {
        let folded = self.folded();
        matches!(self.folded.get(), Some((_, None))).then_some(folded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matching(m: &mut Matcher, pattern: &str) -> Vec<u32> {
        m.filter(pattern);
        m.matches().to_vec()
    }

    #[test]
    fn parts_match_in_any_order_without_case_unless_upper_case() {
        let mut m = Matcher::new(["Alpha beta", "beta gamma", "alphabet"].map(String::from).to_vec());
        assert_eq!(matching(&mut m, "alpha"), [0, 2]);
        assert_eq!(matching(&mut m, "alpha bet"), [0, 2]);
        assert_eq!(matching(&mut m, "bet Alpha"), [0]);
        // Deleting goes back; a new pattern starts from all.
        assert_eq!(matching(&mut m, "al"), [0, 2]);
        assert_eq!(matching(&mut m, "gamma"), [1]);
        assert_eq!(matching(&mut m, ""), [0, 1, 2]);
        // A copy filters on its own.
        let mut copy = m.copy();
        assert_eq!((matching(&mut copy, "gamma"), m.matches()), (vec![1], &[0, 1, 2][..]));
    }

    #[test]
    fn lines_skip_empty_ones() {
        let mut m = Matcher::of_lines(&techne_text::ropey::Rope::from_str("one\n\ntwo\r\nthree"));
        assert_eq!((m.len(), m.text(1), m.line(1)), (3, Some("two"), Some((5, 3, 0))));
        // Several documents' lines, each where it is in its own.
        let ropes = ["a\nb", "\nc"].map(techne_text::ropey::Rope::from_str);
        let several = Matcher::of_lines_of(&ropes);
        assert_eq!((several.len(), several.text(2), several.line(2)), (3, Some("c"), Some((1, 2, 1))));
        assert_eq!(matching(&mut m, "t"), [1, 2]);
        // Folded to lower case beyond ASCII too, as words fold (a final
        // sigma), parts as well (a title-case letter, not upper case).
        let mut m = Matcher::new(vec!["ÉÉN".into(), "ΟΣ".into(), "ǅA".into()]);
        assert_eq!(matching(&mut m, "één"), [0]);
        assert_eq!(matching(&mut m, "ος"), [1]);
        assert_eq!(matching(&mut m, "ǅ"), [2]);
        assert_eq!(matching(&mut m, "ǅA"), [2]);
    }
}
