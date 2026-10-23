//! Source text to S-expressions: R7RS's external representations (7.1.2),
//! plus `[`/`]` as parentheses and `#:name` keywords.
//!
//! Each world has symbol and alias tables of its own (`Symbols`), entered
//! with the world (`alloc::cage::enter`): a symbol id names only what its
//! world interned, so one forged in a world names nothing of another's.
//! A world made from a template starts with a copy of the template's,
//! the ids its code holds kept. They go with the world. Reading with no
//! world entered (the bare reader in tools) uses the thread's own table.

use std::{cell::RefCell, rc::Rc};

use rustc_hash::FxHashMap;

use crate::{
    num::{self, N, Parsed},
    vm::meter::Meter,
};

/// A world's symbols: names by id, ids by name, the aliases its macros
/// made, and the compiler's aliases of root-module names (`compiler::core`).
#[derive(Clone)]
pub struct Symbols {
    names: Vec<Rc<str>>,
    ids: FxHashMap<Rc<str>, u32>,
    aliases: FxHashMap<u32, Alias>,
    /// Aliases made so far, which number their names.
    made: u64,
    core: FxHashMap<u32, u32>,
}

impl Symbols {
    fn intern(&mut self, name: &str) -> u32 {
        if let Some(&id) = self.ids.get(name) {
            return id;
        }
        // Charged to the world entered explicitly, so a flood of symbols
        // is bounded by its memory limit even without the counting
        // allocator (which counts this allocation itself).
        if !cfg!(feature = "counting-allocator") {
            let tag = crate::alloc::current_tag();
            if tag != 0 {
                crate::alloc::charge_account(tag, (SYMBOL_BYTES + name.len()) as isize);
            }
        }
        self.insert(name)
    }

    /// The alias `sym` is, if it is one.
    pub(crate) fn alias(&self, sym: u32) -> Option<Alias> {
        self.aliases.get(&sym).copied()
    }

    fn insert(&mut self, name: &str) -> u32 {
        let rc: Rc<str> = name.into();
        let id = self.names.len() as u32;
        self.names.push(rc.clone());
        self.ids.insert(rc, id);
        id
    }
}

/// Every table starts with the special forms' names, so their ids are
/// the same in every world (`special_form`).
impl Default for Symbols {
    fn default() -> Self {
        let mut s =
            Symbols { names: Vec::new(), ids: FxHashMap::default(), aliases: FxHashMap::default(), made: 0, core: FxHashMap::default() };
        crate::compiler::SPECIAL_FORMS.iter().for_each(|name| {
            s.insert(name);
        });
        s
    }
}

/// The special form symbol `id` names, if it names one: no table
/// lookup, as their ids are fixed.
pub fn special_form(id: u32) -> Option<&'static str> {
    crate::compiler::SPECIAL_FORMS.get(id as usize).copied()
}

thread_local! {
    /// The symbols of no world: what reading outside any world interns.
    static LOOSE: Rc<RefCell<Symbols>> = Rc::default();
}

/// The symbols of the world entered on this thread, or the thread's
/// own when none is.
fn symbols_table() -> Rc<RefCell<Symbols>> {
    // Past the thread's thread-locals (a world dropped by a destructor of
    // one), a table of its own.
    crate::alloc::entered_symbols().or_else(|| LOOSE.try_with(Rc::clone).ok()).unwrap_or_default()
}

/// `f` on the symbols of the world entered on this thread, or the thread's
/// own when none is.
fn with_symbols<R>(f: impl FnOnce(&mut Symbols) -> R) -> R {
    let table = symbols_table();
    let mut table = table.borrow_mut();
    f(&mut table)
}

/// How many symbols the world entered holds.
pub fn interned_symbols() -> usize {
    with_symbols(|s| s.names.len())
}

pub fn intern(name: &str) -> u32 {
    with_symbols(|s| s.intern(name))
}

/// What interning one more new symbol would make the table reallocate,
/// in bytes: its vector and map grow by doubling, all at once, which
/// admitting the symbol alone (`SYMBOL_BYTES`) would not see.
pub fn intern_growth() -> usize {
    with_symbols(|s| {
        let names = if s.names.len() < s.names.capacity() { 0 } else { s.names.capacity().max(4) * 2 * size_of::<Rc<str>>() };
        // A full map's buckets double: a key, a value and a control byte each.
        let map = if s.ids.len() < s.ids.capacity() { 0 } else { (s.ids.capacity().max(4) * 2) * (size_of::<(Rc<str>, u32)>() + 1) };
        names + map
    })
}

/// The symbol named `name`, if there is one.
pub fn lookup(name: &str) -> Option<u32> {
    with_symbols(|s| s.ids.get(name).copied())
}

pub fn symbol_name(id: u32) -> Rc<str> {
    // An id the world never interned (a forged value's) names nothing;
    // say so rather than panic.
    with_symbols(|s| s.names.get(id as usize).cloned()).unwrap_or_else(|| format!("<symbol {id}>").into())
}

/// Whether symbol `id` is named `name`, without copying its name out.
pub fn symbol_is(id: u32, name: &str) -> bool {
    with_symbols(|s| s.names.get(id as usize).is_some_and(|n| &**n == name))
}

/// About what a new symbol holds besides its name, to admit it with.
pub const SYMBOL_BYTES: usize = 64;

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

pub fn make_alias(orig: u32, env_depth: usize, module: u32) -> u32 {
    with_symbols(|s| {
        s.made += 1;
        let text = match s.names.get(orig as usize) {
            Some(name) => format!("{name}\u{1f}{}", s.made),
            None => format!("<symbol {orig}>\u{1f}{}", s.made),
        };
        let id = s.intern(&text);
        s.aliases.insert(id, Alias { orig, env_depth, module });
        id
    })
}

pub fn alias(sym: u32) -> Option<Alias> {
    with_symbols(|s| s.aliases.get(&sym).copied())
}

/// The alias of root-module name `sym` that the compiler refers to it by,
/// made once per world (`compiler::core`).
pub(crate) fn core_alias(sym: u32) -> u32 {
    if let Some(alias) = with_symbols(|s| s.core.get(&sym).copied()) {
        return alias;
    }
    let alias = make_alias(sym, 0, crate::vm::ROOT_MODULE);
    with_symbols(|s| s.core.insert(sym, alias));
    alias
}

/// The user-visible symbol behind a chain of aliases.
pub fn strip(sym: u32) -> u32 {
    with_symbols(|s| {
        let mut sym = sym;
        while let Some(a) = s.aliases.get(&sym) {
            sym = a.orig;
        }
        sym
    })
}

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

/// Whether `s` holds an alias anywhere, in one pass over the table.
fn contains_alias(s: &Sexp) -> bool {
    fn walk(s: &Sexp, aliases: &FxHashMap<u32, Alias>) -> bool {
        match s {
            Sexp::Sym(id) => aliases.contains_key(id),
            Sexp::List(items, tail, _) => crate::nested(|| items.iter().chain(tail.as_deref()).any(|i| walk(i, aliases))),
            Sexp::Vector(items) | Sexp::Dict(items) | Sexp::Set(items) => crate::nested(|| items.iter().any(|i| walk(i, aliases))),
            Sexp::Labeled(_, d) => crate::nested(|| walk(d, aliases)),
            _ => false,
        }
    }
    with_symbols(|t| !t.aliases.is_empty() && walk(s, &t.aliases))
}

/// Remove all aliases from quoted data.
pub fn strip_sexp(s: &Sexp) -> Sexp {
    // The common case, a literal read from source, holds no aliases:
    // cloning it then needs no symbol-table lookup per identifier.
    // Checked once here, however deep `s` goes; the recursion below
    // would otherwise rescan its subtree at every level.
    if contains_alias(s) { strip_sexp_unchecked(s) } else { s.clone() }
}

fn strip_sexp_unchecked(s: &Sexp) -> Sexp {
    match s {
        Sexp::Sym(id) => Sexp::Sym(strip(*id)),
        Sexp::List(items, tail, pos) => crate::nested(|| {
            Sexp::List(items.iter().map(strip_sexp_unchecked).collect(), tail.as_ref().map(|t| Box::new(strip_sexp_unchecked(t))), *pos)
        }),
        Sexp::Vector(items) => crate::nested(|| Sexp::Vector(items.iter().map(strip_sexp_unchecked).collect())),
        Sexp::Dict(items) => crate::nested(|| Sexp::Dict(items.iter().map(strip_sexp_unchecked).collect())),
        Sexp::Set(items) => crate::nested(|| Sexp::Set(items.iter().map(strip_sexp_unchecked).collect())),
        Sexp::Labeled(n, d) => crate::nested(|| Sexp::Labeled(*n, Box::new(strip_sexp_unchecked(d)))),
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
    /// `{key value ...}`: a dict, keys and values in turn.
    Dict(Vec<Sexp>),
    /// `#{obj ...}`: a set.
    Set(Vec<Sexp>),
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
        if !matches!(self, Sexp::List(..) | Sexp::Vector(_) | Sexp::Dict(_) | Sexp::Set(_) | Sexp::Labeled(..) | Sexp::Complex(..))
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
            if matches!(x, Sexp::List(..) | Sexp::Vector(_) | Sexp::Dict(_) | Sexp::Set(_) | Sexp::Labeled(..) | Sexp::Complex(..)) {
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
            Sexp::Vector(items) | Sexp::Dict(items) | Sexp::Set(items) => items.drain(..).for_each(|mut x| take(&mut x)),
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
            Sexp::Dict(items) => crate::nested(|| Sexp::Dict(items.clone())),
            Sexp::Set(items) => crate::nested(|| Sexp::Set(items.clone())),
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
        self.sym().is_some_and(|s| symbol_is(strip(s), name))
    }
}

/// How much source text `display_sexp` shows.
const SHOWN: usize = 4096;

/// Render as source text (aliases shown by their original names), for
/// messages: cut after `SHOWN` bytes, as syntax sharing one string many
/// times would print as far more text than it holds.
pub fn display_sexp(s: &Sexp) -> String {
    let mut out = String::new();
    write_sexp(&mut out, s);
    if out.len() > SHOWN {
        out.truncate(out.floor_char_boundary(SHOWN));
        out.push_str("...");
    }
    out
}

/// `display_sexp` of symbol `id`, sharing its name when that is how it
/// is shown: most are plain names.
pub fn display_symbol(id: u32) -> Rc<str> {
    let name = symbol_name(strip(id));
    if name.len() <= SHOWN && symbol_is_bare(&name) { name } else { display_sexp(&Sexp::Sym(id)).into() }
}

fn write_sexp(out: &mut String, s: &Sexp) {
    if out.len() > SHOWN {
        return;
    }
    // A long atom is shown up to what is left.
    let left = SHOWN + 1 - out.len();
    let big = |bits: u64| bits / 3 > left as u64;
    match s {
        Sexp::Int(i) => out.push_str(&i.to_string()),
        Sexp::Float(f) => out.push_str(&float_repr(*f)),
        Sexp::BigInt(b) if big(b.bits()) => out.push_str("#<integer>"),
        Sexp::BigInt(b) => out.push_str(&b.to_string()),
        Sexp::Ratio(r) if big(r.numer().bits() + r.denom().bits()) => out.push_str("#<ratio>"),
        Sexp::Ratio(r) => out.push_str(&r.to_string()),
        Sexp::Complex(..) => match sexp_number(s) {
            Some(n) if !big(n.bits()) => out.push_str(&num::to_string_radix(&n, 10)),
            _ => out.push_str("#<complex>"),
        },
        Sexp::Bool(b) => out.push_str(if *b { "#t" } else { "#f" }),
        Sexp::Char(c) => out.push_str(&char_repr(*c)),
        Sexp::Str(s) => out.push_str(&string_repr(&s[..s.floor_char_boundary(left)])),
        Sexp::Bytes(b) => out.push_str(&bytes_repr(&b[..b.len().min(left)])),
        Sexp::Sym(id) => out.push_str(&symbol_repr(&symbol_name(strip(*id)))),
        Sexp::Keyword(id) => out.push_str(&format!("#:{}", symbol_name(*id))),
        Sexp::List(items, tail, _) => crate::nested(|| {
            out.push('(');
            for (i, item) in items.iter().enumerate() {
                if out.len() > SHOWN {
                    break;
                }
                if i > 0 {
                    out.push(' ');
                }
                write_sexp(out, item);
            }
            if let Some(t) = tail {
                out.push_str(" . ");
                write_sexp(out, t);
            }
            out.push(')');
        }),
        Sexp::Vector(items) => crate::nested(|| write_items(out, "#(", items, ')')),
        Sexp::Dict(items) => crate::nested(|| write_items(out, "{", items, '}')),
        Sexp::Set(items) => crate::nested(|| write_items(out, "#{", items, '}')),
        Sexp::Labeled(n, d) => {
            out.push_str(&format!("#{n}="));
            write_sexp(out, d);
        }
        Sexp::LabelRef(n) => out.push_str(&format!("#{n}#")),
    }
}

fn write_items(out: &mut String, open: &str, items: &[Sexp], close: char) {
    out.push_str(open);
    for (i, item) in items.iter().enumerate() {
        if out.len() > SHOWN {
            break;
        }
        if i > 0 {
            out.push(' ');
        }
        write_sexp(out, item);
    }
    out.push(close);
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
    read_all(source, false, None)
}

/// `read_located` as if the file started with `#!fold-case` (`include-ci`).
pub fn read_located_folded(source: &str) -> Result<Vec<Sexp>, ReadError> {
    read_all(source, true, None)
}

/// `read_located` (or `read_located_folded`) for a world, metered: what it
/// reads may take far more than the text.
pub(crate) fn read_metered(source: &str, fold_case: bool, meter: &Meter) -> Result<Vec<Sexp>, ReadError> {
    read_all(source, fold_case, Some(meter))
}

/// Every datum of a file with the source span of each part (for tools).
pub fn read_syntax(source: &str) -> Result<Vec<Syntax>, ReadError> {
    read_all(source, false, None)
}

fn read_all<D: Build>(source: &str, fold_case: bool, meter: Option<&Meter>) -> Result<Vec<D>, ReadError> {
    let start = if source.starts_with("#!/") { source.find('\n').unwrap_or(source.len()) } else { 0 };
    let mut r = Reader::<D>::new(source, start);
    r.fold_case = fold_case;
    r.meter = meter;
    std::iter::from_fn(|| r.top().transpose()).collect()
}

/// The next datum of `source` and the bytes read, or `None` at its end.
/// `#!fold-case` holds until the end of this datum.
pub fn read_next(source: &str) -> Result<Option<(Sexp, usize)>, String> {
    read_next_metered(source, None)
}

/// `read_next`, metered for a world (`read_metered`).
pub(crate) fn read_next_metered(source: &str, meter: Option<&Meter>) -> Result<Option<(Sexp, usize)>, String> {
    let mut r = Reader::<Sexp>::new(source, 0);
    r.meter = meter;
    let datum = r.top().map_err(|e| e.message)?;
    // One last look: a datum of many just-under-threshold parts still
    // ends stopped or refused, not merely built. (Turning it into heap
    // values afterwards is `Budget`-admitted allocation as usual.)
    if let Some(m) = meter {
        m.look()?;
    }
    Ok(datum.map(|d| (d, r.pos)))
}

/// What the reader builds from source text: `Sexp`, what the VM reads, or
/// `Syntax`, which also has the byte span of every part.
trait Build: Sized {
    fn atom(s: Sexp, span: (usize, usize)) -> Self;
    fn list(items: Vec<Self>, tail: Option<Self>, span: (usize, usize)) -> Self;
    fn vector(items: Vec<Self>, span: (usize, usize)) -> Self;
    /// `{...}` (a dict) or `#{...}` (a set).
    fn braced(set: bool, items: Vec<Self>, span: (usize, usize)) -> Self;
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
    fn braced(set: bool, items: Vec<Sexp>, _: (usize, usize)) -> Sexp {
        if set { Sexp::Set(items) } else { Sexp::Dict(items) }
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
    fn braced(set: bool, items: Vec<Syntax>, (start, end): (usize, usize)) -> Syntax {
        Syntax::new(if set { SyntaxKind::Set(items) } else { SyntaxKind::Dict(items) }, start, end)
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
    Dict(Vec<Syntax>),
    Set(Vec<Syntax>),
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
            SyntaxKind::Dict(items) => Sexp::Dict(items.iter().map(Syntax::to_sexp).collect()),
            SyntaxKind::Set(items) => Sexp::Set(items.iter().map(Syntax::to_sexp).collect()),
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
    /// Dicts and sets being read around the current datum.
    braces: usize,
    /// Labels whose datum is being read, and those whose datum refers
    /// to one being read.
    open: Vec<u32>,
    unsettled: Vec<u32>,
    meter: Option<&'a Meter>,
    /// The symbols reading interns into, taken once: a read runs in one
    /// world throughout, and interning is per atom.
    symbols: Rc<RefCell<Symbols>>,
    builds: std::marker::PhantomData<D>,
}

/// How deeply data may nest. Reading, compiling and printing recurse on the
/// host stack, which must not overflow.
pub const MAX_DEPTH: usize = 1000;

/// Reading data nested at most this deep needs no stack check: so many
/// levels of the reader's frames fit in `STACK_RED_ZONE`.
const SHALLOW: usize = 16;

/// Where an identifier or number ends (R7RS's delimiters, brackets, and
/// the quote characters, so that `'a'b` is two data).
pub fn is_delimiter(c: char) -> bool {
    c.is_whitespace() || "()[]{}\";|'`,".contains(c)
}

/// The length of the text before the first delimiter.
fn undelimited(s: &str) -> usize {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b < 0x80 {
            if b.is_ascii_whitespace()
                || b == 0x0b
                || matches!(b, b'(' | b')' | b'[' | b']' | b'{' | b'}' | b'"' | b';' | b'|' | b'\'' | b'`' | b',')
            {
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
        Reader {
            src,
            pos,
            fold_case: false,
            labels: Vec::new(),
            depth: 0,
            braces: 0,
            open: Vec::new(),
            unsettled: Vec::new(),
            meter: None,
            symbols: symbols_table(),
            builds: std::marker::PhantomData,
        }
    }

    fn intern(&self, name: &str) -> u32 {
        self.symbols.borrow_mut().intern(name)
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
        self.open.clear();
        self.unsettled.clear();
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
    /// Called before every token, so it dispatches on the first byte:
    /// one that starts no layout or comment (the token's own) returns
    /// at once, and only a non-ASCII one can still be whitespace.
    fn atmosphere(&mut self) -> Result<(), ReadError> {
        loop {
            let rest = self.rest();
            match rest.as_bytes().first() {
                None => return Ok(()),
                // A run of layout at once. The vertical tab is Unicode
                // whitespace but not ASCII whitespace.
                Some(b) if b.is_ascii_whitespace() || *b == 0x0b => {
                    let bytes = rest.as_bytes();
                    self.pos += bytes.iter().position(|b| !b.is_ascii_whitespace() && *b != 0x0b).unwrap_or(bytes.len());
                }
                Some(b';') => self.pos += rest.find('\n').unwrap_or(rest.len()),
                Some(b'#') if rest.starts_with("#|") => self.block_comment()?,
                Some(b'#') if rest.starts_with("#;") => {
                    self.pos += 2;
                    // Labels in a skipped datum are not defined after it.
                    let labels = self.labels.len();
                    self.nested(Self::datum)?;
                    self.labels.truncate(labels);
                }
                Some(b'#') if rest.starts_with("#!") => {
                    let directive = &rest[2..];
                    let len = directive.find(is_delimiter).unwrap_or(directive.len());
                    if let Some(m) = self.meter {
                        m.copied(len).map_err(|message| ReadError { message, pos: None })?;
                    }
                    match &directive[..len] {
                        "fold-case" => self.fold_case = true,
                        "no-fold-case" => self.fold_case = false,
                        name => return self.err(format!("unknown directive #!{}", brief_token(name)), self.pos),
                    }
                    self.pos += 2 + len;
                }
                Some(b) if *b < 0x80 => return Ok(()),
                Some(_) => {
                    let c = rest.chars().next().expect("a character");
                    if c.is_whitespace() {
                        self.pos += c.len_utf8();
                    } else {
                        return Ok(());
                    }
                }
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
            } else {
                // Up to the next character that could start or end one.
                match rest.find(['|', '#']) {
                    Some(0) => {
                        self.bump();
                    }
                    Some(skip) => self.pos += skip,
                    None => return self.err(format!("{INCOMPLETE} in a #| comment"), start),
                }
            }
        }
        Ok(())
    }

    fn token(&mut self) -> Result<Option<Token<D>>, ReadError> {
        if let Some(m) = self.meter {
            m.step().map_err(|message| ReadError { message, pos: None })?;
        }
        if self.depth >= MAX_DEPTH {
            return self.err(format!("data nested more than {MAX_DEPTH} deep"), self.pos);
        }
        self.depth += 1;
        let token = self.token_inner();
        self.depth -= 1;
        token
    }

    /// Read the parts of a compound datum, checking the stack once deep.
    #[inline]
    fn nested<T>(&mut self, read: impl FnOnce(&mut Self) -> T) -> T {
        if self.depth > SHALLOW { crate::deep(|| read(self)) } else { read(self) }
    }

    fn token_inner(&mut self) -> Result<Option<Token<D>>, ReadError> {
        self.atmosphere()?;
        let start = self.pos;
        let Some(c) = self.bump() else { return Ok(None) };
        let atom = |r: &Self, s: Sexp| D::atom(s, (start, r.pos));
        let datum = match c {
            '(' | '[' => self.nested(|r| r.list(if c == '(' { ')' } else { ']' }, start))?,
            ')' | ']' | '}' => return Ok(Some(Token::Close(c))),
            '{' => self.nested(|r| r.braced(false, start))?,
            '\'' => self.nested(|r| r.abbreviation("quote", start))?,
            '`' => self.nested(|r| r.abbreviation("quasiquote", start))?,
            ',' if self.peek() == Some('@') => {
                self.pos += 1;
                self.nested(|r| r.abbreviation("unquote-splicing", start))?
            }
            ',' => self.nested(|r| r.abbreviation("unquote", start))?,
            '"' => {
                let s = Sexp::Str(self.delimited('"', start)?);
                atom(self, s)
            }
            '|' => {
                let text = self.delimited('|', start)?;
                let s = Sexp::Sym(self.intern(&text));
                atom(self, s)
            }
            '#' => self.nested(|r| r.hash(start))?,
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
        // A long atom (a bignum's digits, a huge identifier interned) may
        // be more than the room left on its own.
        if let Some(m) = self.meter {
            m.copied(text.len()).map_err(|message| ReadError { message, pos: None })?;
        }
        // Most numbers in code are small decimal integers: no general parse.
        let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
        if (1..=18).contains(&digits.len())
            && digits.bytes().all(|b| b.is_ascii_digit())
            && let Ok(i) = text.parse()
        {
            return Ok(Sexp::Int(i));
        }
        // Numbers (and complex numbers) start with a digit, a sign or a point.
        let parsed = match text.as_bytes().first() {
            Some(b'0'..=b'9' | b'+' | b'-' | b'.') => num::parse(text, 10),
            _ => Parsed::No,
        };
        match parsed {
            Parsed::Number(n) => Ok(number(n)),
            Parsed::Unsupported(why) => self.err(format!("{}: {why}", brief_token(text)), start),
            // Folding copies only when it changes something: an ASCII
            // name with an uppercase letter, or any non-ASCII one
            // (titlecase letters like \u{1c5} lowercase without being
            // uppercase, so those fold as they always did).
            Parsed::No if self.fold_case && (!text.is_ascii() || text.bytes().any(|b| b.is_ascii_uppercase())) => {
                Ok(Sexp::Sym(self.intern(&text.to_lowercase())))
            }
            Parsed::No => Ok(Sexp::Sym(self.intern(text))),
        }
    }

    fn abbreviation(&mut self, name: &str, start: usize) -> Result<D, ReadError> {
        let head = D::atom(Sexp::Sym(self.intern(name)), (start, self.pos));
        let d = self.datum()?;
        Ok(D::list(vec![head, d], None, (start, self.pos)))
    }

    /// After `{` or `#{`: a dict (keys and values in turn) or a set.
    fn braced(&mut self, set: bool, start: usize) -> Result<D, ReadError> {
        self.braces += 1;
        let items = self.items('}', start);
        self.braces -= 1;
        match items? {
            (_, Some(_)) => self.err(format!("a {} cannot be dotted", if set { "set" } else { "dict" }), start),
            (items, None) if !set && items.len() % 2 == 1 => self.err("a dict needs a value after each key", start),
            (items, None) => Ok(D::braced(set, items, (start, self.pos))),
        }
    }

    fn list(&mut self, close: char, start: usize) -> Result<D, ReadError> {
        let (items, tail) = self.items(close, start)?;
        Ok(D::list(items, tail, (start, self.pos)))
    }

    /// The items and the dotted tail of a list up to `close`.
    fn items(&mut self, close: char, start: usize) -> Result<(Vec<D>, Option<D>), ReadError> {
        // Most lists in code (a call, a binding form) hold a handful
        // of items: one allocation at that size beats growing to it.
        let mut items = Vec::with_capacity(6);
        loop {
            let at = self.pos;
            match self.token()? {
                None => return self.err(format!("{INCOMPLETE}: the list here is not closed"), start),
                Some(Token::Datum(d)) => items.push(d),
                Some(Token::Close(c)) if c == close => return Ok((items, None)),
                Some(Token::Close(c)) => {
                    let open = match close {
                        ')' => '(',
                        ']' => '[',
                        _ => '{',
                    };
                    return self.err(format!("{c} closes a list opened with {open}"), at);
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
    fn delimited(&mut self, close: char, start: usize) -> Result<Rc<str>, ReadError> {
        // No escapes, the common case: the text as it stands, in one
        // copy (a string datum would copy a built `String` again).
        {
            let rest = self.rest();
            if let Some(end) = rest.find([close, '\\'])
                && rest.as_bytes()[end] == close as u8
            {
                if let Some(m) = self.meter {
                    m.copied(end).map_err(|message| ReadError { message, pos: None })?;
                }
                let text: Rc<str> = rest[..end].into();
                self.pos += end + close.len_utf8();
                return Ok(text);
            }
        }
        let mut out = String::new();
        loop {
            // The text up to the next escape or the end, at once.
            let rest = self.rest();
            let plain = rest.find([close, '\\']).unwrap_or(rest.len());
            out.push_str(&rest[..plain]);
            self.pos += plain;
            // One token may be more than the room left.
            if let Some(m) = self.meter {
                m.copied(plain).map_err(|message| ReadError { message, pos: None })?;
            }
            let before = out.len();
            match self.bump() {
                None => return self.err(INCOMPLETE, start),
                Some(c) if c == close => return Ok(out.into()),
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
                            // The hex run is parsed (and would be shown
                            // whole in the error): charge it first.
                            if let Some(m) = self.meter {
                                m.copied(end).map_err(|message| ReadError { message, pos: None })?;
                            }
                            match u32::from_str_radix(&rest[..end], 16).ok().and_then(char::from_u32) {
                                Some(c) => out.push(c),
                                None => return self.err(format!("bad escape \\x{};", brief_token(&rest[..end])), at),
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
                    // What the escape made counts like plain text does:
                    // a string of millions of `\n` is as much output
                    // (and as much delay past a stop) as a plain one.
                    if let Some(m) = self.meter
                        && out.len() > before
                    {
                        m.copied(out.len() - before).map_err(|message| ReadError { message, pos: None })?;
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
            '{' => {
                self.pos += 1;
                self.braced(true, start)
            }
            '\\' => {
                self.pos += 1;
                let c = self.character(start)?;
                atom(self, c)
            }
            ':' => {
                self.pos += 1;
                let text = self.atom_text();
                if let Some(m) = self.meter {
                    m.copied(text.len()).map_err(|message| ReadError { message, pos: None })?;
                }
                let k = Sexp::Keyword(self.intern(text));
                atom(self, k)
            }
            '0'..='9' => {
                let digits = self.rest().find(|c: char| !c.is_ascii_digit()).unwrap_or(self.rest().len());
                let n: u32 = self.rest()[..digits].parse().or_else(|_| self.err("datum label too large", start))?;
                self.pos += digits;
                match self.bump() {
                    Some('=') => {
                        self.labels.push(n);
                        self.open.push(n);
                        let d = self.datum()?;
                        self.open.pop();
                        Ok(D::labeled(n, d, (start, self.pos)))
                    }
                    // A dict is made of what it holds once that is read,
                    // and hashes its keys then: what it holds cannot
                    // refer to a datum not made yet, or that refers to
                    // one (made once the outermost datum is).
                    Some('#') if self.braces > 0 && (self.open.contains(&n) || self.unsettled.contains(&n)) => {
                        self.err(format!("#{n}# inside {{ }} refers to a datum not complete there"), start)
                    }
                    Some('#') if self.labels.contains(&n) => {
                        if let Some(i) = self.open.iter().position(|&m| m == n) {
                            self.unsettled.extend_from_slice(&self.open[i..]);
                        }
                        atom(self, Sexp::LabelRef(n))
                    }
                    Some('#') => self.err(format!("#{n}# refers to no datum label #{n}="), start),
                    _ => self.err("bad datum label", start),
                }
            }
            _ => {
                let text = self.atom_text();
                // The token may be parsed as a number: charge it like
                // an atom before that.
                if let Some(m) = self.meter {
                    m.copied(text.len()).map_err(|message| ReadError { message, pos: None })?;
                }
                match () {
                    _ if text.eq_ignore_ascii_case("t") || text.eq_ignore_ascii_case("true") => atom(self, Sexp::Bool(true)),
                    _ if text.eq_ignore_ascii_case("f") || text.eq_ignore_ascii_case("false") => atom(self, Sexp::Bool(false)),
                    _ if text.eq_ignore_ascii_case("u8") && self.peek() == Some('(') => {
                        self.pos += 1;
                        let bytes = self.bytes(start)?;
                        atom(self, Sexp::Bytes(bytes.into()))
                    }
                    _ => match num::parse(&self.src[start..self.pos], 10) {
                        Parsed::Number(n) => atom(self, number(n)),
                        Parsed::Unsupported(why) => self.err(format!("#{}: {why}", brief_token(text)), start),
                        Parsed::No => self.err(format!("unknown syntax #{}", brief_token(text)), start),
                    },
                }
            }
        }
    }

    /// After `#u8(`: bytes, exact integers from 0 to 255, up to `)`.
    fn bytes(&mut self, start: usize) -> Result<Vec<u8>, ReadError> {
        let mut bytes = Vec::new();
        loop {
            if let Some(m) = self.meter {
                m.step().map_err(|message| ReadError { message, pos: None })?;
            }
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
            // As an atom: a byte's token may itself be more than the
            // room left (megabytes of digits parse into arbitrary
            // precision before being refused as no byte).
            if let Some(m) = self.meter {
                m.copied(text.len()).map_err(|message| ReadError { message, pos: None })?;
            }
            match num::parse(text, 10) {
                Parsed::Number(N::I(b @ 0..=255)) => bytes.push(b as u8),
                _ if text.is_empty() => return self.err("a bytevector holds only bytes", at),
                _ => return self.err(format!("{} is not a byte (an exact integer from 0 to 255)", brief_token(text)), at),
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
        // The name is copied (and may be parsed as hex): charge it like
        // a token before either, however long it runs.
        if let Some(m) = self.meter {
            m.copied(name.len()).map_err(|message| ReadError { message, pos: None })?;
        }
        // The names are ASCII, so folding an ASCII name need not copy
        // it (a non-ASCII one still folds as Unicode, as before).
        let folded;
        let name = if self.fold_case && !name.is_ascii() {
            folded = name.to_lowercase();
            &folded
        } else {
            name
        };
        let named = CHAR_NAMES.iter().find(|(n, _)| if self.fold_case { n.eq_ignore_ascii_case(name) } else { *n == name });
        if let Some((_, c)) = named {
            return Ok(Sexp::Char(*c));
        }
        if let Some(hex) = name.strip_prefix(['x', 'X'])
            && let Some(c) = u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
        {
            return Ok(Sexp::Char(c));
        }
        self.err(format!("unknown character #\\{}", brief_token(name)), start)
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
/// A token as an error shows it: a token can be megabytes (digits that
/// are no byte, say), and formatting it whole would copy it whole.
fn brief_token(text: &str) -> String {
    if text.len() <= 40 {
        return text.to_owned();
    }
    format!("{}… ({} bytes)", &text[..text.floor_char_boundary(40)], text.len())
}

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
    if symbol_is_bare(name) { name.to_owned() } else { escaped(name, '|') }
}

/// Whether symbol `name` reads back as written, without `|...|`.
pub fn symbol_is_bare(name: &str) -> bool {
    let numeric_start = name.starts_with(|c: char| c.is_ascii_digit())
        || name.get(..6).is_some_and(|p| ["+inf.0", "-inf.0", "+nan.0", "-nan.0"].iter().any(|n| p.eq_ignore_ascii_case(n)))
        || matches!(
            name.as_bytes(),
            [b'+' | b'-', b'0'..=b'9', ..] | [b'+' | b'-' | b'.', b'.', b'0'..=b'9', ..] | [b'.', b'0'..=b'9', ..]
        );
    !name.is_empty()
        && name != "."
        && !name.starts_with('#')
        && !numeric_start
        && !name.contains(|c: char| is_delimiter(c) || c.is_control() || c == '\\')
        // The reader parses only these as numbers (`Reader::atom`); the
        // parser also refuses names past its size limits, whatever they say.
        && (!(name.starts_with(['+', '-', '.']) || 4 * name.len() as u64 > num::RATIO_BITS)
            || num::parse(name, 10) == Parsed::No)
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
    fn short_decimal_integers_read_as_the_general_parser_reads_them() {
        for text in ["0", "-0", "+7", "007", "999999999999999999", "-999999999999999999", "9223372036854775807", "1e3", "-", "+"] {
            let general = match num::parse(text, 10) {
                Parsed::Number(n) => number(n),
                _ => Sexp::Sym(intern(text)),
            };
            assert_eq!(format!("{:?}", one(text)), format!("{general:?}"), "{text}");
        }
    }

    #[test]
    fn symbols_written_bare_read_back_as_symbols() {
        let bare = ["a", "->x", "...", "+", "-", "+x", "inf.0"];
        for name in bare {
            assert!(symbol_is_bare(name), "{name}");
            assert!(matches!(one(name), Sexp::Sym(s) if s == intern(name)), "{name}");
        }
        for name in ["-.5x", "+i", "-inf.0", "+NaN.0", "1+", ".5", "a b", "#a", "", "."] {
            assert!(!symbol_is_bare(name), "{name}");
        }
        // Past the number parser's size limits, as it always was.
        assert!(!symbol_is_bare(&format!("a/{}", "a".repeat(32767))));
        assert!(!symbol_is_bare(&"a".repeat(262145)));
        assert!(symbol_is_bare(&"a".repeat(32769)));
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
        // A titlecase letter lowercases without being uppercase.
        assert_eq!(display_sexp(&one("#!fold-case \u{1c5}")), "\u{1c6}");
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
