//! Syntactic analysis of a techne Lisp file: definitions, scopes and the
//! binding each identifier refers to. The file is read by the VM's own
//! reader (`read_syntax`, with spans), so it parses exactly as the VM does.
//! Nothing is evaluated (a language server must not run user code), so
//! macros are not expanded: the core binding forms and the prelude's binding
//! macros are understood directly, and identifiers inside other macro uses
//! are resolved but not reported as unbound.

use std::collections::HashSet;

use techne_vm::reader::{Sexp, Syntax, SyntaxKind, read_syntax, symbol_name};

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

/// A library export: the name inside the library, and the name importers see.
pub type Export = (String, String);

#[derive(Default, Debug)]
pub struct Analysis {
    pub error: Option<(String, u32)>,
    pub defs: Vec<Def>,
    pub refs: Vec<Ref>,
    pub requires: Vec<(String, Span)>,
    pub provides: Vec<String>,
    /// The import sets of top-level `import` forms.
    pub imports: Vec<Syntax>,
    /// Libraries the file defines: name and exports.
    pub libraries: Vec<(Vec<String>, Vec<Export>)>,
    /// Files `include`d, at top level or by a library, relative to the file.
    pub includes: Vec<(String, Span)>,
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
const SYNTAX_WORDS: &[&str] = &["else", "=>", "_", "...", "unquote", "unquote-splicing"];

/// Type names accepted by `define-method` besides record types.
const TYPE_NAMES: &[&str] = &[
    "t",
    "number",
    "integer",
    "float",
    "string",
    "symbol",
    "keyword",
    "char",
    "list",
    "pair",
    "null",
    "vector",
    "procedure",
    "boolean",
    "hash-table",
    "record",
    "void",
    "eof",
    "foreign",
];

pub fn ident(e: &Syntax) -> Option<(String, Span)> {
    e.sym().map(|s| (symbol_name(s).to_string(), e.span))
}

/// Name of a form's head.
pub fn head(items: &[Syntax]) -> Option<String> {
    items.first()?.sym().map(|s| symbol_name(s).to_string())
}

/// The items of a list, dotted or not.
pub fn list(e: &Syntax) -> Option<&[Syntax]> {
    match &e.kind {
        SyntaxKind::List(items, _) => Some(items),
        _ => None,
    }
}

/// The docstring of `(syntax-rules [ellipsis] (literal ...) "doc" rule ...)`.
fn syntax_rules_doc(items: &[Syntax]) -> Option<String> {
    let after = if items.get(1).is_some_and(|i| i.sym().is_some()) { 3 } else { 2 };
    (head(items).as_deref() == Some("syntax-rules")).then(|| items.get(after).and_then(string_lit)).flatten()
}

fn string_lit(e: &Syntax) -> Option<String> {
    match &e.kind {
        SyntaxKind::Atom(Sexp::Str(s)) => Some(s.to_string()),
        _ => None,
    }
}

/// The parts of a library name such as `(srfi 1)`.
pub fn library_name(name: &Syntax) -> Vec<String> {
    list(name)
        .unwrap_or(&[])
        .iter()
        .map(|p| match &p.kind {
            SyntaxKind::Atom(Sexp::Sym(s)) => symbol_name(*s).to_string(),
            SyntaxKind::Atom(Sexp::Int(i)) => i.to_string(),
            _ => String::new(),
        })
        .collect()
}

/// `items[from..]`, empty when there are fewer items.
fn from(items: &[Syntax], i: usize) -> &[Syntax] {
    &items[i.min(items.len())..]
}

struct Walker<'s> {
    source: &'s str,
    a: Analysis,
    /// Innermost last: (name, def index).
    env: Vec<(String, usize)>,
    top: Vec<(String, usize)>,
    /// Macros defined elsewhere (required files, the runtime).
    macros: &'s HashSet<String>,
    /// Inside a use of a macro the walker does not know: references are
    /// resolved but not checked, since the macro may bind them.
    in_macro: usize,
}

/// Analyse `source`; `macros` names the macros it can use from elsewhere.
pub fn analyze(source: &str, macros: &HashSet<String>) -> Analysis {
    let exprs = match read_syntax(source) {
        Ok(e) => e,
        Err(e) => return Analysis { error: Some((e.message, e.pos.unwrap_or(0))), ..Analysis::default() },
    };
    let mut w = Walker { source, a: Analysis::default(), env: Vec::new(), top: Vec::new(), macros, in_macro: 0 };
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
    fn collect_defs(&mut self, e: &Syntax, scope: Span, top: bool) -> Vec<usize> {
        let Some(items) = list(e) else { return vec![] };
        let kind_fn = if top { DefKind::Function } else { DefKind::Local };
        let kind_var = if top { DefKind::Variable } else { DefKind::Local };
        let mut out = Vec::new();
        match head(items).as_deref() {
            Some("define") => match items.get(1) {
                Some(x) if x.sym().is_some() => {
                    let (n, s) = ident(x).unwrap();
                    let is_lambda = items.get(2).and_then(list).and_then(head).as_deref() == Some("lambda");
                    // (define name value "doc"), or a lambda's own docstring.
                    let doc = match items.len() {
                        4 => items.get(3).and_then(string_lit),
                        _ => items.get(2).and_then(list).filter(|l| l.len() > 3).and_then(|l| string_lit(&l[2])),
                    };
                    out.push(self.add_def(&n, s, scope, if is_lambda { kind_fn } else { kind_var }, None, doc));
                }
                Some(sig) if list(sig).is_some() => {
                    // (define (name . params) [doc] body...), also curried.
                    let mut name = list(sig).and_then(|l| l.first());
                    while let Some(inner) = name.and_then(list) {
                        name = inner.first();
                    }
                    if let Some((n, s)) = name.and_then(ident) {
                        let doc = if items.len() > 3 { items.get(2).and_then(string_lit) } else { None };
                        out.push(self.add_def(&n, s, scope, kind_fn, Some(self.text(sig.span)), doc));
                    }
                }
                _ => {}
            },
            Some("define-syntax") => {
                if let Some((n, s)) = items.get(1).and_then(ident) {
                    let doc = items.get(2).and_then(list).and_then(syntax_rules_doc);
                    out.push(self.add_def(&n, s, scope, if top { DefKind::Macro } else { DefKind::Local }, None, doc));
                }
            }
            Some("define-generic") => {
                let target = items.get(1).map(|x| list(x).and_then(|l| l.first()).unwrap_or(x));
                if let Some((n, s)) = target.and_then(ident) {
                    let sig = items.get(1).filter(|x| list(x).is_some()).map(|x| self.text(x.span));
                    out.push(self.add_def(&n, s, scope, if top { DefKind::Generic } else { DefKind::Local }, sig, None));
                }
            }
            Some("define-values") => match items.get(1) {
                Some(formals) if formals.sym().is_some() => {
                    let (n, s) = ident(formals).unwrap();
                    out.push(self.add_def(&n, s, scope, kind_var, None, None));
                }
                Some(formals) => {
                    let tail = match &formals.kind {
                        SyntaxKind::List(_, Some(t)) => Some(&**t),
                        _ => None,
                    };
                    for (n, s) in list(formals).unwrap_or(&[]).iter().chain(tail).filter_map(ident) {
                        out.push(self.add_def(&n, s, scope, kind_var, None, None));
                    }
                }
                None => {}
            },
            Some("define-record-type") => {
                let kind = if top { DefKind::Record } else { DefKind::Local };
                if let Some((n, s)) = items.get(1).and_then(ident) {
                    out.push(self.add_def(&n, s, scope, kind, Some(format!("record type {n}")), None));
                }
                if let Some(ctor) = items.get(2) {
                    let ctor_name = list(ctor).and_then(|l| l.first()).unwrap_or(ctor);
                    if let Some((n, s)) = ident(ctor_name) {
                        out.push(self.add_def(&n, s, scope, kind_fn, Some(self.text(ctor.span)), None));
                    }
                }
                if let Some((n, s)) = items.get(3).and_then(ident) {
                    out.push(self.add_def(&n, s, scope, kind_fn, Some(format!("({n} value)")), None));
                }
                for field in from(items, 4).iter().filter_map(list) {
                    for (n, s) in from(field, 1).iter().filter_map(ident) {
                        out.push(self.add_def(&n, s, scope, kind_fn, None, None));
                    }
                }
            }
            Some("begin") => {
                for f in from(items, 1) {
                    out.extend(self.collect_defs(f, scope, top));
                }
            }
            // A library's definitions, as the file's: the library's body is
            // in the file, and what it exports is used after it.
            Some("define-library") => {
                for decl in from(items, 2).iter().filter(|d| list(d).and_then(head).as_deref() == Some("begin")) {
                    out.extend(self.collect_defs(decl, scope, top));
                }
            }
            _ => {}
        }
        out
    }

    fn reference(&mut self, name: &str, span: Span, checked: bool) {
        if SYNTAX_WORDS.contains(&name) {
            return;
        }
        // Special forms (`if`, `lambda`...) are syntax unless shadowed.
        let shadowed = self.env.iter().chain(self.top.iter()).any(|(n, _)| n == name);
        if !shadowed && techne_vm::compiler::is_special_form(name) {
            return;
        }
        let target = self
            .env
            .iter()
            .rev()
            .chain(self.top.iter().rev())
            .find(|(n, _)| n == name)
            .map_or(Target::Unresolved, |(_, i)| Target::Def(*i));
        let checked = checked && self.in_macro == 0;
        self.a.refs.push(Ref { name: name.to_string(), span, target, checked });
    }

    fn bind_local(&mut self, e: &Syntax, scope: Span) {
        if let Some((n, s)) = ident(e) {
            let i = self.add_def(&n, s, scope, DefKind::Local, None, None);
            self.env.push((n, i));
        }
    }

    /// Bind a parameter list: identifiers, `[x default]`, keywords, rest.
    fn bind_formals(&mut self, formals: &Syntax, scope: Span) {
        match &formals.kind {
            SyntaxKind::Atom(_) => self.bind_local(formals, scope),
            SyntaxKind::List(items, tail) => {
                self.bind_params(items, scope);
                if let Some(t) = tail {
                    self.bind_local(t, scope);
                }
            }
            _ => {}
        }
    }

    fn bind_params(&mut self, params: &[Syntax], scope: Span) {
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

    /// A body: internal definitions are visible to the whole body.
    fn body(&mut self, forms: &[Syntax], scope: Span) {
        let mark = self.env.len();
        let defs: Vec<usize> = forms.iter().flat_map(|f| self.collect_defs(f, scope, false)).collect();
        for i in defs {
            let name = self.a.defs[i].name.clone();
            self.env.push((name, i));
        }
        self.walk_all(forms);
        self.env.truncate(mark);
    }

    fn walk_all(&mut self, forms: &[Syntax]) {
        for f in forms {
            self.walk(f);
        }
    }

    fn walk(&mut self, e: &Syntax) {
        match &e.kind {
            SyntaxKind::Atom(Sexp::Sym(_)) => {
                let (n, s) = ident(e).unwrap();
                self.reference(&n, s, true);
            }
            SyntaxKind::List(items, tail) => {
                self.walk_form(items, e.span);
                if let Some(t) = tail {
                    self.walk(t);
                }
            }
            _ => {}
        }
    }

    fn include(&mut self, names: &[Syntax]) {
        self.a.includes.extend(names.iter().filter_map(|n| string_lit(n).map(|s| (s, n.span))));
    }

    /// Whether `name` names a macro the walker does not know the shape of.
    fn is_macro(&self, name: &str) -> bool {
        let local = self.env.iter().rev().chain(self.top.iter().rev()).find(|(n, _)| n == name);
        match local {
            Some((_, i)) => self.a.defs[*i].kind == DefKind::Macro,
            None => self.macros.contains(name),
        }
    }

    fn walk_form(&mut self, items: &[Syntax], span: Span) {
        let mark = self.env.len();
        let h = head(items);
        match h.as_deref() {
            Some("quote" | "quasiquote" | "define-syntax" | "let-syntax" | "letrec-syntax" | "syntax-rules" | "help") => {}
            Some("include" | "include-ci") => self.include(from(items, 1)),
            Some("import") => self.a.imports.extend(from(items, 1).iter().cloned()),
            Some("require") => {
                for r in from(items, 1) {
                    if let Some(path) = string_lit(r) {
                        self.a.requires.push((path, r.span));
                    }
                }
            }
            Some("provide") => {
                for p in from(items, 1) {
                    if let Some((n, s)) = ident(p) {
                        self.a.provides.push(n.clone());
                        self.reference(&n, s, true);
                    }
                }
            }
            Some("define-library") => {
                let name = items.get(1).map(library_name).unwrap_or_default();
                let mut exports = Vec::new();
                for decl in from(items, 2).iter().filter_map(list) {
                    match head(decl).as_deref() {
                        Some("begin") => self.walk_all(from(decl, 1)),
                        Some("import") => self.a.imports.extend(from(decl, 1).iter().cloned()),
                        Some("include" | "include-ci") => self.include(from(decl, 1)),
                        Some("export") => {
                            for spec in from(decl, 1) {
                                let parts: Vec<(String, Span)> = match list(spec) {
                                    Some([_, inside, outside]) => [inside, outside].into_iter().filter_map(ident).collect(),
                                    _ => ident(spec).into_iter().collect(),
                                };
                                if let Some((inside, s)) = parts.first() {
                                    self.reference(inside, *s, true);
                                    let outside = parts.last().map_or(inside.clone(), |(o, _)| o.clone());
                                    exports.push((inside.clone(), outside));
                                }
                            }
                        }
                        _ => {}
                    }
                }
                self.a.libraries.push((name, exports));
            }
            // Requirements are features, not variables.
            Some("cond-expand") => {
                for clause in from(items, 1).iter().filter_map(list) {
                    self.walk_all(from(clause, 1));
                }
            }
            Some("define") => match items.get(1) {
                Some(sig) if list(sig).is_some() => {
                    // The name is bound by collect_defs; parameters scope the body.
                    self.bind_formals_after_name(sig, span);
                    self.body(from(items, 2), span);
                }
                _ => self.walk_all(from(items, 2)),
            },
            Some("lambda") => {
                if let Some(f) = items.get(1) {
                    self.bind_formals(f, span);
                }
                self.body(from(items, 2), span);
            }
            Some(form @ ("let" | "let*" | "letrec" | "letrec*")) => {
                let named = form == "let" && items.get(1).is_some_and(|x| x.sym().is_some());
                let (bindings, body) = if named { (items.get(2), from(items, 3)) } else { (items.get(1), from(items, 2)) };
                let pairs: Vec<&[Syntax]> = bindings.and_then(list).unwrap_or(&[]).iter().filter_map(list).collect();
                let init = |w: &mut Self, p: &[Syntax]| {
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
                let specs: Vec<&[Syntax]> = items.get(1).and_then(list).unwrap_or(&[]).iter().filter_map(list).collect();
                for s in &specs {
                    if let Some(init) = s.get(1) {
                        self.walk(init);
                    }
                }
                for s in &specs {
                    self.bind_local(&s[0], span);
                }
                for s in &specs {
                    self.walk_all(from(s, 2));
                }
                self.walk_all(from(items, 2));
            }
            Some("receive") => {
                if let Some(e) = items.get(2) {
                    self.walk(e);
                }
                if let Some(f) = items.get(1) {
                    self.bind_formals(f, span);
                }
                self.body(from(items, 3), span);
            }
            Some("let-values" | "let*-values") => {
                for b in items.get(1).and_then(list).unwrap_or(&[]).iter().filter_map(list) {
                    if let Some(e) = b.get(1) {
                        self.walk(e);
                    }
                    self.bind_formals(&b[0], span);
                }
                self.body(from(items, 2), span);
            }
            Some("define-values") => {
                if let Some(e) = items.get(2) {
                    self.walk(e);
                }
            }
            Some("case-lambda") => {
                for clause in from(items, 1).iter().filter_map(list) {
                    let m = self.env.len();
                    if let Some(f) = clause.first() {
                        self.bind_formals(f, span);
                    }
                    self.body(from(clause, 1), span);
                    self.env.truncate(m);
                }
            }
            Some("define-record-type" | "define-generic") => {}
            Some("define-method") => {
                if let Some(sig) = items.get(1).and_then(list) {
                    if let Some((n, s)) = sig.first().and_then(ident) {
                        self.reference(&n, s, true);
                    }
                    for p in from(sig, 1) {
                        match list(p) {
                            Some([var, ty]) => {
                                if let Some((tn, ts)) = ident(ty)
                                    && !TYPE_NAMES.contains(&tn.as_str())
                                {
                                    self.reference(&tn, ts, true);
                                }
                                self.bind_local(var, span);
                            }
                            _ => self.bind_local(p, span),
                        }
                    }
                }
                self.body(from(items, 2), span);
            }
            Some("guard") => {
                self.body(from(items, 2), span);
                if let Some(spec) = items.get(1).and_then(list) {
                    let m = self.env.len();
                    if let Some(v) = spec.first() {
                        self.bind_local(v, span);
                    }
                    for clause in from(spec, 1).iter().filter_map(list) {
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
                for clause in from(items, 2).iter().filter_map(list) {
                    let m = self.env.len();
                    if let Some(f) = clause.get(1) {
                        self.bind_formals(f, span);
                    }
                    self.body(from(clause, 2), span);
                    self.env.truncate(m);
                }
            }
            Some("match-let") => {
                let pairs: Vec<&[Syntax]> = items.get(1).and_then(list).unwrap_or(&[]).iter().filter_map(list).collect();
                for p in &pairs {
                    if let Some(e) = p.get(1) {
                        self.walk(e);
                    }
                }
                for p in &pairs {
                    self.bind_pattern(&p[0], span, false);
                }
                self.body(from(items, 2), span);
            }
            Some("case") => {
                if let Some(k) = items.get(1) {
                    self.walk(k);
                }
                for clause in from(items, 2).iter().filter_map(list) {
                    self.walk_all(from(clause, 1));
                }
            }
            Some(m @ ("match" | "match-lambda")) => {
                let clauses = if m == "match" {
                    if let Some(subject) = items.get(1) {
                        self.walk(subject);
                    }
                    from(items, 2)
                } else {
                    from(items, 1)
                };
                for clause in clauses.iter().filter_map(list) {
                    let m = self.env.len();
                    if let Some(pat) = clause.first() {
                        self.bind_pattern(pat, span, false);
                    }
                    self.walk_all(from(clause, 1));
                    self.env.truncate(m);
                }
            }
            Some(name) if self.is_macro(name) => {
                // The head is a known macro; what it binds is unknown.
                let (n, s) = ident(&items[0]).unwrap();
                self.reference(&n, s, false);
                self.in_macro += 1;
                self.walk_all(from(items, 1));
                self.in_macro -= 1;
            }
            _ => self.walk_all(items),
        }
        self.env.truncate(mark);
    }

    /// The parameters of `(define (name . params) ...)`, also curried
    /// (`(define ((f a) b) ...)`): everything but the innermost name.
    fn bind_formals_after_name(&mut self, sig: &Syntax, scope: Span) {
        let SyntaxKind::List(items, tail) = &sig.kind else { return };
        if let Some(inner) = items.first().filter(|f| list(f).is_some()) {
            self.bind_formals_after_name(inner, scope);
        }
        self.bind_params(from(items, 1), scope);
        if let Some(t) = tail {
            self.bind_local(t, scope);
        }
    }

    /// Pattern variables of a `match` pattern. List heads (`list`, `cons`,
    /// `?`, record types) are constructors; predicates are references.
    fn bind_pattern(&mut self, pat: &Syntax, scope: Span, in_head: bool) {
        match &pat.kind {
            SyntaxKind::Atom(Sexp::Sym(_)) => {
                let (n, s) = ident(pat).unwrap();
                if in_head {
                    if !matches!(n.as_str(), "list" | "cons" | "vector" | "?" | "and" | "or" | "not") {
                        self.reference(&n, s, false);
                    }
                } else if !SYNTAX_WORDS.contains(&n.as_str()) {
                    self.bind_local(pat, scope);
                }
            }
            SyntaxKind::List(items, _) => {
                if head(items).as_deref() == Some("quote") {
                    return;
                }
                let is_pred = head(items).as_deref() == Some("?");
                for (i, sub) in items.iter().enumerate() {
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

/// What checkdoc finds in a file (runtime/TECHNE-VM.md, "Docstrings"):
/// each name the file provides, or a library it defines exports, without
/// a docstring or with one that breaks the convention, and the docstrings
/// of the editor's definitions (`define-command`, `define-mode`...) that
/// do. Each is a message and the span it is about.
pub fn checkdoc(source: &str, a: &Analysis) -> Vec<(String, Span)> {
    use techne_vm::doc::{Subject, param_names, problems};
    let exported: HashSet<&str> = a
        .provides
        .iter()
        .map(String::as_str)
        .chain(a.libraries.iter().flat_map(|(_, e)| e.iter().map(|(inner, _)| inner.as_str())))
        .collect();
    let mut out = Vec::new();
    let mut check = |name: &str, doc: Option<&str>, subject: Subject, params: &[String], span: Span| match doc {
        None => out.push((format!("Document `{name}` with a docstring."), span)),
        Some(doc) => out.extend(problems(doc, subject, &param_names(params)).into_iter().map(|p| (format!("{name}: {p}"), span))),
    };
    for def in a.top_level().filter(|d| exported.contains(d.name.as_str()) && !d.name.starts_with('%')) {
        let subject = if def.kind == DefKind::Function { Subject::Procedure } else { Subject::Other };
        let params = def.signature.as_deref().map(signature_params).unwrap_or_default();
        check(&def.name, def.doc.as_deref(), subject, &params, def.span);
    }
    // The editor's definitions take their docstring as an argument.
    let Ok(forms) = read_syntax(source) else { return out };
    let mut forms: Vec<&Syntax> = forms.iter().collect();
    while let Some(form) = forms.pop() {
        let Some(items) = list(form) else { continue };
        let (subject, name_at, doc_at) = match head(items).as_deref() {
            Some("begin") => {
                forms.extend(&items[1..]);
                continue;
            }
            Some("define-command" | "define-view") => (Subject::Command, 1, 2),
            Some("define-action") => (Subject::Command, 2, 3),
            Some("define-mode" | "define-minor-mode" | "define-hook") => (Subject::Other, 1, 2),
            Some("define-option") => (Subject::Other, 1, 3),
            _ => continue,
        };
        let name = items.get(name_at).map(|n| list(n).and_then(|l| l.first()).unwrap_or(n));
        let (Some((name, span)), Some(doc)) = (name.and_then(ident), items.get(doc_at)) else { continue };
        // A docstring computed by code is not checked.
        if let Some(doc) = string_lit(doc) {
            check(&name, Some(&doc), subject, &[], span);
        }
    }
    out
}

/// The parameters of a signature such as `(f a [b 1] #:k k . rest)`, as
/// written.
fn signature_params(sig: &str) -> Vec<String> {
    let Ok(forms) = read_syntax(sig) else { return vec![] };
    let Some(SyntaxKind::List(items, tail)) = forms.first().map(|f| &f.kind) else { return vec![] };
    let text = |s: &Syntax| sig[s.span.0 as usize..s.span.1 as usize].to_string();
    items.iter().skip(1).map(text).chain(tail.iter().map(|t| format!(". {}", text(t)))).collect()
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
        let a = analyze(src, &HashSet::new());
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
    fn reads_as_the_vm_does() {
        // Syntax Steel's lexer got wrong: `fn` is an ordinary name, `+x` one
        // identifier, `|a b|` one symbol; datum comments and labels read.
        let src = "(define (fn +x) (list +x '|a b| #;(ignored) '#0=(1 . #0#)))\n(fn 1)";
        let a = analyze(src, &HashSet::new());
        assert!(a.error.is_none(), "{:?}", a.error);
        let (_, d) = a.at(src.rfind("fn 1").unwrap() as u32).unwrap();
        assert_eq!(d.unwrap().name, "fn");
        let (_, d) = a.at(src.rfind("+x)").unwrap() as u32).unwrap();
        assert_eq!(d.unwrap().kind, DefKind::Local);
        let known: HashSet<String> = ["list"].iter().map(|s| s.to_string()).collect();
        assert_eq!(unbound(&a, &known).count(), 0);
    }

    #[test]
    fn macro_uses_are_not_reported_unbound() {
        // A macro that binds `it`: references inside its use resolve where
        // they can but are not reported.
        let src =
            "(define-syntax with-it (syntax-rules () \"Bind IT.\" ((_ e body) (let ((it e)) body))))\n(with-it 1 (+ it 1))\n(my-macro y)";
        let macros: HashSet<String> = ["my-macro".to_string()].into();
        let a = analyze(src, &macros);
        let known: HashSet<String> = ["+"].iter().map(|s| s.to_string()).collect();
        assert_eq!(unbound(&a, &known).map(|r| r.name.as_str()).collect::<Vec<_>>(), Vec::<&str>::new());
        let (_, d) = a.at(src.find("with-it 1").unwrap() as u32).unwrap();
        assert_eq!(d.unwrap().kind, DefKind::Macro);
        assert_eq!(d.unwrap().doc.as_deref(), Some("Bind IT."));
    }

    #[test]
    fn checkdoc_finds_what_is_provided_without_a_docstring() {
        let src = "(provide f g h)\n(define (f x) \"Return X.\" x)\n(define (g x) x)\n(define (h a [b 1]) \"returns A\" a)\n(define (k) 1)\n\
                   (define-command (save-it s n) \"Saves it.\" #t)";
        let a = analyze(src, &HashSet::new());
        let found: Vec<String> = checkdoc(src, &a).into_iter().map(|(m, _)| m).collect();
        assert_eq!(
            found,
            [
                "Document `g` with a docstring.",
                "h: Start the first line with a capital letter.",
                "h: Make the first line a complete sentence, ending with a period.",
                "h: Use the imperative: \"Return\", not \"returns\".",
                "h: Name the parameter B in the docstring.",
                "save-it: Use the imperative: \"Save\", not \"Saves\".",
            ]
        );
    }

    #[test]
    fn libraries() {
        let src =
            "(define-library (lib) (export f (rename g h)) (import (scheme base)) (begin (define (f) (g)) (define (g) 1)))\n(import (lib))";
        let a = analyze(src, &HashSet::new());
        let known: HashSet<String> = HashSet::new();
        assert_eq!(unbound(&a, &known).map(|r| r.name.as_str()).collect::<Vec<_>>(), Vec::<&str>::new());
        assert!(a.top_level().any(|d| d.name == "g"));
    }

    #[test]
    fn reports_reader_errors() {
        let a = analyze("(define (f x)\n  (+ x 1)", &HashSet::new());
        let (msg, pos) = a.error.unwrap();
        assert!(msg.contains("not closed"), "{msg}");
        assert_eq!(pos, 0);
    }
}
