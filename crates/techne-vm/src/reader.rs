//! Source text to S-expressions, using Steel's parser without its lowering.

use std::{cell::RefCell, rc::Rc};

use rustc_hash::FxHashMap;
use steel_parser::{
    ast::ExprKind,
    parser::Parser,
    tokens::{IntLiteral, NumberLiteral, RealLiteral, TokenType},
};

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
        Sexp::Float(f) => format!("{f:?}"),
        Sexp::BigInt(b) => b.to_string(),
        Sexp::Bool(b) => (if *b { "#t" } else { "#f" }).into(),
        Sexp::Char(c) => format!("#\\{c}"),
        Sexp::Str(s) => format!("{s:?}"),
        Sexp::Sym(id) => symbol_name(strip(*id)).to_string(),
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
    }
}

/// Read the first datum of `source`; returns it and the bytes consumed.
pub fn read_one(source: &str) -> Result<(Sexp, usize), String> {
    let mut parser = Parser::new(source, steel_parser::parser::SourceId::none()).without_lowering();
    let first = parser.next().ok_or("unexpected end of input")?.map_err(|e| e.to_string())?;
    Ok((convert(first)?, parser.offset()))
}

pub fn read(source: &str) -> Result<Vec<Sexp>, String> {
    read_located(source).map_err(|e| e.message)
}

/// A reader error with the byte offset it refers to (when known).
#[derive(Debug)]
pub struct ReadError {
    pub message: String,
    pub pos: Option<u32>,
}

pub fn read_located(source: &str) -> Result<Vec<Sexp>, ReadError> {
    // Steel's lexer treats `[`/`]` like parentheses, as R6RS and Racket do.
    let exprs = Parser::parse_without_lowering(source).map_err(|e| ReadError { message: e.to_string(), pos: Some(e.span().start) })?;
    exprs.into_iter().map(convert).collect::<Result<_, _>>().map_err(|message| ReadError { message, pos: None })
}

/// "line:col" (1-based) of byte offset `pos` in `source`.
pub fn line_col(source: &str, pos: u32) -> (usize, usize) {
    let before = &source[..(pos as usize).min(source.len())];
    let line = before.matches('\n').count() + 1;
    let col = before.len() - before.rfind('\n').map_or(0, |i| i + 1) + 1;
    (line, col)
}

fn keyword(name: &str) -> Sexp {
    Sexp::Sym(intern(name))
}

fn convert(e: ExprKind) -> Result<Sexp, String> {
    Ok(match e {
        ExprKind::Atom(a) => match a.syn.ty {
            TokenType::Identifier(s) => keyword(match s.resolve() {
                // Steel's lexer spells `,x` / `,@x` as these identifiers.
                "#%unquote" => "unquote",
                "#%unquote-splicing" => "unquote-splicing",
                "#%quasiquote" => "quasiquote",
                other => other,
            }),
            TokenType::Keyword(s) => Sexp::Keyword(intern(s.resolve().trim_start_matches("#:"))),
            TokenType::BooleanLiteral(b) => Sexp::Bool(b),
            TokenType::CharacterLiteral(c) => Sexp::Char(c),
            TokenType::StringLiteral(s) => Sexp::Str(s.resolve().into()),
            TokenType::Number(n) => number(n.resolve())?,
            TokenType::Define => keyword("define"),
            TokenType::If => keyword("if"),
            TokenType::Let => keyword("let"),
            TokenType::Lambda => keyword("lambda"),
            TokenType::Begin => keyword("begin"),
            TokenType::Set => keyword("set!"),
            TokenType::Quote => keyword("quote"),
            TokenType::QuasiQuote => keyword("quasiquote"),
            TokenType::Unquote => keyword("unquote"),
            TokenType::UnquoteSplice => keyword("unquote-splicing"),
            TokenType::Require => keyword("require"),
            TokenType::Return => keyword("return!"),
            TokenType::DefineSyntax => keyword("define-syntax"),
            TokenType::SyntaxRules => keyword("syntax-rules"),
            TokenType::Ellipses => keyword("..."),
            other => return Err(format!("unsupported token {other:?}")),
        },
        ExprKind::Quote(q) => Sexp::List(vec![keyword("quote"), convert(q.expr)?], None, q.location.span.start),
        ExprKind::List(l) => {
            let improper = l.improper;
            let pos = l.location.start;
            let mut items = l.args.into_iter().map(convert).collect::<Result<Vec<_>, _>>()?;
            if improper {
                let tail = items.pop().ok_or("empty improper list")?;
                Sexp::List(items, Some(Box::new(tail)), pos)
            } else {
                Sexp::List(items, None, pos)
            }
        }
        ExprKind::Vector(v) => Sexp::Vector(v.args.into_iter().map(convert).collect::<Result<_, _>>()?),
        other => return Err(format!("unexpected lowered form {other:?}")),
    })
}

fn number(n: NumberLiteral) -> Result<Sexp, String> {
    match n {
        NumberLiteral::Real(RealLiteral::Int(IntLiteral::Small(i))) => Ok(Sexp::Int(i as i64)),
        NumberLiteral::Real(RealLiteral::Int(IntLiteral::Big(b))) => {
            use num_traits::ToPrimitive;
            Ok(b.to_i64().map_or_else(|| Sexp::BigInt(Rc::new(*b)), Sexp::Int))
        }
        NumberLiteral::Real(RealLiteral::Float(f)) => Ok(Sexp::Float(f.0)),
        NumberLiteral::Real(RealLiteral::Rational(IntLiteral::Small(a), IntLiteral::Small(b))) => Ok(Sexp::Float(a as f64 / b as f64)),
        other => Err(format!("unsupported number literal {other}")),
    }
}
