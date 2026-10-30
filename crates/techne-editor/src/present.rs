//! The presentation protocol between the runtime and a frontend (EDITOR.md,
//! section 6): panes, each a view of a text document.
//!
//! Everything here is plain data: no handles to runtime objects and no
//! callbacks, even with frontend and runtime in one process. A snapshot is
//! an immutable text (a rope shares its chunks, so it is cheap to send) at a
//! document revision; the frontend lays out only what it shows. Input that
//! the frontend resolved against a snapshot (a click, a scroll) names that
//! snapshot's revision, and the runtime maps it to the current text or
//! refuses it.
//!
//! Rows of a text document are its lines, addressed by source position.
//! The minibuffer's candidates are logical rows (EDITOR.md, section 2) of
//! styled runs in columns; keys and deltas for rows come when a frontend
//! needs to be sent less than a whole snapshot.
//!
//! Faces (EDITOR.md, section 3) are defined in Lisp and composed by the
//! runtime: a snapshot carries a table of resolved styles (`Style`), and
//! text refers to them by id. A style is partial: an attribute it does
//! not set is taken from what is under it, in the end from the pane's
//! default style. Face names never reach a frontend.

use std::{sync::Arc, time::Instant};

use techne_text::ropey::Rope;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorShape {
    Bar,
    Block,
}

/// A colour: sRGB, with the ANSI colour (0 to 15) a frontend that has
/// only those should use, when the palette said one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Color {
    pub rgb: [u8; 3],
    pub ansi: Option<u8>,
}

impl Color {
    pub const fn new(r: u8, g: u8, b: u8) -> Color {
        Color { rgb: [r, g, b], ansi: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Weight {
    Light,
    Normal,
    Bold,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Slant {
    Normal,
    Italic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Underline {
    /// No underline, in particular: over a style that has one, this
    /// takes it away, where an unset underline would leave it.
    None,
    Line,
    Curl,
    Dotted,
    Dashed,
    Double,
}

/// How text is drawn, as far as one face (or a composition of faces)
/// says: every attribute may be unset, meaning what is under it decides.
/// Frontends realize a style with what they have: a terminal ignores
/// `family` and `height`, say.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub weight: Option<Weight>,
    pub slant: Option<Slant>,
    pub underline: Option<(Underline, Option<Color>)>,
    pub strike: Option<bool>,
    /// The background reaches the pane's edge when the style covers a
    /// line's end.
    pub extend: Option<bool>,
    /// A font family, or `fixed` / `variable`. Only a pane's default
    /// style is asked for it: fonts do not change inside a line.
    pub family: Option<Arc<str>>,
    /// The text's size, as a percentage of the frontend's. As `family`,
    /// only on a pane's default style.
    pub height: Option<u16>,
}

impl Style {
    /// This style with every attribute `over` sets taken from it.
    pub fn patch(&self, over: &Style) -> Style {
        Style {
            fg: over.fg.or(self.fg),
            bg: over.bg.or(self.bg),
            weight: over.weight.or(self.weight),
            slant: over.slant.or(self.slant),
            underline: over.underline.or(self.underline),
            strike: over.strike.or(self.strike),
            extend: over.extend.or(self.extend),
            family: over.family.clone().or_else(|| self.family.clone()),
            height: over.height.or(self.height),
        }
    }

    /// Whether it sets no attribute at all.
    pub fn is_plain(&self) -> bool {
        *self == Style::default()
    }
}

/// A style of a snapshot's table, by its place in it.
pub type StyleId = u32;

/// The styles of what is not buffer text: each the style of the face of
/// its name (`mode-line`, `line-number`...), resolved by the runtime.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Chrome {
    pub region: StyleId,
    pub cursor: StyleId,
    pub cursor_inactive: StyleId,
    pub mode_line: StyleId,
    pub mode_line_inactive: StyleId,
    pub border: StyleId,
    pub line_number: StyleId,
    pub line_number_current: StyleId,
    /// The marker below the end of the text.
    pub eob_marker: StyleId,
    pub completion: StyleId,
    pub completion_selected: StyleId,
    /// The minibuffer's selected candidate (and its input, taken as typed).
    pub selected: StyleId,
    pub prompt: StyleId,
    /// Which-key's popup: a key, the separator after it, and the
    /// description of a prefix (a group of keys).
    pub which_key_key: StyleId,
    pub which_key_separator: StyleId,
    pub which_key_group: StyleId,
}

/// What a frontend shows: panes, one view each, each in its place, and the
/// echo area below them.
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// Increases with every snapshot.
    pub id: u64,
    pub panes: Vec<Pane>,
    /// The pane keys go to.
    pub focus: usize,
    /// The echo area: the session's message or prompt.
    pub echo: String,
    /// The minibuffer, while it is open: it has the keys then.
    pub minibuffer: Option<Minibuffer>,
    /// In-buffer completion's popup, while it is open.
    pub completion: Option<Completion>,
    /// The keys that can follow the prefix being typed, while which-key
    /// shows them; the frontend arranges them in columns to fit.
    pub key_hints: Vec<KeyHint>,
    /// When the inputs this snapshot answers were made, so the frontend can
    /// measure input to frame.
    pub answers: Vec<Instant>,
    /// The styles the snapshot's text and chrome refer to, by id.
    pub styles: Arc<[Style]>,
    pub chrome: Chrome,
}

impl Snapshot {
    /// The focused pane.
    pub fn pane(&self) -> &Pane {
        &self.panes[self.focus]
    }

    /// The style of the table with id `id`; a plain style for an id the
    /// table does not have (a part kept from an older snapshot).
    pub fn style(&self, id: StyleId) -> Style {
        self.styles.get(id as usize).cloned().unwrap_or_default()
    }
}

/// One view of a document: its text at a revision, the view's selections
/// and scroll anchor, its mode line and highlights.
#[derive(Clone, Debug)]
pub struct Pane {
    /// The view, for inputs addressed to it.
    pub view: u64,
    pub revision: u64,
    pub text: Rope,
    /// (anchor, head) byte positions; carets are empty ranges.
    pub selections: Vec<(usize, usize)>,
    pub primary: usize,
    pub cursor: CursorShape,
    /// The scroll anchor: a position on the first visible visual line.
    pub scroll: usize,
    /// The pane's mode line, as Lisp composed it.
    pub status: String,
    /// Highlighted ranges of the text near the scroll anchor: the
    /// session's layers, composed by the runtime into flat ranges that
    /// do not overlap, in order.
    pub layers: Vec<Highlight>,
    /// The pane's default style: what its text is drawn with where no
    /// highlight says otherwise.
    pub default: StyleId,
    /// A visual operation the session asked of this pane, which the
    /// frontend resolves with its layout and answers with `Input::Scroll`
    /// (or `Input::Edge`); set in the snapshot answering the key.
    pub request: Option<ViewRequest>,
    /// Where the pane is, as fractions of the frame's area for panes: the
    /// window tree is Lisp's, the frontend realizes it in lines and cells
    /// (EDITOR.md, section 9).
    pub place: Place,
    /// The display options of the pane's buffer, which the frontend draws
    /// in a gutter beside the text.
    pub display: Display,
}

/// What a frontend draws beside a pane's text, never in it: line numbers
/// and a marker on the lines past the end of the text (Vim's `~`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Display {
    pub line_numbers: LineNumbers,
    pub eob_marker: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LineNumbers {
    #[default]
    Off,
    /// Each line's number, from 1.
    Absolute,
    /// How far each line is from the caret's, which shows its own number,
    /// as Emacs's `relative` with `display-line-numbers-current-absolute`.
    Relative,
}

/// Line numbers take at least this many digits, as Arthur's
/// `display-line-numbers-width`.
const NUMBER_WIDTH: usize = 3;

impl Display {
    /// The gutter's width in characters for a text of `lines` lines: the
    /// numbers right-aligned, a space each side, as Emacs pads them. No
    /// gutter without numbers; the marker is drawn where the text would be.
    pub fn gutter(&self, lines: usize) -> usize {
        match self.line_numbers {
            LineNumbers::Off => 0,
            _ => lines.to_string().len().max(NUMBER_WIDTH) + 2,
        }
    }

    /// The gutter's text for line `line` (from 0), the caret on `current`,
    /// in a gutter `width` wide.
    pub fn number(&self, line: usize, current: usize, width: usize) -> String {
        let n = match self.line_numbers {
            LineNumbers::Off => return String::new(),
            LineNumbers::Relative if line != current => line.abs_diff(current),
            _ => line + 1,
        };
        format!("{n:>w$} ", w = width.saturating_sub(1))
    }
}

/// A visual operation on a pane (EDITOR.md, section 6: the frontend
/// resolves it, the runtime applies the semantic positions it gets back).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ViewRequest {
    /// Scroll by screens (negative: up), keeping `context` lines of the
    /// old screen, the caret keeping its place on the screen; a fraction
    /// scrolls that part of a screen.
    Page { screens: f32, context: usize },
    /// Scroll so the caret's line is at the middle, top or bottom.
    Recenter(Recenter),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recenter {
    Middle,
    Top,
    Bottom,
}

/// A rectangle in fractions (0 to 1) of an area.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Place {
    pub const WHOLE: Place = Place { x: 0.0, y: 0.0, w: 1.0, h: 1.0 };
}

impl Pane {
    pub fn head(&self) -> usize {
        self.selections[self.primary].1
    }
}

/// A range of a pane's text drawn with a style of the snapshot's table,
/// over the pane's default style.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Highlight {
    pub from: usize,
    pub to: usize,
    pub style: StyleId,
}

/// The minibuffer (EDITOR.md, section 10): a prompt, the input being
/// typed and the candidates that match it, as many as the session shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Minibuffer {
    pub prompt: String,
    pub input: String,
    /// The caret in the input, a byte position.
    pub caret: usize,
    pub rows: Arc<[Row]>,
    /// The row of the candidate RET would take.
    pub selected: Option<usize>,
    /// RET takes the input as typed: it is selected, as vertico's prompt.
    pub input_selected: bool,
}

/// What completes the text before a pane's caret (Corfu's popup): rows of
/// candidates, drawn below the line of `at` (above it when there is no
/// room), aligned with `at`, the start of the text they complete.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    pub view: u64,
    pub at: usize,
    pub rows: Arc<[Row]>,
    /// The row of the selected candidate, which is in the text; none
    /// until one is chosen.
    pub selected: Option<usize>,
}

/// A key that can follow a prefix, and what it does: a command's name, or
/// a prefix's (`+file`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyHint {
    pub key: String,
    pub description: String,
    pub prefix: bool,
}

/// A logical row: columns of styled text. A frontend aligns the columns
/// of the rows it shows, the first at the left, each next one after the
/// widest of the one before.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub columns: Vec<Vec<Run>>,
}

impl Row {
    /// The text of a column.
    pub fn text(&self, column: usize) -> String {
        self.columns.get(column).map(|c| c.iter().map(|r| r.text.as_str()).collect()).unwrap_or_default()
    }
}

/// Text drawn with a style (as `Highlight`), or plainly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub style: Option<StyleId>,
}

#[derive(Clone, Debug)]
pub enum Input {
    /// A key in Emacs notation (`"a"`, `"C-x"`, `"M-<"`, `"RET"`, `"<left>"`).
    Key { key: String, at: Instant },
    /// Put the caret of `view` at `pos` of the text at `revision` (a click),
    /// or extend its selection to it; the view gets the focus.
    Click { view: u64, revision: u64, pos: usize, extend: bool, at: Instant },
    /// Scroll `view` so that `anchor` of the text at `revision` is on its
    /// first visible line; with `caret`, its caret goes there too (paging).
    Scroll { view: u64, revision: u64, anchor: usize, caret: Option<usize> },
    /// A page request found `view` already at the end of its text (or the
    /// start, `end` false): nothing scrolled.
    Edge { view: u64, end: bool },
    /// Bound key sequences (from `Output::Bindings`) that this frontend
    /// cannot send, as its key normalizer found: the session reports them.
    /// Keymaps never see a stand-in key instead.
    Unsendable { keys: Vec<String> },
    /// Input the key normalizer could not name (an escape sequence it does
    /// not know), as it came: reported rather than dropped.
    Unrecognized { input: String },
    /// What the system clipboard holds, when it may have changed (another
    /// program put text there): it becomes the newest kill.
    Clipboard { text: String },
    /// The frontend is closing: it detaches.
    Close,
}

#[derive(Debug)]
pub enum Output {
    Snapshot(Box<Snapshot>),
    /// The key sequences the session binds in Emacs notation, sent when a
    /// frontend attaches, so that its key normalizer can report those it
    /// cannot send.
    Bindings(Vec<String>),
    /// What the session needs to come back after a crash (its buffers,
    /// panes, carets and scroll anchors), as data Lisp reads back; sent
    /// when it changes. The host keeps it; frontends ignore it.
    Session(String),
    /// Text killed: the frontend puts it on the system clipboard.
    Clipboard(String),
    /// The session asked to quit: the frontend is detached.
    Quit,
    /// The frontend could not attach (its file could not be opened), and
    /// why: nothing else comes.
    Refused(String),
}
