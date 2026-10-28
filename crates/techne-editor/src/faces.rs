//! Faces composed (EDITOR.md, section 3): the runtime's side of what Lisp
//! defines (`lisp/editor/faces.scm`).
//!
//! Lisp gives a table of faces by name, each already resolved through
//! its inheritance and the palette to a partial style. A name a layer
//! uses that is not in the table stands for its longest dotted prefix
//! that is (`keyword.control` for `keyword`); with none, it is unknown
//! and said so once. Ranges carrying names (`FaceRange`, from Lisp's
//! layers and from presentations) are composed here, in the order they
//! are given, into the flat ranges of a snapshot: at every place the
//! styles over it patch one another attribute by attribute, the later
//! over the earlier. Composed styles are interned, so a snapshot refers
//! to its table by id and ids stay valid as the table grows: a part of
//! an older snapshot, kept when its Lisp stalled, still draws as it did.
//! Interned styles are never reclaimed, which is that validity's price:
//! the table holds every distinct style a session has composed. That is
//! bounded by faces and their overlaps — until styles can carry data
//! (a swatch's own colour), which will want epochs instead.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
};

use crate::present::{Chrome, Highlight, Style, StyleId};

/// A range of text drawn with the face of a name, as Lisp's layers and a
/// presentation's runs give them, before composition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaceRange {
    pub from: usize,
    pub to: usize,
    pub face: String,
}

/// The face table of a generation of Lisp's definitions, composing with
/// it. The interned styles outlive a generation (`set_table`), so their
/// ids never change their meaning.
pub struct Faces {
    /// The generation of Lisp's definitions the table came from
    /// (`face-generation`).
    pub generation: u64,
    /// Faces by name, resolved by Lisp.
    declared: HashMap<String, Style>,
    /// A layer's name, and the declared name it stands for.
    resolved: HashMap<String, Option<String>>,
    /// The interned styles, each once, by id; the first is plain.
    styles: Vec<Style>,
    ids: HashMap<Style, StyleId>,
    /// The interned styles as snapshots share them, rebuilt when one
    /// is interned, not copied into every snapshot.
    shared: Option<Arc<[Style]>>,
    /// The names no prefix of which is declared, seen so far.
    seen_unknown: HashSet<String>,
    /// Those not yet reported.
    unknown: Vec<String>,
}

impl Faces {
    pub fn new(generation: u64, table: Vec<(String, Style)>) -> Faces {
        let mut faces = Faces {
            generation: 0,
            declared: HashMap::new(),
            resolved: HashMap::new(),
            // Id 0 is always the plain style, so falling back to it, for
            // a part of a snapshot built without asking, can do no harm.
            styles: vec![Style::default()],
            ids: HashMap::from([(Style::default(), 0)]),
            shared: None,
            seen_unknown: HashSet::new(),
            unknown: Vec::new(),
        };
        faces.set_table(generation, table);
        faces
    }

    /// An empty table: everything is drawn plainly.
    pub fn empty() -> Faces {
        Faces::new(0, Vec::new())
    }

    /// Take a new generation's table. Interned styles are kept.
    pub fn set_table(&mut self, generation: u64, table: Vec<(String, Style)>) {
        self.generation = generation;
        self.declared = table.into_iter().collect();
        self.resolved.clear();
        // Unknown names stay known to be unknown: one is reported once,
        // not after every face change.
    }

    fn intern(&mut self, style: Style) -> StyleId {
        match self.ids.get(&style) {
            Some(&id) => id,
            None => {
                let id = self.styles.len() as StyleId;
                self.ids.insert(style.clone(), id);
                self.styles.push(style);
                self.shared = None;
                id
            }
        }
    }

    /// The style of the face `name` stands for, or none when it is
    /// unknown: the longest dotted prefix of it that is declared.
    pub fn style_of(&mut self, name: &str) -> Option<Style> {
        let declared = match self.resolved.get(name) {
            Some(d) => d.clone(),
            None => {
                let mut candidate = name;
                let found = loop {
                    if self.declared.contains_key(candidate) {
                        break Some(candidate.to_string());
                    }
                    match candidate.rfind('.') {
                        Some(i) => candidate = &candidate[..i],
                        None => break None,
                    }
                };
                if found.is_none() && self.seen_unknown.insert(name.to_string()) {
                    self.unknown.push(name.to_string());
                }
                self.resolved.insert(name.to_string(), found.clone());
                found
            }
        };
        declared.and_then(|d| self.declared.get(&d).cloned())
    }

    /// The names seen since last asked that no face answers for.
    pub fn take_unknown(&mut self) -> Vec<String> {
        std::mem::take(&mut self.unknown)
    }

    /// The id of one face's style, for text a single face styles (a run).
    pub fn face_id(&mut self, name: &str) -> Option<StyleId> {
        self.style_of(name).map(|s| self.intern(s))
    }

    /// The id of the `default` face's style.
    pub fn default_id(&mut self) -> StyleId {
        self.face_id("default").unwrap_or(0)
    }

    /// The snapshot's chrome: a style id per part, from the face of its
    /// name, falling back to the default style.
    pub fn chrome(&mut self) -> Chrome {
        let default = self.default_id();
        let mut id = |name: &str| self.face_id(name).unwrap_or(default);
        Chrome {
            region: id("region"),
            cursor: id("cursor"),
            cursor_inactive: id("cursor.inactive"),
            mode_line: id("mode-line"),
            mode_line_inactive: id("mode-line.inactive"),
            border: id("border"),
            line_number: id("line-number"),
            line_number_current: id("line-number.current"),
            completion: id("completion"),
            completion_selected: id("completion.selected"),
            selected: id("minibuffer.selected"),
            prompt: id("prompt"),
            key: id("key"),
            shadow: id("shadow"),
            keyword: id("keyword"),
        }
    }

    /// The interned styles, to carry in a snapshot: the same table
    /// every snapshot until another style is interned.
    pub fn table(&mut self) -> Arc<[Style]> {
        self.shared.get_or_insert_with(|| self.styles.clone().into()).clone()
    }

    /// Compose named ranges, lowest first, into flat ranges that do not
    /// overlap: over each stretch, the styles of the ranges covering it
    /// patch one another in order.
    pub fn compose(&mut self, ranges: Vec<FaceRange>) -> Vec<Highlight> {
        // Each range's style, once, keeping the order given; ranges that
        // style nothing (unknown, or a face that sets nothing) cannot
        // change a patch and are left out of the sweep.
        let spans: Vec<(usize, usize, usize, Style)> = ranges
            .into_iter()
            .enumerate()
            .filter_map(|(order, r)| {
                let style = self.style_of(&r.face)?;
                (!style.is_plain() && r.from < r.to).then_some((r.from, r.to, order, style))
            })
            .collect();
        if spans.is_empty() {
            return Vec::new();
        }
        let mut bounds: Vec<usize> = spans.iter().flat_map(|s| [s.0, s.1]).collect();
        bounds.sort_unstable();
        bounds.dedup();
        let mut by_start: Vec<usize> = (0..spans.len()).collect();
        by_start.sort_by_key(|&i| spans[i].0);
        let mut by_end = by_start.clone();
        by_end.sort_by_key(|&i| spans[i].1);
        // The attribute each open range wins, kept as ranges begin and
        // end: a stretch is then read off, not folded from every range
        // over it, however deep the ranges nest.
        let mut winners = Winners::default();
        let (mut starts, mut ends) = (by_start.iter().peekable(), by_end.iter().peekable());
        let mut out: Vec<Highlight> = Vec::new();
        for stretch in bounds.windows(2) {
            let (from, to) = (stretch[0], stretch[1]);
            while ends.peek().is_some_and(|&&i| spans[i].1 <= from) {
                winners.remove(&spans[*ends.next().expect("peeked")]);
            }
            while starts.peek().is_some_and(|&&i| spans[i].0 <= from) {
                winners.add(&spans[*starts.next().expect("peeked")]);
            }
            let style = winners.style();
            if style.is_plain() {
                continue;
            }
            let id = self.intern(style);
            match out.last_mut() {
                Some(last) if last.style == id && last.to == from => last.to = to,
                _ => out.push(Highlight { from, to, style: id }),
            }
        }
        out
    }
}

/// What the open ranges of a composition ask for, attribute by
/// attribute: for each, the values asked for, by the order the ranges
/// were given in. The topmost, if any, is a stretch's.
#[derive(Default)]
struct Winners {
    fg: BTreeMap<usize, crate::present::Color>,
    bg: BTreeMap<usize, crate::present::Color>,
    weight: BTreeMap<usize, crate::present::Weight>,
    slant: BTreeMap<usize, crate::present::Slant>,
    underline: BTreeMap<usize, (crate::present::Underline, Option<crate::present::Color>)>,
    strike: BTreeMap<usize, bool>,
    extend: BTreeMap<usize, bool>,
    family: BTreeMap<usize, Arc<str>>,
    height: BTreeMap<usize, u16>,
}

impl Winners {
    fn add(&mut self, (_, _, order, s): &(usize, usize, usize, Style)) {
        let order = *order;
        if let Some(v) = s.fg {
            self.fg.insert(order, v);
        }
        if let Some(v) = s.bg {
            self.bg.insert(order, v);
        }
        if let Some(v) = s.weight {
            self.weight.insert(order, v);
        }
        if let Some(v) = s.slant {
            self.slant.insert(order, v);
        }
        if let Some(v) = s.underline {
            self.underline.insert(order, v);
        }
        if let Some(v) = s.strike {
            self.strike.insert(order, v);
        }
        if let Some(v) = s.extend {
            self.extend.insert(order, v);
        }
        if let Some(v) = &s.family {
            self.family.insert(order, v.clone());
        }
        if let Some(v) = s.height {
            self.height.insert(order, v);
        }
    }

    fn remove(&mut self, (_, _, order, s): &(usize, usize, usize, Style)) {
        if s.fg.is_some() {
            self.fg.remove(order);
        }
        if s.bg.is_some() {
            self.bg.remove(order);
        }
        if s.weight.is_some() {
            self.weight.remove(order);
        }
        if s.slant.is_some() {
            self.slant.remove(order);
        }
        if s.underline.is_some() {
            self.underline.remove(order);
        }
        if s.strike.is_some() {
            self.strike.remove(order);
        }
        if s.extend.is_some() {
            self.extend.remove(order);
        }
        if s.family.is_some() {
            self.family.remove(order);
        }
        if s.height.is_some() {
            self.height.remove(order);
        }
    }

    fn style(&self) -> Style {
        fn top<T: Clone>(m: &BTreeMap<usize, T>) -> Option<T> {
            m.values().next_back().cloned()
        }
        Style {
            fg: top(&self.fg),
            bg: top(&self.bg),
            weight: top(&self.weight),
            slant: top(&self.slant),
            underline: top(&self.underline),
            strike: top(&self.strike),
            extend: top(&self.extend),
            family: top(&self.family),
            height: top(&self.height),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::present::{Color, Weight};

    fn faces() -> Faces {
        let fg = |c: Color| Style { fg: Some(c), ..Style::default() };
        Faces::new(
            1,
            vec![
                ("default".into(), fg(Color::new(9, 9, 9))),
                ("comment".into(), fg(Color::new(1, 0, 0))),
                ("search".into(), Style { bg: Some(Color::new(0, 2, 0)), weight: Some(Weight::Bold), ..Style::default() }),
            ],
        )
    }

    #[test]
    fn overlaps_patch_attribute_by_attribute() {
        let mut f = faces();
        let range = |from, to, face: &str| FaceRange { from, to, face: face.into() };
        // A comment, with a search match over its middle: the match
        // keeps the comment's foreground and adds its own background.
        let out = f.compose(vec![range(0, 10, "comment"), range(3, 6, "search")]);
        assert_eq!(out.iter().map(|h| (h.from, h.to)).collect::<Vec<_>>(), [(0, 3), (3, 6), (6, 10)]);
        let table = f.table();
        let at = |i: usize| &table[out[i].style as usize];
        assert_eq!(at(0).fg, Some(Color::new(1, 0, 0)));
        assert_eq!(at(0).bg, None);
        assert_eq!(at(1).fg, Some(Color::new(1, 0, 0)));
        assert_eq!(at(1).bg, Some(Color::new(0, 2, 0)));
        // The stretches outside the match share one interned style.
        assert_eq!(out[0].style, out[2].style);
    }

    #[test]
    fn nested_ranges_compose_without_folding_the_whole_stack() {
        // Ranges nested four thousand deep, each face its own colour:
        // the innermost stretch takes the last one's, and composing
        // them is not the day's work (it was, folding every open range
        // at every boundary: seconds in a debug build).
        let n = 4000;
        let table: Vec<(String, Style)> =
            (0..4).map(|i| (format!("f{i}"), Style { fg: Some(Color::new(i as u8, 0, 0)), ..Style::default() })).collect();
        let mut f = Faces::new(1, table);
        let ranges: Vec<FaceRange> = (0..n).map(|i| FaceRange { from: i, to: 2 * n - i, face: format!("f{}", i % 4) }).collect();
        let out = f.compose(ranges);
        assert_eq!(out.len(), 2 * n - 1);
        // The stretch all the ranges cover takes the last one's colour.
        let middle = &out[n - 1];
        assert_eq!((middle.from, middle.to), (n - 1, n + 1));
        assert_eq!(f.table()[middle.style as usize].fg, Some(Color::new(((n - 1) % 4) as u8, 0, 0)));
    }

    #[test]
    fn names_fall_back_to_dotted_prefixes() {
        let mut f = faces();
        assert_eq!(f.style_of("comment.doc").and_then(|s| s.fg), Some(Color::new(1, 0, 0)));
        assert_eq!(f.style_of("nothing.here"), None);
        assert_eq!(f.take_unknown(), ["nothing.here"]);
        // Once per name and table.
        assert_eq!(f.style_of("nothing.here"), None);
        assert!(f.take_unknown().is_empty());
    }
}
