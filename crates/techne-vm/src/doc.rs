//! The docstring convention, and the checks every tool applies: the
//! language server while a file is edited, the editor's `checkdoc` and the
//! test suites that keep the editor's own documentation complete.
//!
//! The convention is Guile's, which Emacs Lisp shares (runtime/TECHNE-VM.md,
//! "Docstrings"):
//!
//! - the first line is a complete sentence, starting with a capital letter
//!   and ending with a period, of at most [`FIRST_LINE_MAX`] characters:
//!   help lists, M-x and completion annotations show only it;
//! - a procedure says what it does in the imperative ("Return", not
//!   "Returns") and names every parameter in upper case ("Return the
//!   length of LIST.");
//! - lines are at most [`LINE_MAX`] characters; continuation lines start in
//!   the source's first column, so that the text is as wide as it looks;
//! - keys are written `\\[command]`, which help shows as the key bound to
//!   the command in the user's profile, never as literal key names;
//! - names of other definitions, and code, are written in backquotes:
//!   `name`.

/// The longest first line.
pub const FIRST_LINE_MAX: usize = 72;
/// The longest line.
pub const LINE_MAX: usize = 80;

/// What is documented, for the rules that depend on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subject {
    /// A procedure: imperative mood, every parameter named.
    Procedure,
    /// A command or an action: imperative mood. Its parameters (the
    /// session, the count or target) are the protocol's, not the user's.
    Command,
    /// A macro, a mode, an option, a hook: a description.
    Other,
}

/// The parameter names in a procedure's parameters as written: `x`,
/// `#:key`, `(x default)` or `[x default]`, `. rest`. Keywords are not
/// parameters; the variable after one is.
pub fn param_names<S: AsRef<str>>(params: &[S]) -> Vec<String> {
    params
        .iter()
        .map(AsRef::as_ref)
        .filter(|p| !p.starts_with("#:"))
        .filter_map(|p| {
            let p = p.trim_start_matches(". ").trim_start_matches(['(', '[']);
            let name = p.split([' ', ')', ']']).next().unwrap_or("");
            (!name.is_empty()).then(|| name.to_string())
        })
        .collect()
}

/// Verbs whose third person ("Returns") starts a docstring that should
/// be in the imperative ("Return").
const VERBS: &[&str] = &[
    "accept", "act", "add", "append", "apply", "ask", "bind", "build", "call", "cancel", "change", "check", "choose", "clear", "close",
    "collect", "compare", "compute", "convert", "copy", "count", "create", "decide", "declare", "define", "delete", "describe", "do",
    "draw", "drop", "edit", "end", "ensure", "evaluate", "exchange", "extend", "fill", "find", "finish", "fold", "follow", "format", "get",
    "give", "go", "hand", "handle", "insert", "inspect", "jump", "keep", "kill", "leave", "list", "load", "look", "make", "map", "mark",
    "match", "move", "name", "note", "offer", "open", "parse", "pass", "put", "raise", "read", "record", "redo", "refresh", "register",
    "remember", "remove", "rename", "replace", "report", "request", "reset", "resolve", "restore", "return", "reverse", "run", "save",
    "scroll", "search", "select", "send", "set", "show", "signal", "sort", "split", "start", "step", "stop", "store", "switch", "take",
    "tell", "test", "toggle", "try", "turn", "undo", "unload", "update", "use", "visit", "wait", "write", "yank",
];

/// The problems of `doc`, the docstring of a `subject` with parameters
/// `params` (names as [`param_names`] gives them): each a sentence saying
/// what to change.
pub fn problems(doc: &str, subject: Subject, params: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let first = doc.lines().next().unwrap_or("");
    if doc.trim().is_empty() {
        out.push("The docstring is empty.".to_string());
        return out;
    }
    if doc != doc.trim() {
        out.push("Remove the whitespace at the start or end of the docstring.".to_string());
    }
    if doc.contains('\t') {
        out.push("Use spaces, not tabs.".to_string());
    }
    if !first.trim_start().starts_with(|c: char| c.is_uppercase() || "`\\(#".contains(c)) {
        out.push("Start the first line with a capital letter.".to_string());
    }
    if !first.trim_end().ends_with('.') {
        out.push("Make the first line a complete sentence, ending with a period.".to_string());
    }
    if first.chars().count() > FIRST_LINE_MAX {
        out.push(format!("Keep the first line within {FIRST_LINE_MAX} characters."));
    }
    if let Some(n) = doc.lines().skip(1).position(|l| l.chars().count() > LINE_MAX) {
        out.push(format!("Line {} is longer than {LINE_MAX} characters.", n + 2));
    }
    if let Some(n) = doc.lines().skip(1).position(|l| l.starts_with(' ') && !l.trim().is_empty()) {
        out.push(format!("Line {} is indented: start continuation lines in the first column.", n + 2));
    }
    if doc.lines().any(|l| l.ends_with(' ')) {
        out.push("Remove the spaces at the ends of lines.".to_string());
    }
    if subject != Subject::Other {
        let word = first.split(|c: char| !c.is_alphanumeric() && c != '-').next().unwrap_or("");
        let lower = word.to_lowercase();
        let stem = lower.strip_suffix("es").filter(|s| VERBS.contains(s)).or_else(|| lower.strip_suffix('s'));
        if let Some(stem) = stem.filter(|s| VERBS.contains(s)) {
            let imperative = format!("{}{}", stem[..1].to_uppercase(), &stem[1..]);
            out.push(format!("Use the imperative: \"{imperative}\", not \"{word}\"."));
        }
    }
    if subject == Subject::Procedure {
        for p in params.iter().filter(|p| !p.starts_with(['_', '%'])) {
            if !mentions(doc, &p.to_uppercase()) {
                out.push(format!("Name the parameter {} in the docstring.", p.to_uppercase()));
            }
        }
    }
    if let Some(key) = literal_key(doc) {
        out.push(format!("Write the key {key} as \\\\[command], so that help shows the user's key."));
    }
    out
}

/// Whether `doc` has `word` as a word of its own, not part of a longer
/// identifier ("LIST" is in "of LIST." but not in "LISTS").
fn mentions(doc: &str, word: &str) -> bool {
    let part = |c: char| c.is_alphanumeric() || "-_?!*<>=/+".contains(c);
    doc.match_indices(word).any(|(i, _)| {
        let before = doc[..i].chars().next_back();
        let after = doc[i + word.len()..].chars().next();
        !before.is_some_and(part) && !after.is_some_and(|c| part(c) && !"?!".contains(c))
    })
}

/// The commands `doc` refers to as `\\[command]`, outside backquotes, for
/// help to show their keys and for checks that they exist.
pub fn key_references(doc: &str) -> Vec<&str> {
    doc.match_indices("\\[")
        .filter(|(i, _)| doc[..*i].matches('`').count().is_multiple_of(2))
        .filter_map(|(i, _)| doc[i + 2..].split_once(']').map(|(name, _)| name))
        .collect()
}

/// A key written as Emacs names it ("C-x", "M-f", "s-v") outside
/// backquotes and outside `\\[...]`, if `doc` has one.
fn literal_key(doc: &str) -> Option<String> {
    let mut quoted = false;
    let mut words = Vec::new();
    let mut word = String::new();
    for c in doc.chars() {
        if c == '`' {
            quoted = !quoted;
            word.clear();
        } else if quoted {
        } else if c.is_whitespace() || "(),;\"'".contains(c) {
            words.push(std::mem::take(&mut word));
        } else {
            word.push(c);
        }
    }
    words.push(word);
    words.into_iter().find(|w| {
        let w = w.trim_end_matches(['.', ':']);
        let mut key = w;
        while let Some(rest) = ["C-", "M-", "s-", "S-", "H-"].iter().find_map(|m| key.strip_prefix(m)) {
            key = rest;
        }
        key.len() < w.len()
            && (key.chars().count() == 1
                || (key.starts_with('<') && key.ends_with('>'))
                || ["RET", "SPC", "TAB", "DEL", "ESC"].contains(&key))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(doc: &str, subject: Subject, params: &[&str]) -> Vec<String> {
        problems(doc, subject, &param_names(params))
    }

    #[test]
    fn a_good_docstring_has_no_problems() {
        let doc = "Return the elements of LIST for which PRED holds.\nThey keep their order; `remove` keeps the others.";
        assert_eq!(check(doc, Subject::Procedure, &["pred", "list"]), Vec::<String>::new());
    }

    #[test]
    fn parameters_as_written() {
        assert_eq!(param_names(&["name", "#:greeting", "(greeting \"hi\")", "[x 1]", ". rest"]), ["name", "greeting", "x", "rest"]);
    }

    #[test]
    fn each_rule() {
        let one = |doc: &str, subject, params: &[&str]| {
            let p = check(doc, subject, params);
            assert_eq!(p.len(), 1, "{doc:?}: {p:?}");
            p[0].clone()
        };
        assert!(one("return X.", Subject::Procedure, &["x"]).contains("capital"));
        assert!(one("Return X", Subject::Procedure, &["x"]).contains("period"));
        assert!(one("Returns X.", Subject::Procedure, &["x"]).contains("\"Return\""));
        assert!(one("Moves the caret.", Subject::Command, &["s", "n"]).contains("\"Move\""));
        assert!(one("Return the length.", Subject::Procedure, &["list"]).contains("LIST"));
        assert!(one("Return the LISTS.", Subject::Procedure, &["list"]).contains("LIST"));
        assert!(one("Save it.\n  More.", Subject::Command, &[]).contains("indented"));
        assert!(one("Save it; C-x C-s does it too.", Subject::Command, &[]).contains("C-x"));
        assert!(one(&format!("Save {}.", "it ".repeat(30)), Subject::Command, &[]).contains("first line"));
        assert!(one(" Save it.", Subject::Command, &[]).contains("whitespace"));
    }

    #[test]
    fn every_special_form_is_documented() {
        use crate::compiler::{SPECIAL_FORM_DOCS, SPECIAL_FORMS};
        let documented: Vec<&str> = SPECIAL_FORM_DOCS.iter().map(|(n, ..)| *n).collect();
        assert_eq!(documented, SPECIAL_FORMS);
        for (name, _, doc) in SPECIAL_FORM_DOCS {
            assert_eq!(problems(doc, Subject::Other, &[]), Vec::<String>::new(), "{name}");
        }
    }

    #[test]
    fn keys_in_backquotes_and_references_are_fine() {
        assert_eq!(check("Read a key such as `C-x`; \\[save-buffer] saves.", Subject::Other, &[]), Vec::<String>::new());
        assert_eq!(key_references("Type \\[save-buffer], then \\[quit], not `\\[x]`."), ["save-buffer", "quit"]);
        assert_eq!(check("Return #t if OBJ is a pair? or not.", Subject::Procedure, &["obj"]), Vec::<String>::new());
        assert_eq!(check("Use X-ray and C-like code.", Subject::Other, &[]), Vec::<String>::new());
    }
}
