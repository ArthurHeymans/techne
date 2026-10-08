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
/// A label `s` refers to (`#n#`) before or outside the datum it labels:
/// one of another outermost datum, such as another `quote`.
pub fn dangling_label(s: &Sexp) -> Option<u32> {
    fn walk(s: &Sexp, defined: &mut Vec<u32>) -> Option<u32> {
        match s {
            Sexp::LabelRef(n) => (!defined.contains(n)).then_some(*n),
            Sexp::Labeled(n, d) => crate::nested(|| {
                defined.push(*n);
                walk(d, defined)
            }),
            Sexp::List(items, tail, _) => crate::nested(|| items.iter().chain(tail.as_deref()).find_map(|i| walk(i, defined))),
            Sexp::Vector(items) => crate::nested(|| items.iter().find_map(|i| walk(i, defined))),
            _ => None,
        }
    }
    walk(s, &mut Vec::new())
}

pub fn strip_sexp(s: &Sexp) -> Sexp {
    match s {
        Sexp::Sym(id) => Sexp::Sym(strip(*id)),
        Sexp::List(items, tail, pos) => {
            crate::nested(|| Sexp::List(items.iter().map(strip_sexp).collect(), tail.as_ref().map(|t| Box::new(strip_sexp(t))), *pos))
        }
        Sexp::Vector(items) => crate::nested(|| Sexp::Vector(items.iter().map(strip_sexp).collect())),
        Sexp::Labeled(n, d) => crate::nested(|| Sexp::Labeled(*n, Box::new(strip_sexp(d)))),
        other => other.clone(),
    }
}

/// Byte offset of a list's opening parenthesis in its source file.
pub type Pos = u32;
pub const NO_POS: Pos = u32::MAX;

#[derive(Debug, PartialEq)]
pub enum Sexp {
    Int(i64),
    /// An integer literal outside `i64`.
    BigInt(Rc<num_bigint::BigInt>),
    /// An exact non-integer, in lowest terms.
    Ratio(Rc<num_rational::BigRational>),
    /// A non-real number: real and imaginary part, both real numbers.
    Complex(Box<Sexp>, Box<Sexp>),
    Float(f64),
    Bool(bool),
    Char(char),
    Str(Rc<str>),
    /// `#u8(...)`.
    Bytes(Rc<[u8]>),
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

/// A list taken apart: items, dotted tail and position.
pub type ListParts = (Vec<Sexp>, Option<Box<Sexp>>, Pos);

/// Drop shallow data normally; drain deep trees iteratively before the stack runs out.
impl Drop for Sexp {
    #[inline]
    fn drop(&mut self) {
        if !matches!(self, Sexp::List(..) | Sexp::Vector(_) | Sexp::Labeled(..) | Sexp::Complex(..))
            || stacker::remaining_stack().is_some_and(|remaining| remaining >= crate::STACK_RED_ZONE)
        {
            return;
        }
        drop_sexp(self);
    }
}

#[cold]
#[inline(never)]
fn drop_sexp(s: &mut Sexp) {
    fn detach(s: &mut Sexp, stack: &mut Vec<Sexp>) {
        let mut take = |x: &mut Sexp| {
            if matches!(x, Sexp::List(..) | Sexp::Vector(_) | Sexp::Labeled(..) | Sexp::Complex(..)) {
                stack.push(std::mem::replace(x, Sexp::Bool(false)));
            }
        };
        match s {
            // Draining prevents the processed node's drop from scanning its children again.
            Sexp::List(items, tail, _) => {
                items.drain(..).for_each(|mut x| take(&mut x));
                if let Some(mut t) = tail.take() {
                    take(&mut t);
                }
            }
            Sexp::Vector(items) => items.drain(..).for_each(|mut x| take(&mut x)),
            Sexp::Labeled(_, d) => take(d),
            Sexp::Complex(re, im) => [re, im].into_iter().for_each(|x| take(x)),
            _ => {}
        }
    }
    let mut stack = Vec::new();
    detach(s, &mut stack);
    while let Some(mut s) = stack.pop() {
        detach(&mut s, &mut stack);
    }
}

/// By hand, to copy deeply nested data without overflowing the stack.
impl Clone for Sexp {
    #[inline]
    fn clone(&self) -> Sexp {
        match self {
            Sexp::Int(i) => Sexp::Int(*i),
            Sexp::BigInt(b) => Sexp::BigInt(b.clone()),
            Sexp::Ratio(r) => Sexp::Ratio(r.clone()),
            Sexp::Complex(re, im) => crate::nested(|| Sexp::Complex(re.clone(), im.clone())),
            Sexp::Float(f) => Sexp::Float(*f),
            Sexp::Bool(b) => Sexp::Bool(*b),
            Sexp::Char(c) => Sexp::Char(*c),
            Sexp::Str(s) => Sexp::Str(s.clone()),
            Sexp::Bytes(b) => Sexp::Bytes(b.clone()),
            Sexp::Sym(s) => Sexp::Sym(*s),
            Sexp::Keyword(k) => Sexp::Keyword(*k),
            Sexp::List(items, tail, pos) => crate::nested(|| Sexp::List(items.clone(), tail.clone(), *pos)),
            Sexp::Vector(items) => crate::nested(|| Sexp::Vector(items.clone())),
            Sexp::Labeled(n, d) => crate::nested(|| Sexp::Labeled(*n, d.clone())),
            Sexp::LabelRef(n) => Sexp::LabelRef(*n),
        }
    }
}

impl Sexp {
    /// The items, tail and position of a list, or the datum itself.
    pub fn into_list(mut self) -> Result<ListParts, Sexp> {
        match &mut self {
            Sexp::List(items, tail, pos) => Ok((std::mem::take(items), tail.take(), *pos)),
            _ => Err(self),
        }
    }

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
    crate::nested(|| display_sexp_step(s))
}

fn display_sexp_step(s: &Sexp) -> String {
    match s {
        Sexp::Int(i) => i.to_string(),
        Sexp::Float(f) => float_repr(*f),
        Sexp::BigInt(b) => b.to_string(),
        Sexp::Ratio(r) => r.to_string(),
        Sexp::Complex(..) => match sexp_number(s) {
            Some(n) => num::to_string_radix(&n, 10),
            None => "#<complex>".into(),
        },
        Sexp::Bool(b) => (if *b { "#t" } else { "#f" }).into(),
        Sexp::Char(c) => char_repr(*c),
        Sexp::Str(s) => string_repr(s),
        Sexp::Bytes(b) => bytes_repr(b),
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

/// "line:col" (1-based) of byte offset `pos` in `source` (or of the start
/// of the character it falls in).
pub fn line_col(source: &str, pos: u32) -> (usize, usize) {
    let before = &source[..source.floor_char_boundary(pos as usize)];
    let line = before.matches('\n').count() + 1;
    let col = before.len() - before.rfind('\n').map_or(0, |i| i + 1) + 1;
    (line, col)
}

/// Every datum of a file. A first line starting with `#!/` is skipped.
pub fn read_located(source: &str) -> Result<Vec<Sexp>, ReadError> {
    read_all(source, false)
}

/// `read_located` as if the file started with `#!fold-case` (`include-ci`).
pub fn read_located_folded(source: &str) -> Result<Vec<Sexp>, ReadError> {
    read_all(source, true)
}

/// Every datum of a file with the source span of each part (for tools).
pub fn read_syntax(source: &str) -> Result<Vec<Syntax>, ReadError> {
    read_all(source, false)
}

fn read_all<D: Build>(source: &str, fold_case: bool) -> Result<Vec<D>, ReadError> {
    let start = if source.starts_with("#!/") { source.find('\n').unwrap_or(source.len()) } else { 0 };
    let mut r = Reader::<D>::new(source, start);
    r.fold_case = fold_case;
    std::iter::from_fn(|| r.top().transpose()).collect()
}

/// The next datum of `source` and the bytes read, or `None` at its end.
/// `#!fold-case` holds until the end of this datum.
pub fn read_next(source: &str) -> Result<Option<(Sexp, usize)>, String> {
    let mut r = Reader::<Sexp>::new(source, 0);
    let datum = r.top().map_err(|e| e.message)?;
    Ok(datum.map(|d| (d, r.pos)))
}

/// What the reader builds from source text: `Sexp`, what the VM reads, or
/// `Syntax`, which also has the byte span of every part.
trait Build: Sized {
    fn atom(s: Sexp, span: (usize, usize)) -> Self;
    fn list(items: Vec<Self>, tail: Option<Self>, span: (usize, usize)) -> Self;
    fn vector(items: Vec<Self>, span: (usize, usize)) -> Self;
    fn labeled(n: u32, d: Self, span: (usize, usize)) -> Self;
}

impl Build for Sexp {
    fn atom(s: Sexp, _: (usize, usize)) -> Sexp {
        s
    }
    /// `(a . (b c))` is `(a b c)`.
    fn list(mut items: Vec<Sexp>, tail: Option<Sexp>, span: (usize, usize)) -> Sexp {
        let tail = match tail.map(Sexp::into_list) {
            Some(Ok((more, more_tail, _))) => {
                items.extend(more);
                more_tail
            }
            Some(Err(t)) => Some(Box::new(t)),
            None => None,
        };
        Sexp::List(items, tail, span.0 as u32)
    }
    fn vector(items: Vec<Sexp>, _: (usize, usize)) -> Sexp {
        Sexp::Vector(items)
    }
    fn labeled(n: u32, d: Sexp, _: (usize, usize)) -> Sexp {
        Sexp::Labeled(n, Box::new(d))
    }
}

impl Build for Syntax {
    fn atom(s: Sexp, (start, end): (usize, usize)) -> Syntax {
        Syntax::new(SyntaxKind::Atom(s), start, end)
    }
    fn list(items: Vec<Syntax>, tail: Option<Syntax>, (start, end): (usize, usize)) -> Syntax {
        Syntax::new(SyntaxKind::List(items, tail.map(Box::new)), start, end)
    }
    fn vector(items: Vec<Syntax>, (start, end): (usize, usize)) -> Syntax {
        Syntax::new(SyntaxKind::Vector(items), start, end)
    }
    fn labeled(n: u32, d: Syntax, (start, end): (usize, usize)) -> Syntax {
        Syntax::new(SyntaxKind::Labeled(n, Box::new(d)), start, end)
    }
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
        crate::nested(|| self.to_sexp_step())
    }

    fn to_sexp_step(&self) -> Sexp {
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

struct Reader<'a, D> {
    src: &'a str,
    pos: usize,
    fold_case: bool,
    /// Datum labels defined so far (`#n=`), which `#n#` may refer to.
    labels: Vec<u32>,
    /// Data being read around the current one.
    depth: usize,
    builds: std::marker::PhantomData<D>,
}

/// How deeply data may nest. Reading, compiling and printing recurse on the
/// host stack, which must not overflow.
pub const MAX_DEPTH: usize = 1000;

/// Where an identifier or number ends (R7RS's delimiters, brackets, and
/// the quote characters, so that `'a'b` is two data).
pub fn is_delimiter(c: char) -> bool {
    c.is_whitespace() || "()[]\";|'`,".contains(c)
}

/// The length of the text before the first delimiter.
fn undelimited(s: &str) -> usize {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b < 0x80 {
            if b.is_ascii_whitespace() || b == 0x0b || matches!(b, b'(' | b')' | b'[' | b']' | b'"' | b';' | b'|' | b'\'' | b'`' | b',') {
                return i;
            }
            i += 1;
        } else {
            let c = s[i..].chars().next().expect("a character");
            if c.is_whitespace() {
                return i;
            }
            i += c.len_utf8();
        }
    }
    i
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

enum Token<D> {
    Datum(D),
    Close(char),
    Dot,
}

impl<'a, D: Build> Reader<'a, D> {
    fn new(src: &'a str, pos: usize) -> Reader<'a, D> {
        Reader { src, pos, fold_case: false, labels: Vec::new(), depth: 0, builds: std::marker::PhantomData }
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

    /// The next outermost datum, or `None` at the end of the text: the
    /// scope of the datum labels it defines.
    fn top(&mut self) -> Result<Option<D>, ReadError> {
        self.labels.clear();
        self.next()
    }

    /// The next datum, or `None` at the end of the text.
    fn next(&mut self) -> Result<Option<D>, ReadError> {
        let start = self.pos;
        match self.token()? {
            None => Ok(None),
            Some(Token::Datum(d)) => Ok(Some(d)),
            Some(Token::Close(c)) => self.err(format!("unexpected {c}"), start),
            Some(Token::Dot) => self.err("unexpected .", start),
        }
    }

    /// A datum that must be there.
    fn datum(&mut self) -> Result<D, ReadError> {
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
                // Labels in a skipped datum are not defined after it.
                let labels = self.labels.len();
                self.datum()?;
                self.labels.truncate(labels);
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

    fn token(&mut self) -> Result<Option<Token<D>>, ReadError> {
        if self.depth >= MAX_DEPTH {
            return self.err(format!("data nested more than {MAX_DEPTH} deep"), self.pos);
        }
        self.depth += 1;
        let token = crate::nested(|| self.token_inner());
        self.depth -= 1;
        token
    }

    fn token_inner(&mut self) -> Result<Option<Token<D>>, ReadError> {
        self.atmosphere()?;
        let start = self.pos;
        let Some(c) = self.bump() else { return Ok(None) };
        let atom = |r: &Self, s: Sexp| D::atom(s, (start, r.pos));
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
        let end = undelimited(rest);
        self.pos += end;
        &rest[..end]
    }

    /// A number, or else an identifier.
    fn atom(&self, text: &str, start: usize) -> Result<Sexp, ReadError> {
        // Numbers (and complex numbers) start with a digit, a sign or a point.
        let parsed = match text.as_bytes().first() {
            Some(b'0'..=b'9' | b'+' | b'-' | b'.') => num::parse(text, 10),
            _ => Parsed::No,
        };
        match parsed {
            Parsed::Number(n) => Ok(number(n)),
            Parsed::Unsupported(why) => self.err(format!("{text}: {why}"), start),
            Parsed::No if self.fold_case => Ok(Sexp::Sym(intern(&text.to_lowercase()))),
            Parsed::No => Ok(Sexp::Sym(intern(text))),
        }
    }

    fn abbreviation(&mut self, name: &str, start: usize) -> Result<D, ReadError> {
        let head = D::atom(Sexp::Sym(intern(name)), (start, self.pos));
        let d = self.datum()?;
        Ok(D::list(vec![head, d], None, (start, self.pos)))
    }

    fn list(&mut self, close: char, start: usize) -> Result<D, ReadError> {
        let (items, tail) = self.items(close, start)?;
        Ok(D::list(items, tail, (start, self.pos)))
    }

    /// The items and the dotted tail of a list up to `close`.
    fn items(&mut self, close: char, start: usize) -> Result<(Vec<D>, Option<D>), ReadError> {
        let mut items = Vec::new();
        loop {
            let at = self.pos;
            match self.token()? {
                None => return self.err(format!("{INCOMPLETE}: the list here is not closed"), start),
                Some(Token::Datum(d)) => items.push(d),
                Some(Token::Close(c)) if c == close => return Ok((items, None)),
                Some(Token::Close(c)) => {
                    return self.err(format!("{c} closes a list opened with {}", if close == ')' { '(' } else { '[' }), at);
                }
                Some(Token::Dot) if items.is_empty() => return self.err("nothing before . in a list", at),
                Some(Token::Dot) => {
                    let tail = self.datum()?;
                    let end = self.pos;
                    return match self.token()? {
                        Some(Token::Close(c)) if c == close => Ok((items, Some(tail))),
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
            // The text up to the next escape or the end, at once.
            let rest = self.rest();
            let plain = rest.find([close, '\\']).unwrap_or(rest.len());
            out.push_str(&rest[..plain]);
            self.pos += plain;
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
    fn hash(&mut self, start: usize) -> Result<D, ReadError> {
        let Some(c) = self.peek() else { return self.err(INCOMPLETE, start) };
        let atom = |r: &Self, s: Sexp| Ok(D::atom(s, (start, r.pos)));
        match c {
            '(' => {
                self.pos += 1;
                match self.items(')', start)? {
                    (items, None) => Ok(D::vector(items, (start, self.pos))),
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
                        Ok(D::labeled(n, d, (start, self.pos)))
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
                    "u8" if self.peek() == Some('(') => {
                        self.pos += 1;
                        let bytes = self.bytes(start)?;
                        atom(self, Sexp::Bytes(bytes.into()))
                    }
                    _ => match num::parse(&self.src[start..self.pos], 10) {
                        Parsed::Number(n) => atom(self, number(n)),
                        Parsed::Unsupported(why) => self.err(format!("#{text}: {why}"), start),
                        Parsed::No => self.err(format!("unknown syntax #{text}"), start),
                    },
                }
            }
        }
    }

    /// After `#u8(`: bytes, exact integers from 0 to 255, up to `)`.
    fn bytes(&mut self, start: usize) -> Result<Vec<u8>, ReadError> {
        let mut bytes = Vec::new();
        loop {
            self.atmosphere()?;
            let at = self.pos;
            match self.peek() {
                None => return self.err(INCOMPLETE, start),
                Some(')') => {
                    self.pos += 1;
                    return Ok(bytes);
                }
                _ => {}
            }
            let text = self.atom_text();
            match num::parse(text, 10) {
                Parsed::Number(N::I(b @ 0..=255)) => bytes.push(b as u8),
                _ if text.is_empty() => return self.err("a bytevector holds only bytes", at),
                _ => return self.err(format!("{text} is not a byte (an exact integer from 0 to 255)"), at),
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

/// The number a numeric datum denotes.
pub fn sexp_number(s: &Sexp) -> Option<N> {
    Some(match s {
        Sexp::Int(i) => N::I(*i),
        Sexp::BigInt(b) => N::B((**b).clone()),
        Sexp::Ratio(r) => N::R(Box::new((**r).clone())),
        Sexp::Float(f) => N::F(*f),
        Sexp::Complex(re, im) => N::C(Box::new((sexp_number(re)?, sexp_number(im)?))),
        _ => return None,
    })
}

pub fn number(n: N) -> Sexp {
    match n {
        N::I(i) => Sexp::Int(i),
        N::B(b) => Sexp::BigInt(Rc::new(b)),
        N::R(r) => Sexp::Ratio(Rc::new(*r)),
        N::F(f) => Sexp::Float(f),
        N::C(c) => {
            let (re, im) = *c;
            Sexp::Complex(Box::new(number(re)), Box::new(number(im)))
        }
    }
}

// ----- writing: text that reads back as the same datum -----

/// `write`'s text of a bytevector: `#u8(1 2 3)`.
pub fn bytes_repr(bytes: &[u8]) -> String {
    format!("#u8({})", bytes.iter().map(u8::to_string).collect::<Vec<_>>().join(" "))
}

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
        // A label's scope is the rest of its outermost datum, outside comments.
        assert!(read("#1=(a) #1#").is_err());
        assert!(read("(#;#1=a #1#)").is_err());
        assert!(read("#;#1=a #1#").is_err());
        assert_eq!(display_sexp(&one("(#1=a #;b #1#)")), "(#1=a #1#)");
        // Within one, a literal may not refer to another literal's labels.
        let quotes = one("(list (quote #1=(a . #1#)) (quote #1#))");
        let quotes = quotes.list().unwrap();
        assert_eq!(dangling_label(&quotes[1]), None);
        assert_eq!(dangling_label(&quotes[2]), Some(1));
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
        assert!(matches!(&one("6/3"), Sexp::Int(2)));
        assert!(matches!(&one("1/2"), Sexp::Ratio(r) if r.to_string() == "1/2"));
        assert!(matches!(&one("#e1/2"), Sexp::Ratio(r) if r.to_string() == "1/2"));
        assert_eq!(n("#i1/2"), 0.5);
        assert!(n("+NaN.0").is_nan());
        assert_eq!(n("-inf.0"), f64::NEG_INFINITY);
        assert!(matches!(&one("+"), Sexp::Sym(_)));
        assert!(matches!(&one("1+"), Sexp::Sym(_)));
        assert!(matches!(&one("1+2i"), Sexp::Complex(re, im) if **re == Sexp::Int(1) && **im == Sexp::Int(2)));
        assert!(matches!(&one("-i"), Sexp::Complex(re, im) if **re == Sexp::Int(0) && **im == Sexp::Int(-1)));
    }
}
