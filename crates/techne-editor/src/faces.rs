//! Faces composed (EDITOR.md, section 3): the runtime's side of what Lisp
//! defines (`lisp/editor/faces.scm`).
//!
//! Lisp gives a table of faces by name, each already resolved through
//! its inheritance and the palette to a partial style. Names are
//! interned once (`FaceName`), where they cross from Lisp or from a
//! syntax query, so composing compares and looks up numbers. A name that
//! is not in the table stands for its longest dotted prefix that is
//! (`keyword.control` for `keyword`); with none, it is unknown and said
//! so once. This is the rule `face-attributes` applies in Lisp to a name
//! nothing declared or set: the table holds every name declared or set,
//! so the two agree by construction.
//!
//! Ranges carrying names (`FaceRange`) are composed in the order they
//! are given into the flat ranges of a snapshot: at every place the
//! styles over it patch one another attribute by attribute, the later
//! over the earlier. Composed styles are interned, so a snapshot refers
//! to its table by id and ids stay valid as the table grows: a part of
//! an older snapshot, kept when its Lisp stalled, still draws as it did.
//! Names are never reclaimed: they are bounded by the names layers use.
//! Styles are, when a new face table comes and the interned ones have
//! grown past `MAX_STYLES` — restyling over and over (a theme being
//! tried, a colour animated) would otherwise grow them, and the table
//! every snapshot carries, without bound. A part kept from an older
//! snapshot across that reset may draw in the wrong style once.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
};

use crate::present::{Chrome, Color, Highlight, Slant, Style, StyleId, Underline, Weight};

/// A face's name, interned by `Faces::name`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FaceName(u32);

/// A range of text drawn with a face, before composition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FaceRange {
    pub from: usize,
    pub to: usize,
    pub face: FaceName,
}

/// The face table of a generation of Lisp's definitions, composing with
/// it.
pub struct Faces {
    /// The generation of Lisp's definitions the table came from
    /// (`face-generation`).
    pub generation: u64,
    /// The interned names, by `FaceName`.
    names: Vec<Box<str>>,
    by_name: HashMap<Box<str>, FaceName>,
    /// The table's styles, and the name each is declared under.
    declared: Vec<Style>,
    declared_by_name: HashMap<Box<str>, usize>,
    /// Per `FaceName`, once looked up in this table: the declared style
    /// it stands for, or `None` for an unknown name.
    resolved: Vec<Option<Option<usize>>>,
    /// The interned styles, each once, by id; the first is plain.
    styles: Styles,
    /// The names no prefix of which is declared, not yet reported.
    seen_unknown: HashSet<FaceName>,
    unknown: Vec<String>,
    /// The chrome's styles, made once per face table (`chrome`).
    chrome: Option<Chrome>,
}

/// How many interned styles a new face table lets stay.
const MAX_STYLES: usize = 4096;

/// The interned styles.
struct Styles {
    list: Vec<Style>,
    ids: HashMap<Style, StyleId>,
    /// The list as snapshots share it, rebuilt when a style is interned.
    shared: Option<Arc<[Style]>>,
}

impl Styles {
    fn intern(&mut self, style: &Style) -> StyleId {
        if let Some(&id) = self.ids.get(style) {
            return id;
        }
        let id = self.list.len() as StyleId;
        self.ids.insert(style.clone(), id);
        self.list.push(style.clone());
        self.shared = None;
        id
    }
}

impl Faces {
    pub fn new(generation: u64, table: Vec<(String, Style)>) -> Faces {
        let mut faces = Faces {
            generation: 0,
            names: Vec::new(),
            by_name: HashMap::new(),
            declared: Vec::new(),
            declared_by_name: HashMap::new(),
            resolved: Vec::new(),
            // Id 0 is always the plain style, so falling back to it, for
            // a part of a snapshot built without asking, can do no harm.
            styles: Styles { list: vec![Style::default()], ids: HashMap::from([(Style::default(), 0)]), shared: None },
            seen_unknown: HashSet::new(),
            unknown: Vec::new(),
            chrome: None,
        };
        faces.set_table(generation, table);
        faces
    }

    /// An empty table: everything is drawn plainly.
    pub fn empty() -> Faces {
        Faces::new(0, Vec::new())
    }

    /// Take a new generation's table. Interned styles and names are kept.
    pub fn set_table(&mut self, generation: u64, table: Vec<(String, Style)>) {
        self.generation = generation;
        self.declared_by_name.clear();
        self.declared.clear();
        for (name, style) in table {
            self.declared_by_name.insert(name.into(), self.declared.len());
            self.declared.push(style);
        }
        self.resolved.clear();
        self.chrome = None;
        if self.styles.list.len() > MAX_STYLES {
            self.styles = Styles { list: vec![Style::default()], ids: HashMap::from([(Style::default(), 0)]), shared: None };
        }
        // Unknown names stay known to be unknown: one is reported once,
        // not after every face change.
    }

    /// The interned name `name`.
    pub fn name(&mut self, name: &str) -> FaceName {
        if let Some(&n) = self.by_name.get(name) {
            return n;
        }
        let n = FaceName(self.names.len() as u32);
        self.names.push(name.into());
        self.by_name.insert(name.into(), n);
        n
    }

    /// The text of an interned name.
    pub fn name_text(&self, name: FaceName) -> &str {
        &self.names[name.0 as usize]
    }

    /// The declared style `name` stands for: its own, or its longest
    /// dotted prefix's; none when it is unknown.
    fn resolve(&mut self, name: FaceName) -> Option<usize> {
        let i = name.0 as usize;
        if let Some(Some(found)) = self.resolved.get(i) {
            return *found;
        }
        let text = &self.names[i];
        let mut candidate: &str = text;
        let found = loop {
            if let Some(&d) = self.declared_by_name.get(candidate) {
                break Some(d);
            }
            match candidate.rfind('.') {
                Some(dot) => candidate = &candidate[..dot],
                None => break None,
            }
        };
        if found.is_none() && self.seen_unknown.insert(name) {
            self.unknown.push(text.to_string());
        }
        if self.resolved.len() <= i {
            self.resolved.resize(i + 1, None);
        }
        self.resolved[i] = Some(found);
        found
    }

    /// The style of the face `name` stands for, or none when it is
    /// unknown.
    pub fn style_of(&mut self, name: FaceName) -> Option<&Style> {
        self.resolve(name).map(|d| &self.declared[d])
    }

    /// The names seen since last asked that no face answers for.
    pub fn take_unknown(&mut self) -> Vec<String> {
        std::mem::take(&mut self.unknown)
    }

    /// The id of one face's style, for text a single face styles (a run).
    pub fn face_id(&mut self, name: FaceName) -> Option<StyleId> {
        let d = self.resolve(name)?;
        Some(self.styles.intern(&self.declared[d]))
    }

    /// The id of the style of the face named `name`.
    pub fn face_id_of(&mut self, name: &str) -> Option<StyleId> {
        let name = self.name(name);
        self.face_id(name)
    }

    /// The id of the `default` face's style.
    pub fn default_id(&mut self) -> StyleId {
        self.face_id_of("default").unwrap_or(0)
    }

    /// The snapshot's chrome: a style id for each part a frontend draws
    /// of its own, from the face of that part's name, falling back to
    /// the default style.
    pub fn chrome(&mut self) -> Chrome {
        // The same until the faces or the styles change: not interned
        // anew for every snapshot.
        if let Some(chrome) = self.chrome {
            return chrome;
        }
        let chrome = self.make_chrome();
        self.chrome = Some(chrome);
        chrome
    }

    fn make_chrome(&mut self) -> Chrome {
        let default = self.default_id();
        let mut id = |name: &str| self.face_id_of(name).unwrap_or(default);
        Chrome {
            region: id("region"),
            cursor: id("cursor"),
            cursor_inactive: id("cursor.inactive"),
            mode_line: id("mode-line"),
            mode_line_inactive: id("mode-line.inactive"),
            border: id("border"),
            line_number: id("line-number"),
            line_number_current: id("line-number.current"),
            eob_marker: id("eob-marker"),
            completion: id("completion"),
            completion_selected: id("completion.selected"),
            selected: id("minibuffer.selected"),
            prompt: id("prompt"),
            which_key_key: id("which-key.key"),
            which_key_separator: id("which-key.separator"),
            which_key_group: id("which-key.group"),
        }
    }

    /// The interned styles, to carry in a snapshot: the same table
    /// every snapshot until another style is interned.
    pub fn table(&mut self) -> Arc<[Style]> {
        let styles = &mut self.styles;
        styles.shared.get_or_insert_with(|| styles.list.clone().into()).clone()
    }

    /// Compose named ranges, lowest first, into flat ranges that do not
    /// overlap: over each stretch, the styles of the ranges covering it
    /// patch one another in order.
    pub fn compose(&mut self, ranges: &[FaceRange]) -> Vec<Highlight> {
        // Each range's declared style, keeping the order given; ranges
        // that style nothing (unknown, or a face that sets nothing)
        // cannot change a patch and are left out of the sweep.
        let mut spans: Vec<Span> = Vec::with_capacity(ranges.len());
        for (order, r) in ranges.iter().enumerate() {
            if r.from >= r.to {
                continue;
            }
            if let Some(d) = self.resolve(r.face)
                && !self.declared[d].is_plain()
            {
                spans.push(Span { from: r.from, to: r.to, order, style: d });
            }
        }
        if spans.is_empty() {
            return Vec::new();
        }
        let mut bounds: Vec<usize> = spans.iter().flat_map(|s| [s.from, s.to]).collect();
        bounds.sort_unstable();
        bounds.dedup();
        let mut by_start: Vec<usize> = (0..spans.len()).collect();
        by_start.sort_by_key(|&i| spans[i].from);
        let mut by_end = by_start.clone();
        by_end.sort_by_key(|&i| spans[i].to);
        // The attribute each open range wins, kept as ranges begin and
        // end: a stretch is then read off, not folded from every range
        // over it, however deep the ranges nest.
        let (declared, styles) = (&self.declared, &mut self.styles);
        let mut winners = Winners::default();
        let (mut starts, mut ends) = (by_start.iter().peekable(), by_end.iter().peekable());
        let mut out: Vec<Highlight> = Vec::new();
        for stretch in bounds.windows(2) {
            let (from, to) = (stretch[0], stretch[1]);
            while let Some(&&i) = ends.peek().filter(|&&&i| spans[i].to <= from) {
                ends.next();
                winners.remove(spans[i].order, &declared[spans[i].style]);
            }
            while let Some(&&i) = starts.peek().filter(|&&&i| spans[i].from <= from) {
                starts.next();
                winners.add(spans[i].order, &declared[spans[i].style]);
            }
            let style = winners.style();
            if style.is_plain() {
                continue;
            }
            let id = styles.intern(&style);
            match out.last_mut() {
                Some(last) if last.style == id && last.to == from => last.to = to,
                _ => out.push(Highlight { from, to, style: id }),
            }
        }
        out
    }
}

/// A range to compose: its place, its order among the ranges given, and
/// its declared style.
struct Span {
    from: usize,
    to: usize,
    order: usize,
    style: usize,
}

/// What the open ranges of a composition ask for, attribute by
/// attribute: the values asked for, by the order the ranges were given
/// in. The topmost, if any, is a stretch's.
#[derive(Default)]
struct Winners {
    fg: BTreeMap<usize, Color>,
    bg: BTreeMap<usize, Color>,
    weight: BTreeMap<usize, Weight>,
    slant: BTreeMap<usize, Slant>,
    underline: BTreeMap<usize, (Underline, Option<Color>)>,
    strike: BTreeMap<usize, bool>,
    extend: BTreeMap<usize, bool>,
    family: BTreeMap<usize, Arc<str>>,
    height: BTreeMap<usize, u16>,
}

/// Each attribute of a style, with its stack in `Winners`.
macro_rules! attributes {
    ($m:ident) => {
        $m!(fg);
        $m!(bg);
        $m!(weight);
        $m!(slant);
        $m!(underline);
        $m!(strike);
        $m!(extend);
        $m!(family);
        $m!(height);
    };
}

impl Winners {
    fn add(&mut self, order: usize, s: &Style) {
        macro_rules! add {
            ($a:ident) => {
                if let Some(v) = &s.$a {
                    self.$a.insert(order, v.clone());
                }
            };
        }
        attributes!(add);
    }

    fn remove(&mut self, order: usize, s: &Style) {
        macro_rules! remove {
            ($a:ident) => {
                if s.$a.is_some() {
                    self.$a.remove(&order);
                }
            };
        }
        attributes!(remove);
    }

    fn style(&self) -> Style {
        let mut style = Style::default();
        macro_rules! top {
            ($a:ident) => {
                style.$a = self.$a.values().next_back().cloned();
            };
        }
        attributes!(top);
        style
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restyling_over_and_over_does_not_grow_the_styles_for_ever() {
        let style = |n: u8| Style { fg: Some(Color::new(n, 0, 0)), ..Style::default() };
        let mut f = Faces::new(1, vec![("keyword".into(), style(0))]);
        for g in 0..20_000u64 {
            f.set_table(
                g + 2,
                vec![
                    ("keyword".into(), style((g % 251) as u8)),
                    (
                        "x".into(),
                        Style {
                            weight: Some(Weight::Bold),
                            fg: Some(Color::new((g % 256) as u8, (g / 256 % 256) as u8, 1)),
                            ..Style::default()
                        },
                    ),
                ],
            );
            f.face_id_of("keyword");
            f.face_id_of("x");
        }
        assert!(f.table().len() <= MAX_STYLES + 3, "{}", f.table().len());
    }

    fn fg(c: Color) -> Style {
        Style { fg: Some(c), ..Style::default() }
    }

    fn faces() -> Faces {
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
        let (comment, search) = (f.name("comment"), f.name("search"));
        // A comment, with a search match over its middle: the match
        // keeps the comment's foreground and adds its own background.
        let out = f.compose(&[FaceRange { from: 0, to: 10, face: comment }, FaceRange { from: 3, to: 6, face: search }]);
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
        // the innermost stretch takes the last one's.
        let n = 4000;
        let table: Vec<(String, Style)> = (0..4).map(|i| (format!("f{i}"), fg(Color::new(i as u8, 0, 0)))).collect();
        let mut f = Faces::new(1, table);
        let names: Vec<FaceName> = (0..4).map(|i| f.name(&format!("f{i}"))).collect();
        let ranges: Vec<FaceRange> = (0..n).map(|i| FaceRange { from: i, to: 2 * n - i, face: names[i % 4] }).collect();
        let out = f.compose(&ranges);
        assert_eq!(out.len(), 2 * n - 1);
        let middle = &out[n - 1];
        assert_eq!((middle.from, middle.to), (n - 1, n + 1));
        assert_eq!(f.table()[middle.style as usize].fg, Some(Color::new(((n - 1) % 4) as u8, 0, 0)));
    }

    #[test]
    fn names_fall_back_to_dotted_prefixes() {
        let mut f = faces();
        let doc = f.name("comment.doc");
        assert_eq!(f.style_of(doc).and_then(|s| s.fg), Some(Color::new(1, 0, 0)));
        let nothing = f.name("nothing.here");
        assert_eq!(f.style_of(nothing), None);
        assert_eq!(f.take_unknown(), ["nothing.here"]);
        // Once per name, also across tables.
        f.set_table(2, vec![("comment".into(), fg(Color::new(2, 0, 0)))]);
        assert_eq!(f.style_of(nothing), None);
        assert!(f.take_unknown().is_empty());
        // A new table resolves names anew.
        assert_eq!(f.style_of(doc).and_then(|s| s.fg), Some(Color::new(2, 0, 0)));
    }
}
