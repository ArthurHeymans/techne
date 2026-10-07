//! Source text to S-expressions: R7RS's external representations (7.1.2),
//! plus `[`/`]` as parentheses and `#:name` keywords.

use std::{cell::RefCell, rc::Rc};

use rustc_hash::FxHashMap;

use crate::num::{self, N, Parsed};

#[derive(Default)]
pub struct Symbols {
    names: Vec<Rc<str>>,
    ids: FxHashMap<Rc<str>, u32>,
}

thread_local! {
    static SYMBOLS: RefCell<Symbols> = RefCell::new(Symbols::default());
}

pub fn intern(name: &str) -> u32 {
    SYMBOLS.with(|s| {
        let mut s = s.borrow_mut();
        if let Some(&id) = s.ids.get(name) {
            return id;
        }
        let id = s.names.len() as u32;
        let name: Rc<str> = name.into();
        s.names.push(name.clone());
        s.ids.insert(name, id);
        id
    })
}

pub fn symbol_name(id: u32) -> Rc<str> {
    SYMBOLS.with(|s| s.borrow().names[id as usize].clone())
}

/// A renamed identifier introduced by a macro template (Clinger-Rees style).
/// Bound by a binding form in the expansion, it is a fresh variable; free, it
/// means `orig` as seen from the macro definition: lexical scopes below
/// `env_depth` of the defining compilation, then `module`'s globals.
#[derive(Clone, Copy, Debug)]
pub struct Alias {
    pub orig: u32,
    pub env_depth: usize,
    pub module: u32,
}

thread_local! {
    static ALIASES: RefCell<(FxHashMap<u32, Alias>, u64)> = RefCell::new((FxHashMap::default(), 0));
}

pub fn make_alias(orig: u32, env_depth: usize, module: u32) -> u32 {
    let n = ALIASES.with(|a| {
        let mut a = a.borrow_mut();
        a.1 += 1;
        a.1
    });
    let id = intern(&format!("{}\u{1f}{n}", symbol_name(orig)));
    ALIASES.with(|a| a.borrow_mut().0.insert(id, Alias { orig, env_depth, module }));
    id
}

pub fn alias(sym: u32) -> Option<Alias> {
    ALIASES.with(|a| a.borrow().0.get(&sym).copied())
}

/// The user-visible symbol behind a chain of aliases.
pub fn strip(mut sym: u32) -> u32 {
    while let Some(a) = alias(sym) {
        sym = a.orig;
    }
    sym
}

/// Remove all aliases from quoted data.
pub fn strip_sexp(s: &Sexp) -> Sexp {
    match s {
        Sexp::Sym(id) => Sexp::Sym(strip(*id)),
        Sexp::List(items, tail, pos) => {
            Sexp::List(items.iter().map(strip_sexp).collect(), tail.as_ref().map(|t| Box::new(strip_sexp(t))), *pos)
        }
        Sexp::Vector(items) => Sexp::Vector(items.iter().map(strip_sexp).collect()),
        Sexp::Labeled(n, d) => Sexp::Labeled(*n, Box::new(strip_sexp(d))),
        other => other.clone(),
    }
}

/// Byte offset of a list's opening parenthesis in its source file.
pub type Pos = u32;
pub const NO_POS: Pos = u32::MAX;

#[derive(Clone, Debug, PartialEq)]
pub enum Sexp {
    Int(i64),
    /// An integer literal outside `i64`.
    BigInt(Rc<num_bigint::BigInt>),
    Float(f64),
    Bool(bool),
    Char(char),
    Str(Rc<str>),
    Sym(u32),
    /// `#:name`, holding the symbol id of `name`.
    Keyword(u32),
    /// Proper list when the tail is `None`.
    List(Vec<Sexp>, Option<Box<Sexp>>, Pos),
    Vector(Vec<Sexp>),
    /// `#n=datum`: a datum other parts of the same datum refer to.
    Labeled(u32, Box<Sexp>),
    /// `#n#`: the datum labeled `n`.
    LabelRef(u32),
}

impl Sexp {
    pub fn keyword(&self) -> Option<u32> {
        match self {
            Sexp::Keyword(k) => Some(*k),
            _ => None,
        }
    }
    pub fn sym(&self) -> Option<u32> {
        match self {
            Sexp::Sym(s) => Some(*s),
            _ => None,
        }
    }
    pub fn list(&self) -> Option<&[Sexp]> {
        match self {
            Sexp::List(items, None, _) => Some(items),
            _ => None,
        }
    }
    /// Proper list without a source position (synthesised code).
    pub fn list_of(items: Vec<Sexp>) -> Sexp {
        Sexp::List(items, None, NO_POS)
    }
    pub fn pos(&self) -> Pos {
        match self {
            Sexp::List(_, _, p) => *p,
            _ => NO_POS,
        }
    }
    /// Symbol named `name`, ignoring macro renaming. Only for syntax that is
    /// not a binding (e.g. `else`, `=>`, `...`); special forms are resolved
    /// through the compiler's environment instead.
    pub fn is_sym(&self, name: &str) -> bool {
        self.sym().is_some_and(|s| *symbol_name(strip(s)) == *name)
    }
}

/// Render as source text (aliases shown by their original names).
pub fn display_sexp(s: &Sexp) -> String {
    match s {
        Sexp::Int(i) => i.to_string(),
        Sexp::Float(f) => float_repr(*f),
        Sexp::BigInt(b) => b.to_string(),
        Sexp::Bool(b) => (if *b { "#t" } else { "#f" }).into(),
        Sexp::Char(c) => char_repr(*c),
        Sexp::Str(s) => string_repr(s),
        Sexp::Sym(id) => symbol_repr(&symbol_name(strip(*id))),
        Sexp::Keyword(id) => format!("#:{}", symbol_name(*id)),
        Sexp::List(items, tail, _) => {
            let mut out: Vec<String> = items.iter().map(display_sexp).collect();
            if let Some(t) = tail {
                out.push(".".into());
                out.push(display_sexp(t));
            }
            format!("({})", out.join(" "))
        }
        Sexp::Vector(items) => format!("#({})", items.iter().map(display_sexp).collect::<Vec<_>>().join(" ")),
        Sexp::Labeled(n, d) => format!("#{n}={}", display_sexp(d)),
        Sexp::LabelRef(n) => format!("#{n}#"),
    }
}

// ----- reading -----

/// A reader error with the byte offset it refers to (when known).
#[derive(Debug)]
pub struct ReadError {
    pub message: String,
    pub pos: Option<u32>,
}

/// Whether reading stopped because the text ended inside a datum (a REPL
/// then asks for more).
pub const INCOMPLETE: &str = "unexpected end of input";

pub fn read(source: &str) -> Result<Vec<Sexp>, String> {
    read_located(source).map_err(|e| e.message)
}

/// "line:col" (1-based) of byte offset `pos` in `source`.
pub fn line_col(source: &str, pos: u32) -> (usize, usize) {
    let before = &source[..(pos as usize).min(source.len())];
    let line = before.matches('\n').count() + 1;
    let col = before.len() - before.rfind('\n').map_or(0, |i| i + 1) + 1;
    (line, col)
}

/// Every datum of a file. A first line starting with `#!/` is skipped.
pub fn read_located(source: &str) -> Result<Vec<Sexp>, ReadError> {
    Ok(read_syntax(source)?.iter().map(Syntax::to_sexp).collect())
}

/// Every datum of a file with the source span of each part (for tools).
pub fn read_syntax(source: &str) -> Result<Vec<Syntax>, ReadError> {
    let start = if source.starts_with("#!/") { source.find('\n').unwrap_or(source.len()) } else { 0 };
    let mut r = Reader::new(source, start);
    std::iter::from_fn(|| r.next().transpose()).collect()
}

/// The next datum of `source` and the bytes read, or `None` at its end.
/// `#!fold-case` holds until the end of this datum.
pub fn read_next(source: &str) -> Result<Option<(Sexp, usize)>, String> {
    let mut r = Reader::new(source, 0);
    let datum = r.next().map_err(|e| e.message)?;
    Ok(datum.map(|d| (d.to_sexp(), r.pos)))
}

/// A datum as read, with the byte span of its text and of each part: what
/// tools (the language server) work on. `to_sexp` gives what the VM reads.
#[derive(Clone, Debug)]
pub struct Syntax {
    pub kind: SyntaxKind,
    pub span: (u32, u32),
}

#[derive(Clone, Debug)]
pub enum SyntaxKind {
    /// Anything but the compound data below.
    Atom(Sexp),
    /// Proper when the tail is `None`. `'x` is `(quote x)`, spanning `'x`.
    List(Vec<Syntax>, Option<Box<Syntax>>),
    Vector(Vec<Syntax>),
    Labeled(u32, Box<Syntax>),
}

impl Syntax {
    fn new(kind: SyntaxKind, start: usize, end: usize) -> Syntax {
        Syntax { kind, span: (start as u32, end as u32) }
    }

    pub fn to_sexp(&self) -> Sexp {
        match &self.kind {
            SyntaxKind::Atom(s) => s.clone(),
            SyntaxKind::List(items, tail) => {
                Sexp::List(items.iter().map(Syntax::to_sexp).collect(), tail.as_ref().map(|t| Box::new(t.to_sexp())), self.span.0)
            }
            SyntaxKind::Vector(items) => Sexp::Vector(items.iter().map(Syntax::to_sexp).collect()),
            SyntaxKind::Labeled(n, d) => Sexp::Labeled(*n, Box::new(d.to_sexp())),
        }
    }

    /// The identifier this is, if it is one.
    pub fn sym(&self) -> Option<u32> {
        match &self.kind {
            SyntaxKind::Atom(Sexp::Sym(s)) => Some(*s),
            _ => None,
        }
    }

    /// The items of a proper list.
    pub fn list(&self) -> Option<&[Syntax]> {
        match &self.kind {
            SyntaxKind::List(items, None) => Some(items),
            _ => None,
        }
    }
}

struct Reader<'a> {
    src: &'a str,
    pos: usize,
    fold_case: bool,
    /// Datum labels defined so far (`#n=`), which `#n#` may refer to.
    labels: Vec<u32>,
}

/// Where an identifier or number ends (R7RS's delimiters, brackets, and
/// the quote characters, so that `'a'b` is two data).
pub fn is_delimiter(c: char) -> bool {
    c.is_whitespace() || "()[]\";|'`,".contains(c)
}

const CHAR_NAMES: &[(&str, char)] = &[
    ("alarm", '\x07'),
    ("backspace", '\x08'),
    ("delete", '\x7f'),
    ("escape", '\x1b'),
    ("newline", '\n'),
    ("null", '\0'),
    ("return", '\r'),
    ("space", ' '),
    ("tab", '\t'),
];

enum Token {
    Datum(Syntax),
    Close(char),
    Dot,
}

impl<'a> Reader<'a> {
    fn new(src: &'a str, pos: usize) -> Reader<'a> {
        Reader { src, pos, fold_case: false, labels: Vec::new() }
    }

    fn err<T>(&self, message: impl Into<String>, pos: usize) -> Result<T, ReadError> {
        Err(ReadError { message: message.into(), pos: Some(pos as u32) })
    }

    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn rest(&self) -> &'a str {
        &self.src[self.pos..]
    }

    /// The next datum, or `None` at the end of the text.
    fn next(&mut self) -> Result<Option<Syntax>, ReadError> {
        let start = self.pos;
        match self.token()? {
            None => Ok(None),
            Some(Token::Datum(d)) => Ok(Some(d)),
            Some(Token::Close(c)) => self.err(format!("unexpected {c}"), start),
            Some(Token::Dot) => self.err("unexpected .", start),
        }
    }

    /// A datum that must be there.
    fn datum(&mut self) -> Result<Syntax, ReadError> {
        let start = self.pos;
        match self.next()? {
            Some(d) => Ok(d),
            None => self.err(INCOMPLETE, start),
        }
    }

    /// Skips whitespace, comments, `#;` datum comments and directives.
    fn atmosphere(&mut self) -> Result<(), ReadError> {
        loop {
            let rest = self.rest();
            let Some(c) = rest.chars().next() else { return Ok(()) };
            if c.is_whitespace() {
                self.pos += c.len_utf8();
            } else if c == ';' {
                self.pos += rest.find('\n').unwrap_or(rest.len());
            } else if rest.starts_with("#|") {
                self.block_comment()?;
            } else if rest.starts_with("#;") {
                self.pos += 2;
                self.datum()?;
            } else if let Some(directive) = rest.strip_prefix("#!") {
                let name: String = directive.chars().take_while(|&c| !is_delimiter(c)).collect();
                match name.as_str() {
                    "fold-case" => self.fold_case = true,
                    "no-fold-case" => self.fold_case = false,
                    _ => return self.err(format!("unknown directive #!{name}"), self.pos),
                }
                self.pos += 2 + name.len();
            } else {
                return Ok(());
            }
        }
    }

    fn block_comment(&mut self) -> Result<(), ReadError> {
        let start = self.pos;
        self.pos += 2;
        let mut depth = 1;
        while depth > 0 {
            let rest = self.rest();
            if rest.starts_with("|#") {
                depth -= 1;
                self.pos += 2;
            } else if rest.starts_with("#|") {
                depth += 1;
                self.pos += 2;
            } else if self.bump().is_none() {
                return self.err(format!("{INCOMPLETE} in a #| comment"), start);
            }
        }
        Ok(())
    }

    fn token(&mut self) -> Result<Option<Token>, ReadError> {
        self.atmosphere()?;
        let start = self.pos;
        let Some(c) = self.bump() else { return Ok(None) };
        let atom = |r: &Self, s: Sexp| Syntax::new(SyntaxKind::Atom(s), start, r.pos);
        let datum = match c {
            '(' | '[' => self.list(if c == '(' { ')' } else { ']' }, start)?,
            ')' | ']' => return Ok(Some(Token::Close(c))),
            '\'' => self.abbreviation("quote", start)?,
            '`' => self.abbreviation("quasiquote", start)?,
            ',' if self.peek() == Some('@') => {
                self.pos += 1;
                self.abbreviation("unquote-splicing", start)?
            }
            ',' => self.abbreviation("unquote", start)?,
            '"' => {
                let s = Sexp::Str(self.delimited('"', start)?.into());
                atom(self, s)
            }
            '|' => {
                let s = Sexp::Sym(intern(&self.delimited('|', start)?));
                atom(self, s)
            }
            '#' => self.hash(start)?,
            _ => {
                self.pos = start;
                let text = self.atom_text();
                if text == "." {
                    return Ok(Some(Token::Dot));
                }
                let s = self.atom(text, start)?;
                atom(self, s)
            }
        };
        Ok(Some(Token::Datum(datum)))
    }

    fn atom_text(&mut self) -> &'a str {
        let rest = self.rest();
        let end = rest.find(is_delimiter).unwrap_or(rest.len());
        self.pos += end;
        &rest[..end]
    }

    /// A number, or else an identifier.
    fn atom(&self, text: &str, start: usize) -> Result<Sexp, ReadError> {
        match num::parse(text, 10) {
            Parsed::Number(n) => Ok(number(n)),
            Parsed::Unsupported(why) => self.err(format!("{text}: {why}"), start),
            Parsed::No if self.fold_case => Ok(Sexp::Sym(intern(&text.to_lowercase()))),
            Parsed::No => Ok(Sexp::Sym(intern(text))),
        }
    }

    fn abbreviation(&mut self, name: &str, start: usize) -> Result<Syntax, ReadError> {
        let head = Syntax::new(SyntaxKind::Atom(Sexp::Sym(intern(name))), start, self.pos);
        let d = self.datum()?;
        Ok(Syntax::new(SyntaxKind::List(vec![head, d], None), start, self.pos))
    }

    fn list(&mut self, close: char, start: usize) -> Result<Syntax, ReadError> {
        let mut items = Vec::new();
        loop {
            let at = self.pos;
            match self.token()? {
                None => return self.err(format!("{INCOMPLETE}: the list here is not closed"), start),
                Some(Token::Datum(d)) => items.push(d),
                Some(Token::Close(c)) if c == close => return Ok(Syntax::new(SyntaxKind::List(items, None), start, self.pos)),
                Some(Token::Close(c)) => {
                    return self.err(format!("{c} closes a list opened with {}", if close == ')' { '(' } else { '[' }), at);
                }
                Some(Token::Dot) if items.is_empty() => return self.err("nothing before . in a list", at),
                Some(Token::Dot) => {
                    let tail = self.datum()?;
                    let end = self.pos;
                    return match self.token()? {
                        Some(Token::Close(c)) if c == close => {
                            Ok(Syntax::new(SyntaxKind::List(items, Some(Box::new(tail))), start, self.pos))
                        }
                        None => self.err(INCOMPLETE, start),
                        _ => self.err("more than one datum after . in a list", end),
                    };
                }
            }
        }
    }

    /// The text of a string or `|identifier|` up to its closing `close`.
    fn delimited(&mut self, close: char, start: usize) -> Result<String, ReadError> {
        let mut out = String::new();
        loop {
            match self.bump() {
                None => return self.err(INCOMPLETE, start),
                Some(c) if c == close => return Ok(out),
                Some('\\') => {
                    let at = self.pos - 1;
                    match self.bump() {
                        None => return self.err(INCOMPLETE, start),
                        Some('a') => out.push('\x07'),
                        Some('b') => out.push('\x08'),
                        Some('t') => out.push('\t'),
                        Some('n') => out.push('\n'),
                        Some('r') => out.push('\r'),
                        Some(c @ ('"' | '\\' | '|')) => out.push(c),
                        Some('x' | 'X') => {
                            let rest = self.rest();
                            let Some(end) = rest.find(';') else { return self.err("\\x escape without ;", at) };
                            match u32::from_str_radix(&rest[..end], 16).ok().and_then(char::from_u32) {
                                Some(c) => out.push(c),
                                None => return self.err(format!("bad escape \\x{};", &rest[..end]), at),
                            }
                            self.pos += end + 1;
                        }
                        // A line continuation: \ then spaces, a newline, spaces.
                        Some(c) if c == ' ' || c == '\t' || c == '\n' || c == '\r' => {
                            let rest = &self.src[at + 1..];
                            let spaces = |s: &str| s.find(|c| c != ' ' && c != '\t').unwrap_or(s.len());
                            let mut i = spaces(rest);
                            match rest[i..].strip_prefix("\r\n").or_else(|| rest[i..].strip_prefix('\n')) {
                                Some(after) => i = rest.len() - after.len(),
                                None => return self.err("\\ followed by spaces but no newline", at),
                            }
                            i += spaces(&rest[i..]);
                            self.pos = at + 1 + i;
                        }
                        Some(c) => return self.err(format!("unknown escape \\{c}"), at),
                    }
                }
                Some(c) => out.push(c),
            }
        }
    }

    /// What follows `#`.
    fn hash(&mut self, start: usize) -> Result<Syntax, ReadError> {
        let Some(c) = self.peek() else { return self.err(INCOMPLETE, start) };
        let atom = |r: &Self, s: Sexp| Ok(Syntax::new(SyntaxKind::Atom(s), start, r.pos));
        match c {
            '(' => {
                self.pos += 1;
                match self.list(')', start)?.kind {
                    SyntaxKind::List(items, None) => Ok(Syntax::new(SyntaxKind::Vector(items), start, self.pos)),
                    _ => self.err("a vector cannot be dotted", start),
                }
            }
            '\\' => {
                self.pos += 1;
                let c = self.character(start)?;
                atom(self, c)
            }
            ':' => {
                self.pos += 1;
                let k = Sexp::Keyword(intern(self.atom_text()));
                atom(self, k)
            }
            '0'..='9' => {
                let digits = self.rest().find(|c: char| !c.is_ascii_digit()).unwrap_or(self.rest().len());
                let n: u32 = self.rest()[..digits].parse().or_else(|_| self.err("datum label too large", start))?;
                self.pos += digits;
                match self.bump() {
                    Some('=') => {
                        self.labels.push(n);
                        let d = self.datum()?;
                        Ok(Syntax::new(SyntaxKind::Labeled(n, Box::new(d)), start, self.pos))
                    }
                    Some('#') if self.labels.contains(&n) => atom(self, Sexp::LabelRef(n)),
                    Some('#') => self.err(format!("#{n}# refers to no datum label #{n}="), start),
                    _ => self.err("bad datum label", start),
                }
            }
            _ => {
                let text = self.atom_text();
                let lower = text.to_ascii_lowercase();
                match lower.as_str() {
                    "t" | "true" => atom(self, Sexp::Bool(true)),
                    "f" | "false" => atom(self, Sexp::Bool(false)),
                    "u8" if self.peek() == Some('(') => self.err("bytevectors are not supported yet", start),
                    _ => match num::parse(&self.src[start..self.pos], 10) {
                        Parsed::Number(n) => atom(self, number(n)),
                        Parsed::Unsupported(why) => self.err(format!("#{text}: {why}"), start),
                        Parsed::No => self.err(format!("unknown syntax #{text}"), start),
                    },
                }
            }
        }
    }

    /// After `#\`: one character, its name or `x` and its hex code.
    fn character(&mut self, start: usize) -> Result<Sexp, ReadError> {
        let Some(first) = self.bump() else { return self.err(INCOMPLETE, start) };
        let rest = self.rest();
        let more = rest.find(is_delimiter).unwrap_or(rest.len());
        if more == 0 {
            return Ok(Sexp::Char(first));
        }
        self.pos += more;
        let name = &self.src[start + 2..self.pos];
        let folded = if self.fold_case { name.to_lowercase() } else { name.to_owned() };
        if let Some((_, c)) = CHAR_NAMES.iter().find(|(n, _)| *n == folded) {
            return Ok(Sexp::Char(*c));
        }
        if let Some(hex) = name.strip_prefix(['x', 'X'])
            && let Some(c) = u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
        {
            return Ok(Sexp::Char(c));
        }
        self.err(format!("unknown character #\\{name}"), start)
    }
}

fn number(n: N) -> Sexp {
    match n {
        N::I(i) => Sexp::Int(i),
        N::B(b) => Sexp::BigInt(Rc::new(b)),
        N::F(f) => Sexp::Float(f),
    }
}

// ----- writing: text that reads back as the same datum -----

/// `write`'s text of a character.
pub fn char_repr(c: char) -> String {
    match CHAR_NAMES.iter().find(|(_, ch)| *ch == c) {
        Some((name, _)) => format!("#\\{name}"),
        None if c.is_control() => format!("#\\x{:x}", c as u32),
        None => format!("#\\{c}"),
    }
}

/// `write`'s text of a string or, between `|`, of an identifier.
fn escaped(s: &str, quote: char) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\x07' => out.push_str("\\a"),
            '\x08' => out.push_str("\\b"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if c.is_control() => out.push_str(&format!("\\x{:x};", c as u32)),
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

pub fn string_repr(s: &str) -> String {
    escaped(s, '"')
}

/// An identifier as written: bare when it reads back as itself (and does
/// not look like the start of a number to other readers), else between `|`.
pub fn symbol_repr(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    let numeric_start = name.starts_with(|c: char| c.is_ascii_digit())
        || ["+inf.0", "-inf.0", "+nan.0", "-nan.0"].iter().any(|p| lower.starts_with(p))
        || matches!(
            name.as_bytes(),
            [b'+' | b'-', b'0'..=b'9', ..] | [b'+' | b'-' | b'.', b'.', b'0'..=b'9', ..] | [b'.', b'0'..=b'9', ..]
        );
    let bare = !name.is_empty()
        && name != "."
        && !name.starts_with('#')
        && !numeric_start
        && !name.contains(|c: char| is_delimiter(c) || c.is_control() || c == '\\')
        && num::parse(name, 10) == Parsed::No;
    if bare { name.to_owned() } else { escaped(name, '|') }
}

/// A float as written: the shortest digits that read back as it, always
/// with a point or an exponent, and an exponent outside 1e-6..1e21.
pub fn float_repr(f: f64) -> String {
    if f.is_nan() {
        return "+nan.0".into();
    }
    if f.is_infinite() {
        return (if f > 0.0 { "+inf.0" } else { "-inf.0" }).into();
    }
    let a = f.abs();
    if a != 0.0 && !(1e-6..1e21).contains(&a) {
        let s = format!("{f:e}");
        let (mantissa, exponent) = s.split_once('e').expect("exponent");
        let mantissa = if mantissa.contains('.') { mantissa.to_owned() } else { format!("{mantissa}.0") };
        let exponent = if exponent.starts_with('-') { exponent.to_owned() } else { format!("+{exponent}") };
        return format!("{mantissa}e{exponent}");
    }
    let s = f.to_string();
    if s.contains('.') { s } else { s + ".0" }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(s: &str) -> Sexp {
        read(s).unwrap().pop().unwrap()
    }

    #[test]
    fn data_round_trip_through_their_text() {
        for text in [
            "(a b . c)",
            "#(1 \"two\" #\\x)",
            "|a b|",
            "||",
            "|1|",
            "\"a\\nb\\x0;\"",
            "#\\space",
            "#\\delete",
            "#\\x1",
            "1.0e+21",
            "1.0e-7",
            "0.1",
            "-0.0",
            "123456789012345678901234567890",
        ] {
            assert_eq!(display_sexp(&one(text)), text);
        }
    }

    #[test]
    fn comments_directives_and_labels() {
        assert_eq!(read("#| a #| nested |# |# 1 #;(2 3) ; four\n").unwrap(), vec![Sexp::Int(1)]);
        assert_eq!(display_sexp(&one("(a . b #;c)")), "(a . b)");
        assert_eq!(display_sexp(&one("#!fold-case (ABC #\\SPACE \"X\")")), "(abc #\\space \"X\")");
        assert_eq!(display_sexp(&one("#0=(1 . #0#)")), "#0=(1 . #0#)");
        assert!(read("(#1# #1=a)").is_err());
        assert_eq!(read("#false\"8\"").unwrap(), vec![Sexp::Bool(false), Sexp::Str("8".into())]);
    }

    #[test]
    fn spans_cover_every_datum() {
        let src = "(f 'x #(1 \"s\") . y)";
        let d = &read_syntax(src).unwrap()[0];
        assert_eq!(d.span, (0, src.len() as u32));
        let SyntaxKind::List(items, Some(tail)) = &d.kind else { panic!() };
        let text = |s: &Syntax| &src[s.span.0 as usize..s.span.1 as usize];
        assert_eq!(items.iter().map(text).collect::<Vec<_>>(), ["f", "'x", "#(1 \"s\")"]);
        assert_eq!(text(tail), "y");
        assert_eq!(display_sexp(&d.to_sexp()), "(f (quote x) #(1 \"s\") . y)");
    }

    #[test]
    fn numbers() {
        let n = |s| match one(s) {
            Sexp::Int(i) => i as f64,
            Sexp::Float(f) => f,
            other => panic!("{s}: {other:?}"),
        };
        assert_eq!(n("#x10"), 16.0);
        assert_eq!(n("#e1.5e1"), 15.0);
        assert!(matches!(one("#i1"), Sexp::Float(f) if f == 1.0));
        assert!(matches!(one("6/3"), Sexp::Int(2)));
        assert_eq!(n("1/2"), 0.5);
        assert!(n("+NaN.0").is_nan());
        assert_eq!(n("-inf.0"), f64::NEG_INFINITY);
        assert!(matches!(one("+"), Sexp::Sym(_)));
        assert!(matches!(one("1+"), Sexp::Sym(_)));
        assert!(read("1+2i").unwrap_err().contains("complex"));
        assert!(read("#e1/2").unwrap_err().contains("rational"));
    }
}
