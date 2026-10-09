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
    /// For a document's lines: each entry's start and line number.
    lines: Vec<(usize, usize)>,
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
        // Documents' ropes break lines only at line feeds (techne-text
        // builds ropey without its Unicode and CR line breaks).
        let text: String = rope.chunks().collect();
        let (mut spans, mut lines, mut start) = (Vec::new(), Vec::new(), 0);
        for (i, end) in memchr::memchr_iter(b'\n', text.as_bytes()).chain([text.len()]).enumerate() {
            let to = if text[start..end].ends_with('\r') { end - 1 } else { end };
            if to > start {
                spans.push((start as u32, to as u32));
                lines.push((start, i + 1));
            }
            start = end + 1;
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

    /// A line entry's start and line number.
    pub fn line(&self, entry: usize) -> Option<(usize, usize)> {
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
            let found = within
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

impl Entries {
    fn folded(&self) -> (&str, &[(u32, u32)]) {
        let (folded, spans) = self.folded.get_or_init(|| {
            if self.text.is_ascii() {
                return (self.text.to_ascii_lowercase(), None);
            }
            let mut folded = String::new();
            let spans = self
                .spans
                .iter()
                .map(|&(from, to)| {
                    let at = folded.len();
                    folded.push_str(&self.text[from as usize..to as usize].to_lowercase());
                    (at as u32, folded.len() as u32)
                })
                .collect();
            (folded, Some(spans))
        });
        (folded, spans.as_deref().unwrap_or(&self.spans))
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
        assert_eq!((m.len(), m.text(1), m.line(1)), (3, Some("two"), Some((5, 3))));
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
