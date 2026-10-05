//! Syntactic analysis of a techne Lisp file: definitions, scopes and the
//! binding each identifier refers to. Nothing is evaluated (a language server
//! must not run user code), so macros are not expanded; the core binding forms
//! and the prelude's binding macros are understood directly.

use std::collections::HashSet;

use steel_parser::{ast::ExprKind, parser::Parser, tokens::TokenType};

pub type Span = (u32, u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefKind {
    Function,
    Variable,
    Macro,
    Record,
    Generic,
    Local,
}

#[derive(Clone, Debug)]
pub struct Def {
    pub name: String,
    /// The defining occurrence of the name.
    pub span: Span,
    /// Where the binding is visible (whole file for top-level definitions).
    pub scope: Span,
    pub kind: DefKind,
    /// Header such as `(greet name #:greeting [greeting "hello"])`.
    pub signature: Option<String>,
    pub doc: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Def(usize),
    Unresolved,
}

#[derive(Clone, Debug)]
pub struct Ref {
    pub name: String,
    pub span: Span,
    pub target: Target,
    /// Only references in evaluated positions are checked for unbound names.
    pub checked: bool,
}

#[derive(Default, Debug)]
pub struct Analysis {
    pub error: Option<(String, u32)>,
    pub defs: Vec<Def>,
    pub refs: Vec<Ref>,
    pub requires: Vec<(String, Span)>,
    pub provides: Vec<String>,
}

impl Analysis {
    pub fn top_level(&self) -> impl Iterator<Item = &Def> {
        self.defs.iter().filter(|d| d.kind != DefKind::Local)
    }

    /// The reference or definition name at byte offset `pos`.
    pub fn at(&self, pos: u32) -> Option<(&str, Option<&Def>)> {
        if let Some(r) = self.refs.iter().find(|r| r.span.0 <= pos && pos <= r.span.1) {
            let def = match r.target {
                Target::Def(i) => Some(&self.defs[i]),
                Target::Unresolved => None,
            };
            return Some((&r.name, def));
        }
        self.defs.iter().find(|d| d.span.0 <= pos && pos <= d.span.1).map(|d| (d.name.as_str(), Some(d)))
    }

    /// Bindings visible at `pos` (locals in scope and top-level definitions).
    pub fn visible_at(&self, pos: u32) -> impl Iterator<Item = &Def> {
        self.defs.iter().filter(move |d| d.kind != DefKind::Local || (d.scope.0 <= pos && pos <= d.scope.1))
    }
}

/// Identifiers that are syntax, not variable references.
const SYNTAX_WORDS: &[&str] = &["else", "=>", "_", "...", "#%unquote", "#%unquote-splicing", "unquote", "unquote-splicing"];

/// Type names accepted by `define-method` besides record types.
const TYPE_NAMES: &[&str] = &[
    "t", "number", "integer", "float", "string", "symbol", "keyword", "char", "list", "pair", "null", "vector",
    "procedure", "boolean", "hash-table", "record", "void", "eof", "foreign",
];

fn ident(e: &ExprKind) -> Option<(&str, Span)> {
    match e {
        ExprKind::Atom(a) => {
            let span = (a.syn.span.start, a.syn.span.end);
            match &a.syn.ty {
                TokenType::Identifier(s) => Some((s.resolve(), span)),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Name of a form's head, including the parser's special tokens.
fn head(items: &[ExprKind]) -> Option<&str> {
    match items.first()? {
        ExprKind::Atom(a) => Some(match &a.syn.ty {
            TokenType::Identifier(s) => s.resolve(),
            TokenType::Define => "define",
            TokenType::If => "if",
            TokenType::Let => "let",
            TokenType::Lambda => "lambda",
            TokenType::Begin => "begin",
            TokenType::Set => "set!",
            TokenType::Quote => "quote",
            TokenType::Require => "require",
            TokenType::DefineSyntax => "define-syntax",
            TokenType::SyntaxRules => "syntax-rules",
            TokenType::QuasiQuote => "quasiquote",
            _ => return None,
        }),
        _ => None,
    }
}

fn span_of(e: &ExprKind) -> Span {
    match e {
        ExprKind::Atom(a) => (a.syn.span.start, a.syn.span.end),
        ExprKind::List(l) => (l.location.start, l.location.end),
        ExprKind::Quote(q) => (q.location.span.start, q.location.span.end),
        _ => (0, 0),
    }
}

fn list(e: &ExprKind) -> Option<&[ExprKind]> {
    match e {
        ExprKind::List(l) => Some(&l.args),
        _ => None,
    }
}

fn string_lit(e: &ExprKind) -> Option<String> {
    match e {
        ExprKind::Atom(a) => match &a.syn.ty {
            TokenType::StringLiteral(s) => Some(s.resolve().to_string()),
            _ => None,
        },
        _ => None,
    }
}

struct Walker<'s> {
    source: &'s str,
    a: Analysis,
    /// Innermost last: (name, def index).
    env: Vec<(String, usize)>,
    top: Vec<(String, usize)>,
}

pub fn analyze(source: &str) -> Analysis {
    let exprs = match Parser::parse_without_lowering(source) {
        Ok(e) => e,
        Err(e) => return Analysis { error: Some((e.to_string(), e.span().start)), ..Analysis::default() },
    };
    let mut w = Walker { source, a: Analysis::default(), env: Vec::new(), top: Vec::new() };
    let whole = (0, source.len() as u32);
    // Top-level definitions are visible everywhere in the file.
    for e in &exprs {
        w.collect_defs(e, whole, true);
    }
    w.top = w.a.defs.iter().enumerate().map(|(i, d)| (d.name.clone(), i)).collect();
    for e in &exprs {
        w.walk(e);
    }
    w.a
}

impl Walker<'_> {
    fn text(&self, span: Span) -> String {
        self.source.get(span.0 as usize..span.1 as usize).unwrap_or("").to_string()
    }

    fn add_def(&mut self, name: &str, span: Span, scope: Span, kind: DefKind, signature: Option<String>, doc: Option<String>) -> usize {
        self.a.defs.push(Def { name: name.to_string(), span, scope, kind, signature, doc });
        self.a.defs.len() - 1
    }

    /// Record the definitions a body-level form introduces (for letrec*
    /// scoping); returns their indices.
    fn collect_defs(&mut self, e: &ExprKind, scope: Span, top: bool) -> Vec<usize> {
        let Some(items) = list(e) else { return vec![] };
        let kind_fn = if top { DefKind::Function } else { DefKind::Local };
        let kind_var = if top { DefKind::Variable } else { DefKind::Local };
        let mut out = Vec::new();
        match head(items) {
            Some("define") => match items.get(1) {
                Some(x) if ident(x).is_some() => {
                    let (n, s) = ident(x).unwrap();
                    let is_lambda = items.get(2).and_then(list).and_then(head) == Some("lambda");
                    out.push(self.add_def(n, s, scope, if is_lambda { kind_fn } else { kind_var }, None, None));
                }
                Some(sig @ ExprKind::List(l)) => {
                    // (define (name . params) [doc] body...), also curried.
                    let mut name = l.args.first();
                    while let Some(ExprKind::List(inner)) = name {
                        name = inner.args.first();
                    }
                    if let Some((n, s)) = name.and_then(ident) {
                        let doc = if items.len() > 3 { items.get(2).and_then(string_lit) } else { None };
                        out.push(self.add_def(n, s, scope, kind_fn, Some(self.text(span_of(sig))), doc));
                    }
                }
                _ => {}
            },
            Some("define-syntax") => {
                if let Some((n, s)) = items.get(1).and_then(ident) {
                    out.push(self.add_def(n, s, scope, if top { DefKind::Macro } else { DefKind::Local }, None, None));
                }
            }
            Some("define-generic") => {
                let target = items.get(1).map(|x| list(x).and_then(|l| l.first()).unwrap_or(x));
                if let Some((n, s)) = target.and_then(ident) {
                    let sig = items.get(1).filter(|x| list(x).is_some()).map(|x| self.text(span_of(x)));
                    out.push(self.add_def(n, s, scope, if top { DefKind::Generic } else { DefKind::Local }, sig, None));
                }
            }
            Some("define-values") => {
                for (n, s) in items.get(1).and_then(list).unwrap_or(&[]).iter().filter_map(ident) {
                    out.push(self.add_def(n, s, scope, kind_var, None, None));
                }
            }
            Some("define-record-type") => {
                let kind = if top { DefKind::Record } else { DefKind::Local };
                if let Some((n, s)) = items.get(1).and_then(ident) {
                    out.push(self.add_def(n, s, scope, kind, Some(format!("record type {n}")), None));
                }
                if let Some(ctor) = items.get(2) {
                    let ctor_name = list(ctor).and_then(|l| l.first()).unwrap_or(ctor);
                    if let Some((n, s)) = ident(ctor_name) {
                        out.push(self.add_def(n, s, scope, kind_fn, Some(self.text(span_of(ctor))), None));
                    }
                }
                if let Some((n, s)) = items.get(3).and_then(ident) {
                    out.push(self.add_def(n, s, scope, kind_fn, Some(format!("({n} value)")), None));
                }
                for field in items.iter().skip(4).filter_map(list) {
                    for (n, s) in field.iter().skip(1).filter_map(ident) {
                        out.push(self.add_def(n, s, scope, kind_fn, None, None));
                    }
                }
            }
            Some("begin") => {
                for f in &items[1..] {
                    out.extend(self.collect_defs(f, scope, top));
                }
            }
            _ => {}
        }
        out
    }

    fn reference(&mut self, name: &str, span: Span, checked: bool) {
        if SYNTAX_WORDS.contains(&name) || name.starts_with("#:") {
            return;
        }
        let target = self
            .env
            .iter()
            .rev()
            .chain(self.top.iter().rev())
            .find(|(n, _)| n == name)
            .map_or(Target::Unresolved, |(_, i)| Target::Def(*i));
        self.a.refs.push(Ref { name: name.to_string(), span, target, checked });
    }

    fn bind_local(&mut self, e: &ExprKind, scope: Span) {
        if let Some((n, s)) = ident(e) {
            let i = self.add_def(n, s, scope, DefKind::Local, None, None);
            self.env.push((n.to_string(), i));
        }
    }

    /// Bind a parameter list: identifiers, `[x default]`, keywords, rest.
    fn bind_formals(&mut self, formals: &ExprKind, scope: Span) {
        match formals {
            ExprKind::Atom(_) => self.bind_local(formals, scope),
            ExprKind::List(l) => {
                for p in &l.args {
                    match list(p) {
                        Some([var, default]) => {
                            self.walk(default);
                            self.bind_local(var, scope);
                        }
                        _ => self.bind_local(p, scope),
                    }
                }
            }
            _ => {}
        }
    }

    /// A body: internal definitions are visible to the whole body.
    fn body(&mut self, forms: &[ExprKind], scope: Span) {
        let mark = self.env.len();
        let defs: Vec<usize> = forms.iter().flat_map(|f| self.collect_defs(f, scope, false)).collect();
        for i in defs {
            let name = self.a.defs[i].name.clone();
            self.env.push((name, i));
        }
        for f in forms {
            self.walk(f);
        }
        self.env.truncate(mark);
    }

    fn walk_all(&mut self, forms: &[ExprKind]) {
        for f in forms {
            self.walk(f);
        }
    }

    fn walk(&mut self, e: &ExprKind) {
        match e {
            ExprKind::Atom(_) => {
                if let Some((n, s)) = ident(e) {
                    self.reference(n, s, true);
                }
            }
            ExprKind::List(l) => self.walk_form(&l.args, (l.location.start, l.location.end)),
            _ => {}
        }
    }

    fn walk_form(&mut self, items: &[ExprKind], span: Span) {
        let mark = self.env.len();
        match head(items) {
            Some(
                "quote" | "quasiquote" | "define-syntax" | "let-syntax" | "letrec-syntax" | "syntax-rules" | "require"
                | "help",
            ) => {
                if head(items) == Some("require") {
                    for r in &items[1..] {
                        if let Some(path) = string_lit(r) {
                            self.a.requires.push((path, span_of(r)));
                        }
                    }
                }
            }
            Some("provide") => {
                for p in &items[1..] {
                    if let Some((n, s)) = ident(p) {
                        self.a.provides.push(n.to_string());
                        self.reference(n, s, true);
                    }
                }
            }
            Some("define") => match items.get(1) {
                Some(ExprKind::List(sig)) => {
                    // The name is bound by collect_defs; parameters scope the body.
                    self.bind_params(&sig.args[1.min(sig.args.len())..], span);
                    self.body(&items[2..], span);
                }
                _ => self.walk_all(&items[2..]),
            },
            Some("lambda") => {
                if let Some(f) = items.get(1) {
                    self.bind_formals(f, span);
                }
                self.body(&items[2..], span);
            }
            Some(form @ ("let" | "let*" | "letrec" | "letrec*")) => {
                let named = form == "let" && items.get(1).and_then(ident).is_some();
                let (bindings, body) = if named {
                    (items.get(2), &items[3.min(items.len())..])
                } else {
                    (items.get(1), &items[2.min(items.len())..])
                };
                let pairs: Vec<&[ExprKind]> = bindings.and_then(list).unwrap_or(&[]).iter().filter_map(list).collect();
                let init = |w: &mut Self, p: &[ExprKind]| {
                    if let Some(e) = p.get(1) {
                        w.walk(e);
                    }
                };
                match form {
                    // Each init sees the variables bound before it.
                    "let*" => {
                        for p in &pairs {
                            init(self, p);
                            self.bind_local(&p[0], span);
                        }
                    }
                    // Inits see all the variables.
                    "letrec" | "letrec*" => {
                        for p in &pairs {
                            self.bind_local(&p[0], span);
                        }
                        for p in &pairs {
                            init(self, p);
                        }
                    }
                    // Plain and named let: inits see the outer scope.
                    _ => {
                        for p in &pairs {
                            init(self, p);
                        }
                        if named {
                            self.bind_local(&items[1], span);
                        }
                        for p in &pairs {
                            self.bind_local(&p[0], span);
                        }
                    }
                }
                self.body(body, span);
            }
            Some("do") => {
                let specs: Vec<&[ExprKind]> = items.get(1).and_then(list).unwrap_or(&[]).iter().filter_map(list).collect();
                for s in &specs {
                    if let Some(init) = s.get(1) {
                        self.walk(init);
                    }
                }
                for s in &specs {
                    self.bind_local(&s[0], span);
                }
                for s in &specs {
                    self.walk_all(&s[2.min(s.len())..]);
                }
                self.walk_all(&items[2.min(items.len())..]);
            }
            Some("receive") => {
                if let Some(e) = items.get(2) {
                    self.walk(e);
                }
                if let Some(f) = items.get(1) {
                    self.bind_formals(f, span);
                }
                self.body(&items[3.min(items.len())..], span);
            }
            Some("let-values" | "let*-values") => {
                for b in items.get(1).and_then(list).unwrap_or(&[]).iter().filter_map(list) {
                    if let Some(e) = b.get(1) {
                        self.walk(e);
                    }
                    self.bind_formals(&b[0], span);
                }
                self.body(&items[2.min(items.len())..], span);
            }
            Some("define-values") => {
                if let Some(e) = items.get(2) {
                    self.walk(e);
                }
            }
            Some("case-lambda") => {
                for clause in items[1..].iter().filter_map(list) {
                    let m = self.env.len();
                    if let Some(f) = clause.first() {
                        self.bind_formals(f, span);
                    }
                    self.body(&clause[1..], span);
                    self.env.truncate(m);
                }
            }
            Some("define-record-type" | "define-generic") => {}
            Some("define-method") => {
                if let Some(sig) = items.get(1).and_then(list) {
                    if let Some((n, s)) = sig.first().and_then(ident) {
                        self.reference(n, s, true);
                    }
                    for p in &sig[1.min(sig.len())..] {
                        match list(p) {
                            Some([var, ty]) => {
                                if let Some((tn, ts)) = ident(ty)
                                    && !TYPE_NAMES.contains(&tn)
                                {
                                    self.reference(tn, ts, true);
                                }
                                self.bind_local(var, span);
                            }
                            _ => self.bind_local(p, span),
                        }
                    }
                }
                self.body(&items[2.min(items.len())..], span);
            }
            Some("guard") => {
                self.body(&items[2.min(items.len())..], span);
                if let Some(spec) = items.get(1).and_then(list) {
                    let m = self.env.len();
                    if let Some(v) = spec.first() {
                        self.bind_local(v, span);
                    }
                    for clause in spec[1.min(spec.len())..].iter().filter_map(list) {
                        self.walk_all(clause);
                    }
                    self.env.truncate(m);
                }
            }
            // (restart-case expr (name formals body...) ...)
            Some("restart-case") => {
                if let Some(e) = items.get(1) {
                    self.walk(e);
                }
                for clause in items[2.min(items.len())..].iter().filter_map(list) {
                    let m = self.env.len();
                    if let Some(f) = clause.get(1) {
                        self.bind_formals(f, span);
                    }
                    self.body(&clause[2.min(clause.len())..], span);
                    self.env.truncate(m);
                }
            }
            Some("match-let") => {
                let pairs: Vec<&[ExprKind]> = items.get(1).and_then(list).unwrap_or(&[]).iter().filter_map(list).collect();
                for p in &pairs {
                    if let Some(e) = p.get(1) {
                        self.walk(e);
                    }
                }
                for p in &pairs {
                    self.bind_pattern(&p[0], span, false);
                }
                self.body(&items[2.min(items.len())..], span);
            }
            Some("case") => {
                if let Some(k) = items.get(1) {
                    self.walk(k);
                }
                for clause in items[2.min(items.len())..].iter().filter_map(list) {
                    self.walk_all(&clause[1.min(clause.len())..]);
                }
            }
            Some("match" | "match-lambda") => {
                let clauses = if head(items) == Some("match") {
                    if let Some(subject) = items.get(1) {
                        self.walk(subject);
                    }
                    &items[2.min(items.len())..]
                } else {
                    &items[1..]
                };
                for clause in clauses.iter().filter_map(list) {
                    let m = self.env.len();
                    if let Some(pat) = clause.first() {
                        self.bind_pattern(pat, span, false);
                    }
                    self.walk_all(&clause[1..]);
                    self.env.truncate(m);
                }
            }
            _ => self.walk_all(items),
        }
        self.env.truncate(mark);
    }

    fn bind_params(&mut self, params: &[ExprKind], scope: Span) {
        for p in params {
            match list(p) {
                Some([var, default]) => {
                    self.walk(default);
                    self.bind_local(var, scope);
                }
                _ => self.bind_local(p, scope),
            }
        }
    }

    /// Pattern variables of a `match` pattern. List heads (`list`, `cons`,
    /// `?`, record types) are constructors; predicates are references.
    fn bind_pattern(&mut self, pat: &ExprKind, scope: Span, in_head: bool) {
        match pat {
            ExprKind::Atom(_) => {
                if let Some((n, s)) = ident(pat) {
                    if in_head {
                        if !matches!(n, "list" | "cons" | "vector" | "?" | "and" | "or" | "not") {
                            self.reference(n, s, false);
                        }
                    } else if !SYNTAX_WORDS.contains(&n) {
                        self.bind_local(pat, scope);
                    }
                }
            }
            ExprKind::List(l) => {
                if head(&l.args) == Some("quote") {
                    return;
                }
                let is_pred = head(&l.args) == Some("?");
                for (i, sub) in l.args.iter().enumerate() {
                    if is_pred && i == 1 {
                        self.walk(sub);
                    } else {
                        self.bind_pattern(sub, scope, i == 0);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Unresolved references that are not built-ins: (name, span).
pub fn unbound<'a>(a: &'a Analysis, known: &'a HashSet<String>) -> impl Iterator<Item = &'a Ref> {
    a.refs.iter().filter(move |r| r.checked && r.target == Target::Unresolved && !known.contains(&r.name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def_at<'a>(a: &'a Analysis, src: &str, needle: &str, nth: usize) -> Option<&'a Def> {
        let pos = src.match_indices(needle).nth(nth).unwrap().0 as u32;
        a.at(pos).and_then(|(_, d)| d)
    }

    #[test]
    fn resolves_scopes() {
        let src = r#"(define (greet name #:greeting [greeting "hi"]) "Doc." (string-append greeting name))
(define (f x) (let ((y x)) (let loop ((i y)) (if (< i 3) (loop (+ i 1)) (g i)))))
(define (g z) (match z [(list a b) (+ a b)] [(? number? n) n] [_ 0]))
(define-record-type point (make-point x y) point? (x point-x))
(point-x (make-point 1 2))
(restart-case (f 1) (use-value (v) v) (skip () 'skipped))
(match-let ([(list p q) (list 1 2)]) (+ p q))
(help undefined-but-quoted)
(unknown-thing 1)"#;
        let a = analyze(src);
        assert!(a.error.is_none());
        let greet = def_at(&a, src, "greeting name", 0).unwrap();
        assert_eq!(greet.kind, DefKind::Local);
        let g = def_at(&a, src, "(g i)", 0);
        assert!(g.is_none(), "position of the list, not the identifier");
        let pos = src.find("(g i)").unwrap() as u32 + 1;
        let (_, d) = a.at(pos).unwrap();
        assert_eq!(d.unwrap().name, "g");
        assert_eq!(d.unwrap().signature.as_deref(), Some("(g z)"));
        let (_, d) = a.at(src.find("loop (+").unwrap() as u32).unwrap();
        assert_eq!(d.unwrap().kind, DefKind::Local);
        let (_, d) = a.at(src.find("(+ a b)").unwrap() as u32 + 3).unwrap();
        assert_eq!(d.unwrap().name, "a");
        let (_, d) = a.at(src.find("point-x (make").unwrap() as u32).unwrap();
        assert_eq!(d.unwrap().kind, DefKind::Function);
        let greet_def = a.top_level().find(|d| d.name == "greet").unwrap();
        assert_eq!(greet_def.doc.as_deref(), Some("Doc."));
        let known: HashSet<String> = ["string-append", "<", "+", "number?", "list"].iter().map(|s| s.to_string()).collect();
        let unbound: Vec<_> = unbound(&a, &known).map(|r| r.name.as_str()).collect();
        assert_eq!(unbound, vec!["unknown-thing"]);
    }

    #[test]
    fn reports_reader_errors() {
        let a = analyze("(define (f x)\n  (+ x 1)");
        assert!(a.error.is_some());
    }
}
