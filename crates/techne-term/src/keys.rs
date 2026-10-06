//! Terminal input in the runtime's Emacs notation: the terminal's key
//! normalizer (EDITOR.md, sections 6 and 8). Keymaps only ever see what it
//! names; what it cannot name is reported, never dropped.
//!
//! Terminals send keys as bytes. In the legacy encoding some chords share
//! bytes with others (C-i is TAB, C-/ and C-_ are both 0x1f) or send
//! nothing distinct at all (C-1, C-S-a): each byte decodes to one canonical
//! key, and `sendable` says which chords cannot arrive as themselves. With
//! the kitty keyboard protocol (flags 1 and 4: disambiguate, and report the
//! shifted key) every chord has its own sequence.

use std::mem;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    Legacy,
    Kitty,
}

/// The kitty keyboard flags we ask for: disambiguate escape codes (1) and
/// report alternate keys (4), so that C-? arrives as C-? and not C-S-/.
pub const KITTY_FLAGS: u8 = 5;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A key in Emacs notation.
    Key(String),
    Mouse(Mouse),
    /// Input that names no key, as it came (escaped).
    Unrecognized(String),
    /// The terminal answered the kitty keyboard query: it has the protocol.
    KittyKeyboard,
    /// The terminal answered the device attributes query. Terminals answer
    /// queries in order, so after this one a kitty answer will not come.
    DeviceAttributes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mouse {
    pub kind: MouseKind,
    /// Zero-based cell.
    pub col: usize,
    pub row: usize,
    pub shift: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseKind {
    Press,
    Drag,
    Release,
    WheelUp,
    WheelDown,
    /// Other buttons, which are not used (as in the window).
    Other,
}

/// Decodes a stream of terminal input. A sequence split across reads waits
/// for the rest; `flush` ends it after a pause (a lone ESC is the Escape
/// key).
#[derive(Default)]
pub struct Decoder {
    pending: Vec<u8>,
}

impl Decoder {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Event> {
        self.pending.extend_from_slice(bytes);
        let buf = mem::take(&mut self.pending);
        let mut events = Vec::new();
        let mut at = 0;
        while at < buf.len() {
            match parse(&buf[at..]) {
                Some((event, used)) => {
                    events.extend(event);
                    at += used;
                }
                None => break,
            }
        }
        self.pending = buf[at..].to_vec();
        events
    }

    /// Whether an incomplete sequence waits for more input.
    pub fn waiting(&self) -> bool {
        !self.pending.is_empty()
    }

    /// End an incomplete sequence: ESC alone is the Escape key, and ESC
    /// followed by `[` or `O` the meta key of those; anything else is
    /// reported.
    pub fn flush(&mut self) -> Vec<Event> {
        flush(&mem::take(&mut self.pending))
    }
}

fn flush(b: &[u8]) -> Vec<Event> {
    match b {
        [] => vec![],
        [0x1b] => vec![Event::Key("ESC".into())],
        [0x1b, c @ (b'[' | b'O')] => vec![Event::Key(format!("M-{}", *c as char))],
        [0x1b, rest @ ..] if rest[0] == 0x1b => match &flush(rest)[..] {
            [Event::Key(k)] => vec![Event::Key(add_meta(k))],
            _ => vec![unrecognized(b)],
        },
        _ => vec![unrecognized(b)],
    }
}

fn unrecognized(bytes: &[u8]) -> Event {
    Event::Unrecognized(bytes.escape_ascii().to_string())
}

/// One event from the start of `b` and the bytes it used; `None` while the
/// sequence is incomplete.
fn parse(b: &[u8]) -> Option<(Option<Event>, usize)> {
    match b[0] {
        0x1b => parse_escape(b),
        0x00..=0x1f | 0x7f => Some((Some(Event::Key(control(b[0]))), 1)),
        _ => parse_utf8(b),
    }
}

/// The legacy name of a control byte.
fn control(c: u8) -> String {
    match c {
        0x00 => "C-SPC".into(),
        0x09 => "TAB".into(),
        0x0d => "RET".into(),
        0x1b => "ESC".into(),
        0x7f => "DEL".into(),
        0x01..=0x1a => format!("C-{}", (c - 1 + b'a') as char),
        _ => format!("C-{}", (c + 0x40) as char),
    }
}

fn parse_utf8(b: &[u8]) -> Option<(Option<Event>, usize)> {
    let len = match b[0] {
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        _ => 1,
    };
    if b.len() < len {
        // Wait for the rest, unless what is there already cannot be it.
        return if b[1..].iter().all(|&x| x & 0xc0 == 0x80) { None } else { Some((Some(unrecognized(&b[..1])), 1)) };
    }
    match std::str::from_utf8(&b[..len]) {
        Ok(s) => Some((Some(Event::Key(char_name(s.chars().next().expect("one char")))), len)),
        Err(_) => Some((Some(unrecognized(&b[..1])), 1)),
    }
}

fn char_name(c: char) -> String {
    if c == ' ' { "SPC".into() } else { c.to_string() }
}

fn parse_escape(b: &[u8]) -> Option<(Option<Event>, usize)> {
    match b.get(1)? {
        b'[' => parse_csi(b),
        b'O' => {
            let key = match b.get(2)? {
                b'A' => "<up>",
                b'B' => "<down>",
                b'C' => "<right>",
                b'D' => "<left>",
                b'H' => "<home>",
                b'F' => "<end>",
                b'P' => "<f1>",
                b'Q' => "<f2>",
                b'R' => "<f3>",
                b'S' => "<f4>",
                _ => return Some((Some(unrecognized(&b[..3])), 3)),
            };
            Some((Some(Event::Key(key.into())), 3))
        }
        // Meta: ESC before a key. ESC before anything else was the Escape
        // key on its own.
        _ => match parse(&b[1..])? {
            (Some(Event::Key(k)), used) => Some((Some(Event::Key(add_meta(&k))), used + 1)),
            _ => Some((Some(Event::Key("ESC".into())), 1)),
        },
    }
}

/// `key` with M- in its place among the modifiers (after C-).
fn add_meta(key: &str) -> String {
    match key.strip_prefix("C-") {
        Some(rest) if !rest.is_empty() => format!("C-M-{rest}"),
        _ => format!("M-{key}"),
    }
}

/// A control sequence: ESC [ parameters final.
fn parse_csi(b: &[u8]) -> Option<(Option<Event>, usize)> {
    let end = match b[2..].iter().position(|x| !(0x20..=0x3f).contains(x)) {
        Some(i) => i + 2,
        None if b.len() < 64 => return None,
        None => return Some((Some(unrecognized(b)), b.len())),
    };
    let (params, fin) = (&b[2..end], b[end]);
    if !(0x40..=0x7e).contains(&fin) {
        // Not a final byte: the sequence ends before it.
        return Some((Some(unrecognized(&b[..end])), end));
    }
    let event = csi_event(params, fin).unwrap_or_else(|| unrecognized(&b[..=end]));
    Some((Some(event), end + 1))
}

fn csi_event(params: &[u8], fin: u8) -> Option<Event> {
    let params = std::str::from_utf8(params).ok()?;
    if let Some(p) = params.strip_prefix('<') {
        return sgr_mouse(p, fin).map(Event::Mouse);
    }
    if let Some(p) = params.strip_prefix('?') {
        return match fin {
            b'u' if p.bytes().all(|c| c.is_ascii_digit()) => Some(Event::KittyKeyboard),
            b'c' => Some(Event::DeviceAttributes),
            _ => None,
        };
    }
    // number[:alternates];modifiers[:event][;text]
    let fields: Vec<Vec<&str>> = params.split(';').map(|f| f.split(':').collect()).collect();
    let num = |i: usize, j: usize| -> Option<u32> {
        match fields.get(i).and_then(|f| f.get(j)) {
            None | Some(&"") => None,
            Some(s) => s.parse().ok(),
        }
    };
    // Modifiers are sent plus one; caps and num lock (64, 128) are ignored.
    let mods = Mods::from_bits(num(1, 0).unwrap_or(1).checked_sub(1)?);
    // Only presses (event type 1) are asked for.
    if num(1, 1).is_some_and(|e| e != 1) {
        return None;
    }
    let named = |name: &str| Some(Event::Key(mods.name(Base::Named(name))));
    match fin {
        b'A' => named("<up>"),
        b'B' => named("<down>"),
        b'C' => named("<right>"),
        b'D' => named("<left>"),
        b'H' => named("<home>"),
        b'F' => named("<end>"),
        b'P' => named("<f1>"),
        b'Q' => named("<f2>"),
        b'R' => named("<f3>"),
        b'S' => named("<f4>"),
        b'Z' => named("<backtab>"),
        b'~' => named(match num(0, 0)? {
            1 | 7 => "<home>",
            2 => "<insert>",
            3 => "<delete>",
            4 | 8 => "<end>",
            5 => "<prior>",
            6 => "<next>",
            11 => "<f1>",
            12 => "<f2>",
            13 => "<f3>",
            14 => "<f4>",
            15 => "<f5>",
            17 => "<f6>",
            18 => "<f7>",
            19 => "<f8>",
            20 => "<f9>",
            21 => "<f10>",
            23 => "<f11>",
            24 => "<f12>",
            _ => return None,
        }),
        b'u' => {
            let base = match num(0, 0)? {
                9 => Base::Named("TAB"),
                13 => Base::Named("RET"),
                27 => Base::Named("ESC"),
                127 => Base::Named("DEL"),
                // Functional keys of the protocol's private use area are
                // not named yet.
                c => Base::Char(char::from_u32(c).filter(|c| !c.is_control() && !('\u{e000}'..='\u{f8ff}').contains(c))?),
            };
            // With shift, the key it gives (C-? rather than C-S-/).
            let (base, mods) = match (base, num(0, 1).and_then(char::from_u32)) {
                (Base::Char(_), Some(shifted)) if mods.shift => (Base::Char(shifted), Mods { shift: false, ..mods }),
                other => (other.0, mods),
            };
            Some(Event::Key(mods.name(base)))
        }
        _ => None,
    }
}

fn sgr_mouse(params: &str, fin: u8) -> Option<Mouse> {
    let n: Vec<usize> = params.split(';').map(|s| s.parse().ok()).collect::<Option<_>>()?;
    let [button, col, row] = n[..] else { return None };
    let kind = match (button & !(4 | 8 | 16), fin) {
        (0, b'M') => MouseKind::Press,
        (32, b'M') => MouseKind::Drag,
        (0, b'm') => MouseKind::Release,
        (64, _) => MouseKind::WheelUp,
        (65, _) => MouseKind::WheelDown,
        _ => MouseKind::Other,
    };
    Some(Mouse { kind, col: col.checked_sub(1)?, row: row.checked_sub(1)?, shift: button & 4 != 0 })
}

#[derive(Clone, Copy, Debug, Default)]
struct Mods {
    shift: bool,
    meta: bool,
    ctrl: bool,
    sup: bool,
    hyper: bool,
}

#[derive(Clone, Copy)]
enum Base<'a> {
    Named(&'a str),
    Char(char),
}

impl Mods {
    fn from_bits(bits: u32) -> Mods {
        Mods { shift: bits & 1 != 0, meta: bits & (2 | 32) != 0, ctrl: bits & 4 != 0, sup: bits & 8 != 0, hyper: bits & 16 != 0 }
    }

    /// The key's name, with the modifiers in the window's order. Shift is
    /// spelled out only for `<named>` keys; a character is already the one
    /// shift gives.
    fn name(self, base: Base) -> String {
        let (base, shift) = match base {
            Base::Named(n) => (n.to_string(), self.shift && n.starts_with('<')),
            Base::Char(c) if self.shift && c.is_ascii_lowercase() => (c.to_ascii_uppercase().to_string(), false),
            Base::Char(c) => (char_name(c), false),
        };
        [(self.ctrl, "C-"), (self.meta, "M-"), (self.sup, "s-"), (self.hyper, "H-"), (shift, "S-")]
            .iter()
            .filter(|(on, _)| *on)
            .map(|(_, p)| *p)
            .chain([base.as_str()])
            .collect()
    }
}

/// Whether a terminal speaking `protocol` can send `key` as itself.
pub fn sendable(key: &str, protocol: Protocol) -> bool {
    if protocol == Protocol::Kitty {
        return true;
    }
    let (mut base, mut ctrl, mut shift, mut other) = (key, false, false, false);
    while let Some((m, rest)) = base.split_once('-').filter(|(m, rest)| m.len() == 1 && !rest.is_empty()) {
        match m {
            "C" => ctrl = true,
            "S" => shift = true,
            "M" => {}
            _ => other = true,
        }
        base = rest;
    }
    if other {
        return false;
    }
    let mut chars = base.chars();
    match (chars.next(), chars.next()) {
        // Named keys carry their modifiers as a parameter.
        _ if base.starts_with('<') => true,
        _ if shift => false,
        // C-SPC is NUL; C-RET, C-TAB, C-DEL, C-ESC are their keys alone.
        _ if base == "SPC" => true,
        _ if matches!(base, "RET" | "TAB" | "DEL" | "ESC") => !ctrl,
        (Some(c), None) => !ctrl || (c.is_ascii_lowercase() && c != 'i' && c != 'm') || matches!(c, '\\' | ']' | '^' | '_'),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(bytes: &[u8]) -> Vec<String> {
        let mut d = Decoder::default();
        d.feed(bytes)
            .into_iter()
            .chain(d.flush())
            .map(|e| match e {
                Event::Key(k) => k,
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn legacy_keys() {
        assert_eq!(keys(b"a A\x01\x00\t\r\x7f\x1f\x1d"), ["a", "SPC", "A", "C-a", "C-SPC", "TAB", "RET", "DEL", "C-_", "C-]"]);
        assert_eq!(keys("é名".as_bytes()), ["é", "名"]);
        assert_eq!(keys(b"\x1bf\x1b<\x1b\x01\x1b\x7f"), ["M-f", "M-<", "C-M-a", "M-DEL"]);
        assert_eq!(
            keys(b"\x1b[D\x1b[1;5C\x1b[1;2A\x1bOH\x1b[3~\x1b[5;3~\x1b[15~"),
            ["<left>", "C-<right>", "S-<up>", "<home>", "<delete>", "M-<prior>", "<f5>"]
        );
        assert_eq!(keys(b"\x1b"), ["ESC"]);
        assert_eq!(keys(b"\x1b\x1b[D"), ["M-<left>"]);
    }

    #[test]
    fn kitty_keys() {
        assert_eq!(keys(b"\x1b[47;5u\x1b[47:63;6u\x1b[97;5u\x1b[105;5u\x1b[97:65;6u"), ["C-/", "C-?", "C-a", "C-i", "C-A"]);
        assert_eq!(
            keys(b"\x1b[27u\x1b[13;5u\x1b[32;5u\x1b[44:60;4u\x1b[95;7u\x1b[97;133u"),
            ["ESC", "C-RET", "C-SPC", "M-<", "C-M-_", "C-a"]
        );
        let mut d = Decoder::default();
        assert_eq!(d.feed(b"\x1b[?5u\x1b[?62;22c"), [Event::KittyKeyboard, Event::DeviceAttributes]);
    }

    #[test]
    fn split_sequences_wait_and_unknown_ones_are_reported() {
        let mut d = Decoder::default();
        assert_eq!(d.feed(b"x\x1b[1;5"), [Event::Key("x".into())]);
        assert!(d.waiting());
        assert_eq!(d.feed(b"D\xc3"), [Event::Key("C-<left>".into())]);
        assert_eq!(d.feed(b"\xa9"), [Event::Key("é".into())]);
        assert_eq!(
            d.feed(b"\x1b[99~\xffq"),
            [Event::Unrecognized("\\x1b[99~".into()), Event::Unrecognized("\\xff".into()), Event::Key("q".into())]
        );
        assert_eq!(d.feed(b"\x1b[57441;2u"), [Event::Unrecognized("\\x1b[57441;2u".into())]);
        assert_eq!(d.feed(b"\x1b["), []);
        assert_eq!(d.flush(), [Event::Key("M-[".into())]);
        assert_eq!(d.feed(b"\x1b\x1b"), []);
        assert_eq!(d.flush(), [Event::Key("M-ESC".into())]);
    }

    #[test]
    fn mouse() {
        let mut d = Decoder::default();
        let m = |kind, col, row, shift| Event::Mouse(Mouse { kind, col, row, shift });
        assert_eq!(
            d.feed(b"\x1b[<0;5;2M\x1b[<32;6;2M\x1b[<0;6;2m\x1b[<4;1;1M\x1b[<65;3;3M\x1b[<2;1;1M"),
            [
                m(MouseKind::Press, 4, 1, false),
                m(MouseKind::Drag, 5, 1, false),
                m(MouseKind::Release, 5, 1, false),
                m(MouseKind::Press, 0, 0, true),
                m(MouseKind::WheelDown, 2, 2, false),
                m(MouseKind::Other, 0, 0, false),
            ]
        );
    }

    #[test]
    fn what_a_legacy_terminal_can_send() {
        let legacy = |k| sendable(k, Protocol::Legacy);
        for k in ["a", "é", "C-a", "C-x", "C-_", "C-SPC", "M-<", "C-M-_", "M-DEL", "RET", "S-<left>", "C-<f5>", "M-x"] {
            assert!(legacy(k), "{k}");
        }
        for k in ["C-/", "C-?", "C-i", "C-m", "C-A", "C-1", "C-RET", "S-RET", "s-a", "H-x", "C-M-/"] {
            assert!(!legacy(k), "{k}");
            assert!(sendable(k, Protocol::Kitty), "{k}");
        }
    }
}
