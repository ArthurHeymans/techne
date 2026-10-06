//! Terminal keys in the runtime's Emacs notation, and which keys a terminal
//! can send at all (EDITOR.md, sections 6 and 8).
//!
//! Without the kitty keyboard protocol a terminal sends control characters:
//! C-/ arrives as C-_ (as in Emacs, both undo), C-? cannot be told from DEL,
//! and Control with a digit or most punctuation is not sent. Such keys are
//! reported as unbindable rather than silently never arriving.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// The key, or `None` for releases and keys not bound by name.
pub fn key_name(ev: &KeyEvent) -> Option<String> {
    if ev.kind == KeyEventKind::Release {
        return None;
    }
    let m = ev.modifiers;
    let (ctrl, alt) = (m.contains(KeyModifiers::CONTROL), m.contains(KeyModifiers::ALT));
    let base = match ev.code {
        // Legacy terminals send C-\ C-] C-^ C-_ as bytes 0x1c-0x1f, which
        // crossterm reports as Control with 4-7.
        KeyCode::Char(c @ '4'..='7') if ctrl => ['\\', ']', '^', '_'][c as usize - '4' as usize].to_string(),
        KeyCode::Char(' ') => "SPC".into(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "RET".into(),
        KeyCode::Backspace => "DEL".into(),
        KeyCode::Esc => "ESC".into(),
        KeyCode::Tab => "TAB".into(),
        KeyCode::BackTab => "<backtab>".into(),
        KeyCode::Delete => "<delete>".into(),
        KeyCode::Insert => "<insert>".into(),
        KeyCode::Left => "<left>".into(),
        KeyCode::Right => "<right>".into(),
        KeyCode::Up => "<up>".into(),
        KeyCode::Down => "<down>".into(),
        KeyCode::Home => "<home>".into(),
        KeyCode::End => "<end>".into(),
        KeyCode::PageUp => "<prior>".into(),
        KeyCode::PageDown => "<next>".into(),
        KeyCode::F(n) => format!("<f{n}>"),
        _ => return None,
    };
    let mut name = String::new();
    if ctrl {
        name.push_str("C-");
    }
    if alt {
        name.push_str("M-");
    }
    if m.contains(KeyModifiers::SUPER) {
        name.push_str("s-");
    }
    if m.contains(KeyModifiers::SHIFT) && base.starts_with('<') {
        name.push_str("S-");
    }
    name.push_str(&base);
    Some(name)
}

/// Whether a terminal without the kitty keyboard protocol can send `key`
/// (one key in Emacs notation).
pub fn legacy_sendable(key: &str) -> bool {
    let mut rest = key;
    let (mut ctrl, mut sup) = (false, false);
    loop {
        if let Some(r) = rest.strip_prefix("C-") {
            ctrl = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("M-") {
            // Meta is an ESC prefix: always possible.
            rest = r;
        } else if let Some(r) = rest.strip_prefix("s-") {
            sup = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("S-").filter(|r| r.starts_with('<')) {
            rest = r;
        } else {
            break;
        }
    }
    if sup {
        return false;
    }
    if !ctrl {
        return true;
    }
    match rest {
        // Function and cursor keys carry modifiers in their escape sequences.
        r if r.starts_with('<') => true,
        "SPC" => true,
        r => {
            let mut chars = r.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => c.is_ascii_lowercase() || "@[\\]^_".contains(c),
                _ => false,
            }
        }
    }
}

/// The bound key sequences a terminal without the kitty keyboard protocol
/// cannot send.
pub fn unsendable(bound: &[String]) -> Vec<String> {
    let mut v: Vec<String> = bound.iter().filter(|seq| !seq.split(' ').all(legacy_sendable)).cloned().collect();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(code: KeyCode, mods: KeyModifiers) -> Option<String> {
        key_name(&KeyEvent::new(code, mods))
    }

    #[test]
    fn emacs_notation() {
        let none = KeyModifiers::NONE;
        assert_eq!(name(KeyCode::Char('a'), none).as_deref(), Some("a"));
        assert_eq!(name(KeyCode::Char('A'), KeyModifiers::SHIFT).as_deref(), Some("A"));
        assert_eq!(name(KeyCode::Char('x'), KeyModifiers::CONTROL).as_deref(), Some("C-x"));
        assert_eq!(name(KeyCode::Char('<'), KeyModifiers::ALT).as_deref(), Some("M-<"));
        assert_eq!(name(KeyCode::Char(' '), KeyModifiers::CONTROL).as_deref(), Some("C-SPC"));
        assert_eq!(name(KeyCode::Char('7'), KeyModifiers::CONTROL).as_deref(), Some("C-_"));
        assert_eq!(name(KeyCode::Char('7'), KeyModifiers::CONTROL | KeyModifiers::ALT).as_deref(), Some("C-M-_"));
        assert_eq!(name(KeyCode::Enter, none).as_deref(), Some("RET"));
        assert_eq!(name(KeyCode::Left, KeyModifiers::SHIFT).as_deref(), Some("S-<left>"));
        assert_eq!(name(KeyCode::Char('é'), none).as_deref(), Some("é"));
    }

    #[test]
    fn what_a_legacy_terminal_can_send() {
        for k in ["a", "C-x", "M-f", "C-M-_", "C-SPC", "C-_", "M-DEL", "C-<left>", "S-<up>", "<f5>", "M-<"] {
            assert!(legacy_sendable(k), "{k}");
        }
        for k in ["C-/", "C-?", "C-1", "C-;", "C-RET", "s-a", "C-A"] {
            assert!(!legacy_sendable(k), "{k}");
        }
        let bound = ["C-x C-s", "C-?", "C-x C-;", "C-/"].map(String::from);
        assert_eq!(unsendable(&bound), ["C-/", "C-?", "C-x C-;"]);
    }
}
