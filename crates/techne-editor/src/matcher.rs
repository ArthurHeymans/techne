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

/// A part of a pattern to find. On x86_64 with AVX2, memchr's search for
/// a pair of the needle's bytes is called directly: memmem's dispatch to it
/// costs some 30 instructions a call, many when a part is searched for from
/// each of 50,000 entries. Elsewhere, and on short haystacks, memmem.
struct Needle {
    memmem: Finder<'static>,
    #[cfg(target_arch = "x86_64")]
    avx2: Option<memchr::arch::x86_64::avx2::packedpair::Finder>,
}

impl Needle {
    fn new(needle: &[u8]) -> Needle {
        Needle {
            memmem: Finder::new(needle).into_owned(),
            #[cfg(target_arch = "x86_64")]
            avx2: memchr::arch::x86_64::avx2::packedpair::Finder::new(needle),
        }
    }

    fn needle(&self) -> &[u8] {
        self.memmem.needle()
    }

    fn find(&self, haystack: &[u8]) -> Option<usize> {
        #[cfg(target_arch = "x86_64")]
        if let Some(avx2) = &self.avx2
            && haystack.len() >= avx2.min_haystack_len()
        {
            return avx2.find(haystack, self.needle());
        }
        self.memmem.find(haystack)
    }
}

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
        // ASCII text is folded now, with the entries, at once, rather than
        // on the first key filtering, which has the keystroke budget to
        // itself. Other text is folded entry by entry, several times
        // slower, and not searched as a whole: when a key needs it.
        if entries.text.is_ascii() {
            let _ = entries.folded.set((entries.text.to_ascii_lowercase(), None));
        }
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
            let mut parts: Vec<(bool, Needle)> = pattern
                .split(' ')
                .filter(|p| !p.is_empty())
                .map(|p| match p.chars().any(char::is_uppercase) {
                    true => (false, Needle::new(p.as_bytes())),
                    false => (true, Needle::new(p.to_lowercase().as_bytes())),
                })
                .collect();
            // The longest part first, as likely the rarest; each part once,
            // a line yanked into the input repeating its words.
            parts.sort_by(|(fa, a), (fb, b)| b.needle().len().cmp(&a.needle().len()).then((fa, a.needle()).cmp(&(fb, b.needle()))));
            parts.dedup_by(|(fa, a), (fb, b)| fa == fb && a.needle() == b.needle());
            let e = &self.entries;
            // Each part narrows the entries the parts before left. While
            // they are more than one in 16, it is found in all the text at
            // once, from one of them to the next past each occurrence,
            // rather than in each on its own, some 100 instructions more an
            // entry: several common words (a line yanked into the input)
            // took 40 M in the lines of a 100k-line file. Among fewer, each
            // is searched, not the text between them.
            let found = parts.iter().fold(None, |entries: Option<Vec<u32>>, (fold, f)| {
                let entries = entries.as_deref().unwrap_or(within);
                let in_place = if *fold { e.folded_in_place() } else { Some((e.text.as_str(), e.spans.as_slice())) };
                match in_place {
                    Some((text, spans)) if entries.len() * 16 >= spans.len() => Some(entries_with(text, spans, f, entries)),
                    _ => {
                        let (text, spans) = if *fold { e.folded() } else { (e.text.as_str(), e.spans.as_slice()) };
                        Some(
                            entries
                                .iter()
                                .copied()
                                .filter(|&i| {
                                    let (from, to) = spans[i as usize];
                                    f.find(&text.as_bytes()[from as usize..to as usize]).is_some()
                                })
                                .collect(),
                        )
                    }
                }
            });
            let found = found.unwrap_or_else(|| within.clone());
            self.stack.push((pattern.to_string(), found));
        }
        self.matches().len()
    }

    /// The entries matching the last pattern, in order.
    pub fn matches(&self) -> &[u32] {
        &self.stack.last().expect("the empty pattern").1
    }
}

/// Those of `entries`, in order, at `spans` of `text` in order, that
/// `finder`'s needle occurs in. The text is searched from an entry only
/// when the occurrence found last is before it: no byte is searched twice.
fn entries_with(text: &str, spans: &[(u32, u32)], finder: &Needle, entries: &[u32]) -> Vec<u32> {
    let (text, n) = (text.as_bytes(), finder.needle().len());
    let mut next: Option<usize> = None;
    let mut found = Vec::new();
    for &entry in entries {
        let (from, to) = (spans[entry as usize].0 as usize, spans[entry as usize].1 as usize);
        // The first occurrence at or after `from`.
        if next.is_none_or(|i| i < from) {
            next = finder.find(&text[from..]).map(|i| from + i);
        }
        match next {
            None => break,
            Some(i) if i + n <= to => found.push(entry),
            Some(_) => {}
        }
    }
    found
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

    /// Typed a key at a time, patterns of common words, repeated and in
    /// either case, match as each entry searched on its own would: through
    /// the text at once among many entries, each among few, and each where
    /// a non-ASCII entry folds out of place.
    #[test]
    fn several_common_parts_match_as_each_entry_would() {
        let words = ["alpha", "Beta", "gamma", "let", "fn", "mut", "é"];
        let mut seed = 1u64;
        let mut below = |n: u64| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) % n
        };
        let mut entries: Vec<String> =
            (0..2000).map(|_| (0..below(8)).map(|_| words[below(6) as usize]).collect::<Vec<_>>().join(" ")).collect();
        let patterns = ["gamma let alpha gamma", "beta Beta mut fn", "let fn mut alpha beta gamma", "Beta é"];
        for ascii in [true, false] {
            if !ascii {
                entries[1000].push_str(" é");
            }
            let mut m = Matcher::new(entries.clone());
            for pattern in patterns {
                for typed in pattern.char_indices().map(|(i, c)| &pattern[..i + c.len_utf8()]) {
                    let expected: Vec<u32> = (0..entries.len() as u32)
                        .filter(|&i| {
                            typed.split(' ').filter(|p| !p.is_empty()).all(|p| match p.chars().any(char::is_uppercase) {
                                true => entries[i as usize].contains(p),
                                false => entries[i as usize].to_lowercase().contains(p),
                            })
                        })
                        .collect();
                    assert_eq!(matching(&mut m, typed), expected, "{typed:?}");
                }
            }
        }
    }
}
