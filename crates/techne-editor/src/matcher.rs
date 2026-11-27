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
use regex_cursor::{Cursor, Input, engines::meta::Regex};
use techne_vm::data::Data;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PartKind {
    Literal,
    Flex,
    LiteralOrRegexp,
    Regexp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilterPart {
    pub kind: PartKind,
    pub text: String,
    pub fold: bool,
}

impl FilterPart {
    pub fn literal(text: impl Into<String>, fold: bool) -> Self {
        Self { kind: PartKind::Literal, text: text.into(), fold }
    }

    pub fn from_legacy(raw: &str) -> Self {
        let fold = !raw.chars().any(char::is_uppercase);
        Self { kind: PartKind::Literal, text: raw.to_string(), fold }
    }

    pub fn normalized(&self) -> Self {
        match self.kind {
            PartKind::Literal | PartKind::Flex if self.fold => Self { kind: self.kind, text: self.text.to_lowercase(), fold: true },
            _ => self.clone(),
        }
    }
}

pub(crate) fn parse_filter_parts(items: &[Data], allow_strings: bool) -> Result<Vec<FilterPart>, String> {
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        match item {
            Data::String(s) if allow_strings => {
                if !s.is_empty() {
                    out.push(FilterPart::from_legacy(s));
                }
            }
            Data::List(elems) if elems.len() == 3 => {
                let (Data::String(kind), Data::String(text), Data::Bool(fold)) = (&elems[0], &elems[1], &elems[2]) else {
                    return Err("filter part must be (kind text fold?)".into());
                };
                let kind = match kind.as_str() {
                    "literal" => PartKind::Literal,
                    "flex" => PartKind::Flex,
                    "literal-or-regexp" => PartKind::LiteralOrRegexp,
                    "regexp" => PartKind::Regexp,
                    other => return Err(format!("unknown filter part kind: {other}")),
                };
                if !text.is_empty() {
                    out.push(FilterPart { kind, text: text.clone(), fold: *fold });
                }
            }
            _ => return Err("filter part must be (kind text fold?)".into()),
        }
    }
    Ok(out)
}

struct SliceCursor<'a>(&'a [u8]);

impl Cursor for SliceCursor<'_> {
    fn chunk(&self) -> &[u8] {
        self.0
    }
    fn utf8_aware(&self) -> bool {
        true
    }
    fn advance(&mut self) -> bool {
        false
    }
    fn backtrack(&mut self) -> bool {
        false
    }
    fn total_bytes(&self) -> Option<usize> {
        Some(self.0.len())
    }
    fn offset(&self) -> usize {
        0
    }
}

pub(crate) fn compile_regexp(pattern: &str, fold: bool) -> Option<Regex> {
    if fold { Regex::new(&format!("(?i){pattern}")).ok() } else { Regex::new(pattern).ok() }
}

pub(crate) fn regexp_is_match(re: &Regex, haystack: &[u8]) -> bool {
    re.is_match(Input::new(SliceCursor(haystack)))
}

pub(crate) fn flex_match(haystack: &str, needle: &str) -> bool {
    let mut chars = needle.chars();
    let Some(mut want) = chars.next() else { return true };
    for c in haystack.chars() {
        if c == want {
            match chars.next() {
                Some(next) => want = next,
                None => return true,
            }
        }
    }
    false
}

/// Whether `next` is guaranteed to match a subset of what `prev` matched.
/// Regexp parts only refine when unchanged, since editing a regexp (e.g.
/// adding `|alt` or `?`) can broaden its matches. Switching from folded to
/// case-sensitive is only provably monotonic for ASCII (Unicode context-
/// sensitive folding like Greek final sigma `ΟΣ` vs `ΟΣΑ` is not).
fn refines(prev: &[FilterPart], next: &[FilterPart]) -> bool {
    prev.iter().all(|p| {
        next.iter().any(|q| match (p.kind, q.kind) {
            (PartKind::Literal, PartKind::Literal) => {
                (p.fold == q.fold && q.text.contains(&p.text))
                    || (p.fold && !q.fold && p.text.is_ascii() && q.text.is_ascii() && q.text.to_ascii_lowercase().contains(&p.text))
            }
            (PartKind::Flex, PartKind::Flex) => {
                (p.fold == q.fold && q.text.starts_with(&p.text))
                    || (p.fold && !q.fold && p.text.is_ascii() && q.text.is_ascii() && q.text.to_ascii_lowercase().starts_with(&p.text))
            }
            (PartKind::LiteralOrRegexp, PartKind::LiteralOrRegexp) | (PartKind::Regexp, PartKind::Regexp) => p == q,
            _ => false,
        })
    })
}

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
    /// The same, folded to lower case: at the same spans when the text is
    /// ASCII (folding keeps its length), else each entry beyond ASCII
    /// folded on its own, as Lisp's `string-downcase` folds it, after the
    /// others.
    folded: OnceCell<Folded>,
    /// For documents' lines: each entry's start and line number in its
    /// document, and which document it is.
    lines: Vec<(usize, usize, usize)>,
}

pub struct Matcher {
    entries: Rc<Entries>,
    /// Part lists, each refining the one before, with their matches; the
    /// first is the empty list, which every entry matches.
    stack: Vec<(Vec<FilterPart>, Vec<u32>)>,
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
        // Sized up front: grown as it fills, a large buffer's text and
        // spans would be copied over and over. Empty lines are no entries:
        // the excess goes once they are known to be many.
        let ropes: Vec<_> = ropes.into_iter().collect();
        let n = ropes.iter().map(|r| r.len_lines()).sum();
        let mut text = String::with_capacity(ropes.iter().map(|r| r.len_bytes()).sum());
        let (mut spans, mut lines) = (Vec::with_capacity(n), Vec::with_capacity(n));
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
        if spans.len() < spans.capacity() / 2 {
            spans.shrink_to_fit();
            lines.shrink_to_fit();
        }
        Matcher::of(Entries { text, spans, folded: OnceCell::new(), lines })
    }

    fn of(entries: Entries) -> Matcher {
        // The text is folded now, with the entries, rather than on the
        // first key filtering, which has the keystroke budget to itself:
        // 5.6 M of the 41 M that completing the input took over the lines
        // of a 100k-line file with one character beyond ASCII.
        let _ = entries.folded();
        Matcher::sharing(Rc::new(entries))
    }

    fn sharing(entries: Rc<Entries>) -> Matcher {
        let all = (0..entries.spans.len() as u32).collect();
        Matcher { entries, stack: vec![(Vec::new(), all)] }
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

    /// Match `pattern` (legacy space-separated smart-case literals); return
    /// how many entries match.
    pub fn filter(&mut self, pattern: &str) -> usize {
        let parts: Vec<FilterPart> = pattern.split(' ').filter(|p| !p.is_empty()).map(FilterPart::from_legacy).collect();
        self.filter_parts(&parts)
    }

    /// Match parsed Orderless `parts`; return how many entries match.
    pub fn filter_parts(&mut self, parts: &[FilterPart]) -> usize {
        let mut normalized: Vec<FilterPart> = parts.iter().filter(|p| !p.text.is_empty()).map(FilterPart::normalized).collect();
        // Run literal parts first (longest first, as likely the rarest), then
        // flex and regexp parts on the already-narrowed entries; deduplicate.
        normalized
            .sort_by(|a, b| a.kind.cmp(&b.kind).then(b.text.len().cmp(&a.text.len())).then(a.fold.cmp(&b.fold)).then(a.text.cmp(&b.text)));
        normalized.dedup();

        while !refines(&self.stack.last().expect("the empty pattern").0, &normalized) {
            self.stack.pop();
        }
        let (last, within) = self.stack.last().expect("the empty pattern");
        if *last != normalized {
            let e = &self.entries;
            let mut found: Option<Vec<u32>> = None;
            for part in &normalized {
                if found.as_ref().is_some_and(Vec::is_empty) {
                    break;
                }
                let entries = found.as_deref().unwrap_or(within);
                match part.kind {
                    PartKind::Literal => {
                        let f = &Needle::new(part.text.as_bytes());
                        let (text, spans) = if part.fold { e.folded() } else { (e.text.as_str(), e.spans.as_slice()) };
                        found = match entries.len() * 16 >= spans.len() {
                            // Entries folding lengthened are after the text.
                            true => Some(entries_with(text, spans, f, entries, e.text.len())),
                            false => Some(
                                entries
                                    .iter()
                                    .copied()
                                    .filter(|&i| {
                                        let (from, to) = spans[i as usize];
                                        f.find(&text.as_bytes()[from as usize..to as usize]).is_some()
                                    })
                                    .collect(),
                            ),
                        };
                    }
                    PartKind::Flex => {
                        let (text, spans) = if part.fold { e.folded() } else { (e.text.as_str(), e.spans.as_slice()) };
                        found = Some(
                            entries
                                .iter()
                                .copied()
                                .filter(|&i| {
                                    let (from, to) = spans[i as usize];
                                    flex_match(&text[from as usize..to as usize], &part.text)
                                })
                                .collect(),
                        );
                    }
                    PartKind::LiteralOrRegexp => {
                        let literal = if part.fold { part.text.to_lowercase() } else { part.text.clone() };
                        let f = &Needle::new(literal.as_bytes());
                        let re = compile_regexp(&part.text, part.fold);
                        let (ltext, lspans) = if part.fold { e.folded() } else { (e.text.as_str(), e.spans.as_slice()) };
                        let (rtext, rspans) = (e.text.as_str(), e.spans.as_slice());
                        found = Some(
                            entries
                                .iter()
                                .copied()
                                .filter(|&i| {
                                    let (lf, lt) = lspans[i as usize];
                                    if f.find(&ltext.as_bytes()[lf as usize..lt as usize]).is_some() {
                                        return true;
                                    }
                                    if let Some(re) = &re {
                                        let (rf, rt) = rspans[i as usize];
                                        return regexp_is_match(re, &rtext.as_bytes()[rf as usize..rt as usize]);
                                    }
                                    false
                                })
                                .collect(),
                        );
                    }
                    PartKind::Regexp => {
                        if let Some(re) = compile_regexp(&part.text, part.fold) {
                            let (text, spans) = (e.text.as_str(), e.spans.as_slice());
                            found = Some(
                                entries
                                    .iter()
                                    .copied()
                                    .filter(|&i| {
                                        let (from, to) = spans[i as usize];
                                        regexp_is_match(&re, &text.as_bytes()[from as usize..to as usize])
                                    })
                                    .collect(),
                            );
                        } else {
                            let literal = if part.fold { part.text.to_lowercase() } else { part.text.clone() };
                            let f = &Needle::new(literal.as_bytes());
                            let (text, spans) = if part.fold { e.folded() } else { (e.text.as_str(), e.spans.as_slice()) };
                            found = Some(
                                entries
                                    .iter()
                                    .copied()
                                    .filter(|&i| {
                                        let (from, to) = spans[i as usize];
                                        f.find(&text.as_bytes()[from as usize..to as usize]).is_some()
                                    })
                                    .collect(),
                            );
                        }
                    }
                }
            }
            let found = found.unwrap_or_else(|| within.clone());
            self.stack.push((normalized, found));
        }
        self.matches().len()
    }

    /// The entries matching the last pattern, in order.
    pub fn matches(&self) -> &[u32] {
        &self.stack.last().expect("the empty pattern").1
    }
}

/// Those of `entries`, in order, at `spans` of `text`, that `finder`'s
/// needle occurs in. The spans before `split` are in order, and so are
/// those after it (a folded text's entries that folding lengthened). In
/// each part the text is searched, up to the part's end, from an entry
/// only when the occurrence found last is before it: no byte is searched
/// twice, and none once no part has an occurrence left.
fn entries_with(text: &str, spans: &[(u32, u32)], finder: &Needle, entries: &[u32], split: usize) -> Vec<u32> {
    if split >= text.len() {
        return entries_in_place(text.as_bytes(), spans, finder, entries);
    }
    const NONE: usize = usize::MAX;
    let (text, n) = (text.as_bytes(), finder.needle().len());
    let ends = [split.min(text.len()), text.len()];
    // In each part, the first occurrence at or after the entry searched
    // from last, NONE for none.
    let mut next: [Option<usize>; 2] = [None, if split < text.len() { None } else { Some(NONE) }];
    // Taken when many entries are left, and most usually match: growing
    // by doubling would copy the list over and over (an allocator cannot
    // always grow it in place). The excess goes once the count is known.
    let mut found = Vec::with_capacity(entries.len());
    for &entry in entries {
        let (from, to) = (spans[entry as usize].0 as usize, spans[entry as usize].1 as usize);
        let part = usize::from(from >= split);
        // The first occurrence at or after `from`.
        if next[part].is_none_or(|i| i < from) {
            next[part] = Some(finder.find(&text[from..ends[part]]).map_or(NONE, |i| from + i));
        }
        match next[part] {
            Some(i) if i != NONE && i + n <= to => found.push(entry),
            _ if next == [Some(NONE); 2] => break,
            _ => {}
        }
    }
    found.shrink_to_fit();
    found
}

/// `entries_with` where every span is in order, as in a text folded in
/// place: one cursor, the loop run for each of 100k lines with a few
/// instructions to spare (the two parts' bookkeeping took 20 more an
/// entry, 4 M of the keys workload search-yank-line).
fn entries_in_place(text: &[u8], spans: &[(u32, u32)], finder: &Needle, entries: &[u32]) -> Vec<u32> {
    let n = finder.needle().len();
    let mut next: Option<usize> = None;
    let mut found = Vec::with_capacity(entries.len());
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
    found.shrink_to_fit();
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
            // folding may lengthen, are folded after it, in order. They are
            // found from the bytes beyond ASCII, skipped a block at a time,
            // not by asking each entry: some 75 instructions an entry, 7.5 M
            // for a 100k-line file with one character beyond ASCII.
            let mut folded = self.text.to_ascii_lowercase();
            let mut spans = self.spans.clone();
            let bytes = self.text.as_bytes();
            let mut at = 0;
            while let Some(p) = beyond_ascii(bytes, at) {
                // The entry holding it, if a line break is not where it is.
                let k = self.spans.partition_point(|&(_, to)| to as usize <= p);
                let Some(&(from, to)) = self.spans.get(k).filter(|&&(from, _)| from as usize <= p) else {
                    at = p + 1;
                    continue;
                };
                let start = folded.len();
                folded.push_str(&techne_text::motion::lowered(&self.text[from as usize..to as usize]).0);
                spans[k] = (start as u32, folded.len() as u32);
                at = to as usize;
            }
            (folded, Some(spans))
        });
        (folded, spans.as_deref().unwrap_or(&self.spans))
    }
}

/// The first byte of `bytes` from `at` that is not ASCII.
fn beyond_ascii(bytes: &[u8], at: usize) -> Option<usize> {
    let rest = bytes.get(at..)?;
    let skipped = rest.chunks(64).take_while(|c| c.is_ascii()).count() * 64;
    rest[skipped.min(rest.len())..].iter().position(|b| !b.is_ascii()).map(|i| at + skipped + i)
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

    /// Folding that changes an entry's length, empty entries at the end,
    /// and a needle across two entries, which matches neither.
    #[test]
    fn entries_fold_apart_and_end_empty() {
        let texts = ["ab", "İx", "cd", "x", "İX", "", ""].map(String::from).to_vec();
        let m = Matcher::new(texts.clone());
        assert_eq!(matching(&mut m.copy(), "bc"), Vec::<u32>::new(), "across entries");
        assert_eq!(matching(&mut m.copy(), "dx"), Vec::<u32>::new(), "across entries, one folded");
        assert_eq!(matching(&mut m.copy(), "x"), [1, 3, 4]);
        assert_eq!(matching(&mut m.copy(), "i\u{307}x"), [1, 4], "İ folds to two characters");
        assert_eq!(matching(&mut m.copy(), "X"), [4]);
        // Many entries, so that the text is searched at once, the folded
        // ones among them, ending empty.
        let many: Vec<String> = (0..64).flat_map(|_| texts.iter().cloned()).collect();
        let x: Vec<u32> = (0..many.len() as u32).filter(|&i| many[i as usize].to_lowercase().contains('x')).collect();
        assert_eq!(matching(&mut Matcher::new(many.clone()), "x"), x);
        assert_eq!(matching(&mut Matcher::new(many), "zz"), Vec::<u32>::new());
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
    /// non-ASCII entries fold out of place, among the others, and whole
    /// patterns at once (as completing the input gives them).
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
                for i in (3..entries.len()).step_by(97) {
                    entries[i].push_str(if i % 2 == 0 { " é" } else { " Gamma É" });
                }
            }
            let expected = |typed: &str| -> Vec<u32> {
                (0..entries.len() as u32)
                    .filter(|&i| {
                        typed.split(' ').filter(|p| !p.is_empty()).all(|p| match p.chars().any(char::is_uppercase) {
                            true => entries[i as usize].contains(p),
                            false => entries[i as usize].to_lowercase().contains(p),
                        })
                    })
                    .collect()
            };
            let mut m = Matcher::new(entries.clone());
            for pattern in patterns {
                for typed in pattern.char_indices().map(|(i, c)| &pattern[..i + c.len_utf8()]) {
                    assert_eq!(matching(&mut m, typed), expected(typed), "{typed:?}");
                }
                assert_eq!(matching(&mut m.copy(), pattern), expected(pattern), "{pattern:?} at once");
            }
            for pattern in ["gamma é", "é", "gamma", "é gamma alpha"] {
                assert_eq!(matching(&mut Matcher::new(entries.clone()), pattern), expected(pattern), "{pattern:?} first");
            }
        }
    }

    #[test]
    fn orderless_parts_and_regexp_broadening() {
        let mut m = Matcher::new(
            ["alpha-one", "beta-two", "describe-named-command", "hello world", "foo.bar", "fooxbar"].map(String::from).to_vec(),
        );
        // Narrow to literal "alpha", then broaden with regexp "alpha|beta".
        m.filter_parts(&[FilterPart::literal("alpha", true)]);
        assert_eq!(m.matches(), &[0]);
        m.filter_parts(&[FilterPart { kind: PartKind::Regexp, text: "alpha|beta".into(), fold: true }]);
        assert_eq!(m.matches(), &[0, 1]);

        // Flex subsequence matching.
        m.filter_parts(&[FilterPart { kind: PartKind::Flex, text: "dscrb".into(), fold: true }]);
        assert_eq!(m.matches(), &[2]);

        // Literal with embedded space (escaped space in Orderless).
        m.filter_parts(&[FilterPart::literal("hello world", true)]);
        assert_eq!(m.matches(), &[3]);

        // Literal dot vs regexp dot.
        m.filter_parts(&[FilterPart::literal("foo.bar", true)]);
        assert_eq!(m.matches(), &[4]);
        m.filter_parts(&[FilterPart { kind: PartKind::Regexp, text: "foo.bar".into(), fold: true }]);
        assert_eq!(m.matches(), &[4, 5]);

        // LiteralOrRegexp matches both literal punctuation and regexp.
        let mut m2 = Matcher::new(["foo+bar", "foooobar", "baz"].map(String::from).to_vec());
        m2.filter_parts(&[FilterPart { kind: PartKind::LiteralOrRegexp, text: "foo+bar".into(), fold: true }]);
        assert_eq!(m2.matches(), &[0, 1]);

        // Unicode context-sensitive case folding (Greek final sigma):
        // filtering with folded `ος` then case-sensitive `ΟΣ` must not
        // falsely reuse the empty cached result.
        let mut greek = Matcher::new(vec!["ΟΣΑ".into()]);
        greek.filter_parts(&[FilterPart::literal("ος", true)]);
        assert_eq!(greek.matches(), &[] as &[u32]);
        greek.filter_parts(&[FilterPart::literal("ΟΣ", false)]);
        assert_eq!(greek.matches(), &[0]);

        let mut greek_flex = Matcher::new(vec!["ΟΣΑ".into()]);
        greek_flex.filter_parts(&[FilterPart { kind: PartKind::Flex, text: "ος".into(), fold: true }]);
        assert_eq!(greek_flex.matches(), &[] as &[u32]);
        greek_flex.filter_parts(&[FilterPart { kind: PartKind::Flex, text: "ΟΣ".into(), fold: false }]);
        assert_eq!(greek_flex.matches(), &[0]);
    }
}
