//! winit keys in the runtime's Emacs notation: `a`, `A`, `C-x`, `M-<`,
//! `C-M-_`, `RET`, `<left>`. The key normalizer of EDITOR.md, section 6:
//! keymaps only ever see this notation.

use winit::keyboard::{Key, ModifiersState, NamedKey};

/// The key, or `None` for keys that are not bound by name (modifiers alone,
/// dead keys).
pub fn key_name(key: &Key, mods: ModifiersState) -> Option<String> {
    let base = match key {
        Key::Named(named) => named_key(*named)?.to_string(),
        Key::Character(s) => {
            let mut chars = s.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                // A composed string is text, not a key.
                return (!mods.control_key() && !mods.alt_key()).then(|| s.to_string());
            }
            match c {
                ' ' => "SPC".to_string(),
                c => c.to_string(),
            }
        }
        _ => return None,
    };
    let mut name = String::new();
    if mods.control_key() {
        name.push_str("C-");
    }
    if mods.alt_key() {
        name.push_str("M-");
    }
    if mods.super_key() {
        name.push_str("s-");
    }
    // Shift is already in the character; named keys keep it explicitly.
    if mods.shift_key() && matches!(key, Key::Named(_)) && base.starts_with('<') {
        name.push_str("S-");
    }
    name.push_str(&base);
    Some(name)
}

fn named_key(k: NamedKey) -> Option<&'static str> {
    Some(match k {
        NamedKey::Enter => "RET",
        NamedKey::Backspace => "DEL",
        NamedKey::Escape => "ESC",
        NamedKey::Tab => "TAB",
        NamedKey::Space => "SPC",
        NamedKey::Delete => "<delete>",
        NamedKey::ArrowLeft => "<left>",
        NamedKey::ArrowRight => "<right>",
        NamedKey::ArrowUp => "<up>",
        NamedKey::ArrowDown => "<down>",
        NamedKey::Home => "<home>",
        NamedKey::End => "<end>",
        NamedKey::PageUp => "<prior>",
        NamedKey::PageDown => "<next>",
        NamedKey::Insert => "<insert>",
        NamedKey::F1 => "<f1>",
        NamedKey::F2 => "<f2>",
        NamedKey::F3 => "<f3>",
        NamedKey::F4 => "<f4>",
        NamedKey::F5 => "<f5>",
        NamedKey::F6 => "<f6>",
        NamedKey::F7 => "<f7>",
        NamedKey::F8 => "<f8>",
        NamedKey::F9 => "<f9>",
        NamedKey::F10 => "<f10>",
        NamedKey::F11 => "<f11>",
        NamedKey::F12 => "<f12>",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(key: Key, mods: ModifiersState) -> Option<String> {
        key_name(&key, mods)
    }

    #[test]
    fn emacs_notation() {
        let ch = |s: &str| Key::Character(s.into());
        let none = ModifiersState::empty();
        assert_eq!(name(ch("a"), none).as_deref(), Some("a"));
        assert_eq!(name(ch("A"), ModifiersState::SHIFT).as_deref(), Some("A"));
        assert_eq!(name(ch("x"), ModifiersState::CONTROL).as_deref(), Some("C-x"));
        assert_eq!(name(ch("<"), ModifiersState::ALT | ModifiersState::SHIFT).as_deref(), Some("M-<"));
        assert_eq!(name(ch("_"), ModifiersState::CONTROL | ModifiersState::ALT).as_deref(), Some("C-M-_"));
        assert_eq!(name(ch(" "), ModifiersState::CONTROL).as_deref(), Some("C-SPC"));
        assert_eq!(name(Key::Named(NamedKey::Space), ModifiersState::CONTROL).as_deref(), Some("C-SPC"));
        assert_eq!(name(Key::Named(NamedKey::Enter), none).as_deref(), Some("RET"));
        assert_eq!(name(Key::Named(NamedKey::ArrowLeft), ModifiersState::SHIFT).as_deref(), Some("S-<left>"));
        assert_eq!(name(Key::Named(NamedKey::Shift), ModifiersState::SHIFT), None);
        assert_eq!(name(ch("é"), none).as_deref(), Some("é"));
    }
}
