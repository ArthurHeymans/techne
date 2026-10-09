//! `syntax-rules` macros.
//!
//! Hygiene follows Clinger and Rees' renaming: every identifier a template
//! introduces is replaced by a fresh alias (`reader::make_alias`) per expansion.
//! The compiler resolves an alias bound inside the expansion as a new variable,
//! and a free alias as the original identifier in the macro's definition
//! environment, so neither side can capture the other's names.

use rustc_hash::FxHashMap;

use crate::reader::{Pos, Sexp, intern, make_alias, symbol_name};

pub struct Macro {
    pub name: u32,
    ellipsis: u32,
    literals: Vec<u32>,
    rules: Vec<(Sexp, Sexp)>,
    /// Lexical depth of the definition environment (0 for global macros).
    pub env_depth: usize,
    pub module: u32,
    /// The other modules its rules refer to: those of the aliases in them,
    /// when a macro's expansion defines it.
    pub modules: Box<[u32]>,
    /// The docstring after the literals, as Guile has it:
    /// `(syntax-rules (literal ...) "doc" (pattern template) ...)`.
    pub doc: Option<std::rc::Rc<str>>,
    /// Where it is defined: its source file (shared, as the VM may free
    /// its slot first, `Vm::release_file`) and the position of its
    /// `syntax-rules`.
    pub file: crate::vm::SourceFile,
    pub pos: Pos,
}

#[derive(Clone, Debug)]
enum Bound {
    One(Sexp),
    Many(Vec<Bound>),
}

type Binds = FxHashMap<u32, Bound>;

/// Add the modules of the aliases in `s`, along their chains, to `out`.
fn alias_modules(s: &Sexp, out: &mut Vec<u32>) {
    match s {
        Sexp::Sym(id) => {
            let mut sym = *id;
            while let Some(a) = crate::reader::alias(sym) {
                out.push(a.module);
                sym = a.orig;
            }
        }
        Sexp::List(items, tail, _) => crate::nested(|| items.iter().chain(tail.as_deref()).for_each(|i| alias_modules(i, out))),
        Sexp::Vector(items) => crate::nested(|| items.iter().for_each(|i| alias_modules(i, out))),
        Sexp::Labeled(_, d) => alias_modules(d, out),
        _ => {}
    }
}

impl Macro {
    /// Bytes it holds (`Vm::held`), besides its file's source text.
    pub fn bytes(&self) -> usize {
        use std::mem::size_of;
        let rules = self.rules.iter().map(|(p, t)| p.bytes() + t.bytes()).sum::<usize>();
        size_of::<Macro>()
            + self.literals.capacity() * size_of::<u32>()
            + self.rules.capacity() * size_of::<(Sexp, Sexp)>()
            + rules
            + self.modules.len() * size_of::<u32>()
            + self.doc.as_ref().map_or(0, |d| d.len())
    }

    /// Parse `(syntax-rules [ellipsis] (literal ...) [doc] (pattern template) ...)`
    /// from `file`.
    pub fn parse(name: u32, spec: &Sexp, env_depth: usize, module: u32, file: &crate::vm::SourceFile) -> Result<Macro, String> {
        let items = spec.list().ok_or("syntax-rules: expected a list")?;
        if items.first().is_none_or(|h| !h.is_sym("syntax-rules")) {
            return Err("only syntax-rules transformers are supported".into());
        }
        let (ellipsis, rest) = match items.get(1) {
            Some(Sexp::Sym(e)) => (*e, &items[2..]),
            _ => (intern("..."), &items[1..]),
        };
        let literals: Vec<u32> = rest
            .first()
            .and_then(|l| l.list())
            .ok_or("syntax-rules: expected a literal list")?
            .iter()
            .map(|l| l.sym().ok_or_else(|| "syntax-rules: literals must be symbols".to_string()))
            .collect::<Result<_, _>>()?;
        // An ellipsis listed as a literal is only a literal.
        let ellipsis = if literals.contains(&ellipsis) { intern("\u{1f}no ellipsis") } else { ellipsis };
        let (doc, rules) = match &rest[1..] {
            [Sexp::Str(doc), rules @ ..] => (Some(doc.clone()), rules),
            rules => (None, rules),
        };
        let rules = rules
            .iter()
            .map(|r| match r.list() {
                Some([pattern, template]) => Ok((pattern.clone(), template.clone())),
                _ => Err(format!("{}: malformed syntax rule", symbol_name(name))),
            })
            .collect::<Result<Vec<(Sexp, Sexp)>, _>>()?;
        let mut modules = Vec::new();
        rules.iter().for_each(|(p, t)| [p, t].into_iter().for_each(|s| alias_modules(s, &mut modules)));
        modules.retain(|&m| m != module);
        modules.sort_unstable();
        modules.dedup();
        let modules = modules.into();
        let m = Macro { name, ellipsis, literals, rules, env_depth, module, modules, doc, file: file.clone(), pos: spec.pos() };
        for (pattern, _) in &m.rules {
            // The keyword position is ignored.
            if let Sexp::List(items, tail, _) = pattern
                && let Some(rest) = items.get(1..)
            {
                m.check_pattern(&rest_list(rest, tail.as_deref()), &mut Vec::new())?;
            }
        }
        Ok(m)
    }

    /// A pattern binds each variable once and has at most one ellipsis per
    /// sequence, after an element: what matching relies on.
    fn check_pattern(&self, pat: &Sexp, seen: &mut Vec<u32>) -> Result<(), String> {
        match pat {
            Sexp::List(..) | Sexp::Vector(_) => crate::nested(|| self.check_pattern_step(pat, seen)),
            _ => self.check_pattern_step(pat, seen),
        }
    }

    fn check_pattern_step(&self, pat: &Sexp, seen: &mut Vec<u32>) -> Result<(), String> {
        let who = symbol_name(self.name);
        match pat {
            Sexp::Sym(_) if self.is_ellipsis(pat) => Err(format!("{who}: an ellipsis must follow a pattern")),
            Sexp::Sym(_) if self.pattern_vars(pat).is_empty() => Ok(()),
            Sexp::Sym(p) if seen.contains(p) => Err(format!("{who}: pattern variable {} appears more than once", symbol_name(*p))),
            Sexp::Sym(p) => {
                seen.push(*p);
                Ok(())
            }
            Sexp::List(items, _, _) | Sexp::Vector(items) => {
                let ellipses: Vec<usize> = items.iter().enumerate().filter(|(_, i)| self.is_ellipsis(i)).map(|(k, _)| k).collect();
                match ellipses[..] {
                    [_, _, ..] => return Err(format!("{who}: more than one ellipsis in a pattern sequence")),
                    [0] => return Err(format!("{who}: an ellipsis must follow a pattern")),
                    _ => {}
                }
                let tail = match pat {
                    Sexp::List(_, tail, _) => tail.as_deref(),
                    _ => None,
                };
                items.iter().filter(|i| !self.is_ellipsis(i)).chain(tail).try_for_each(|i| self.check_pattern(i, seen))
            }
            _ => Ok(()),
        }
    }

    /// Expand a use of this macro. `same_literal(input, literal)` decides
    /// whether an input identifier denotes the literal (free-identifier=?).
    pub fn expand(&self, form: &Sexp, same_literal: &dyn Fn(u32, u32) -> bool) -> Result<Sexp, String> {
        let Sexp::List(input, in_tail, _) = form else { return Err("macro use must be a list".into()) };
        for (pattern, template) in &self.rules {
            let Sexp::List(pat, pat_tail, _) = pattern else { continue };
            if pat.is_empty() || input.is_empty() {
                continue;
            }
            let mut binds = Binds::default();
            // The keyword position is ignored.
            let pat_rest = rest_list(&pat[1..], pat_tail.as_deref());
            let in_rest = rest_list(&input[1..], in_tail.as_deref());
            if self.matches(&pat_rest, &in_rest, &mut binds, same_literal) {
                let mut renames = FxHashMap::default();
                return self.instantiate(template, &binds, &mut renames, form.pos(), true);
            }
        }
        Err(format!("{}: no syntax rule matches {}", symbol_name(self.name), crate::reader::display_sexp(form)))
    }

    fn is_ellipsis(&self, s: &Sexp) -> bool {
        s.sym() == Some(self.ellipsis)
    }

    fn matches(&self, pat: &Sexp, form: &Sexp, binds: &mut Binds, lit: &dyn Fn(u32, u32) -> bool) -> bool {
        match pat {
            Sexp::Sym(_) => self.matches_step(pat, form, binds, lit),
            _ => crate::nested(|| self.matches_step(pat, form, binds, lit)),
        }
    }

    fn matches_step(&self, pat: &Sexp, form: &Sexp, binds: &mut Binds, lit: &dyn Fn(u32, u32) -> bool) -> bool {
        match pat {
            Sexp::Sym(p) if self.literals.contains(p) => matches!(form, Sexp::Sym(f) if lit(*f, *p)),
            Sexp::Sym(p) if symbol_name(*p).as_ref() == "_" => true,
            Sexp::Sym(p) => {
                binds.insert(*p, Bound::One(form.clone()));
                true
            }
            Sexp::List(items, tail, _) => {
                let (fitems, ftail): (&[Sexp], Option<&Sexp>) = match form {
                    Sexp::List(fi, ft, _) => (fi, ft.as_deref()),
                    _ => return false,
                };
                self.match_seq(items, tail.as_deref(), fitems, ftail, binds, lit)
            }
            Sexp::Vector(items) => match form {
                Sexp::Vector(fitems) => self.match_seq(items, None, fitems, None, binds, lit),
                _ => false,
            },
            datum => datum == form,
        }
    }

    fn match_seq(
        &self,
        pats: &[Sexp],
        ptail: Option<&Sexp>,
        forms: &[Sexp],
        ftail: Option<&Sexp>,
        binds: &mut Binds,
        lit: &dyn Fn(u32, u32) -> bool,
    ) -> bool {
        let Some(e) = pats.iter().position(|p| self.is_ellipsis(p)) else {
            // No ellipsis: element-wise, then the tail.
            if forms.len() < pats.len() {
                return false;
            }
            if !pats.iter().zip(forms).all(|(p, f)| self.matches(p, f, binds, lit)) {
                return false;
            }
            let rest = &forms[pats.len()..];
            return match ptail {
                Some(t) => self.matches(t, &rest_list(rest, ftail), binds, lit),
                None => rest.is_empty() && ftail.is_none(),
            };
        };
        if e == 0 {
            return false;
        }
        let (prefix, repeated, suffix) = (&pats[..e - 1], &pats[e - 1], &pats[e + 1..]);
        if forms.len() < prefix.len() + suffix.len() || (ptail.is_none() && ftail.is_some()) {
            return false;
        }
        let count = forms.len() - prefix.len() - suffix.len();
        if !prefix.iter().zip(forms).all(|(p, f)| self.matches(p, f, binds, lit)) {
            return false;
        }
        let mut reps = Vec::with_capacity(count);
        for f in &forms[prefix.len()..prefix.len() + count] {
            let mut b = Binds::default();
            if !self.matches(repeated, f, &mut b, lit) {
                return false;
            }
            reps.push(b);
        }
        for v in self.pattern_vars(repeated) {
            let seq = reps.iter_mut().map(|b| b.remove(&v).expect("bound in every repetition")).collect();
            binds.insert(v, Bound::Many(seq));
        }
        let rest = &forms[prefix.len() + count..];
        if !suffix.iter().zip(rest).all(|(p, f)| self.matches(p, f, binds, lit)) {
            return false;
        }
        match ptail {
            Some(t) => self.matches(t, &rest_list(&rest[suffix.len()..], ftail), binds, lit),
            None => true,
        }
    }

    fn pattern_vars(&self, pat: &Sexp) -> Vec<u32> {
        let mut out = Vec::new();
        self.collect_vars(pat, &mut out);
        out
    }

    fn collect_vars(&self, pat: &Sexp, out: &mut Vec<u32>) {
        match pat {
            Sexp::List(..) | Sexp::Vector(_) => crate::nested(|| self.collect_vars_step(pat, out)),
            _ => self.collect_vars_step(pat, out),
        }
    }

    fn collect_vars_step(&self, pat: &Sexp, out: &mut Vec<u32>) {
        match pat {
            Sexp::Sym(p) if !self.literals.contains(p) && *p != self.ellipsis && symbol_name(*p).as_ref() != "_" => out.push(*p),
            Sexp::List(items, tail, _) => {
                items.iter().for_each(|i| self.collect_vars(i, out));
                if let Some(t) = tail {
                    self.collect_vars(t, out);
                }
            }
            Sexp::Vector(items) => items.iter().for_each(|i| self.collect_vars(i, out)),
            _ => {}
        }
    }

    /// Instantiates a template; with `ellipsis` false (inside `(... t)`),
    /// ellipses are plain identifiers.
    fn instantiate(&self, t: &Sexp, binds: &Binds, renames: &mut FxHashMap<u32, u32>, pos: Pos, ellipsis: bool) -> Result<Sexp, String> {
        match t {
            Sexp::List(..) | Sexp::Vector(_) => crate::nested(|| self.instantiate_step(t, binds, renames, pos, ellipsis)),
            _ => self.instantiate_step(t, binds, renames, pos, ellipsis),
        }
    }

    fn instantiate_step(
        &self,
        t: &Sexp,
        binds: &Binds,
        renames: &mut FxHashMap<u32, u32>,
        pos: Pos,
        ellipsis: bool,
    ) -> Result<Sexp, String> {
        match t {
            Sexp::Sym(s) if !ellipsis && self.is_ellipsis(t) => Ok(Sexp::Sym(*s)),
            Sexp::Sym(s) => match binds.get(s) {
                Some(Bound::One(v)) => Ok(v.clone()),
                Some(Bound::Many(_)) => {
                    Err(format!("{}: pattern variable {} used without ellipsis", symbol_name(self.name), symbol_name(*s)))
                }
                None => Ok(Sexp::Sym(*renames.entry(*s).or_insert_with(|| make_alias(*s, self.env_depth, self.module)))),
            },
            Sexp::List(items, None, _) if ellipsis && items.len() == 2 && self.is_ellipsis(&items[0]) => {
                self.instantiate(&items[1], binds, renames, pos, false)
            }
            Sexp::List(items, tail, _) => {
                let mut out = self.instantiate_seq(items, binds, renames, pos, ellipsis)?;
                let mut tail = tail.as_ref().map(|t| self.instantiate(t, binds, renames, pos, ellipsis)).transpose()?;
                // A list in the tail (`(f x . args)` with ARGS a list) continues this one.
                while let Some(t) = tail {
                    match t.into_list() {
                        Ok((more, more_tail, _)) => {
                            out.extend(more);
                            tail = more_tail.map(|t| *t);
                        }
                        Err(t) => {
                            tail = Some(t);
                            break;
                        }
                    }
                }
                Ok(Sexp::List(out, tail.map(Box::new), pos))
            }
            Sexp::Vector(items) => Ok(Sexp::Vector(self.instantiate_seq(items, binds, renames, pos, ellipsis)?)),
            datum => Ok(datum.clone()),
        }
    }

    fn instantiate_seq(
        &self,
        items: &[Sexp],
        binds: &Binds,
        renames: &mut FxHashMap<u32, u32>,
        pos: Pos,
        ellipsis: bool,
    ) -> Result<Vec<Sexp>, String> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < items.len() {
            let item = &items[i];
            let depth = if ellipsis { items[i + 1..].iter().take_while(|x| self.is_ellipsis(x)).count() } else { 0 };
            if depth == 0 {
                out.push(self.instantiate(item, binds, renames, pos, ellipsis)?);
            } else {
                out.extend(self.repeat(item, depth, binds, renames, pos)?);
            }
            i += 1 + depth;
        }
        Ok(out)
    }

    /// Expand `item` followed by `depth` ellipses.
    fn repeat(&self, item: &Sexp, depth: usize, binds: &Binds, renames: &mut FxHashMap<u32, u32>, pos: Pos) -> Result<Vec<Sexp>, String> {
        let vars: Vec<u32> = self.pattern_vars(item).into_iter().filter(|v| matches!(binds.get(v), Some(Bound::Many(_)))).collect();
        if vars.is_empty() {
            return Err(format!("{}: ellipsis follows a template without pattern variables", symbol_name(self.name)));
        }
        let len = vars
            .iter()
            .map(|v| match &binds[v] {
                Bound::Many(xs) => xs.len(),
                Bound::One(_) => 0,
            })
            .max()
            .unwrap_or(0);
        let mut out = Vec::new();
        for k in 0..len {
            let mut b = binds.clone();
            for v in &vars {
                if let Bound::Many(xs) = &binds[v] {
                    match xs.get(k) {
                        Some(x) => {
                            b.insert(*v, x.clone());
                        }
                        None => return Err(format!("{}: mismatched ellipsis lengths", symbol_name(self.name))),
                    }
                }
            }
            if depth > 1 {
                out.extend(self.repeat(item, depth - 1, &b, renames, pos)?);
            } else {
                out.push(self.instantiate(item, &b, renames, pos, true)?);
            }
        }
        Ok(out)
    }
}

fn rest_list(items: &[Sexp], tail: Option<&Sexp>) -> Sexp {
    match (items, tail) {
        ([], Some(t)) => t.clone(),
        _ => Sexp::List(items.to_vec(), tail.map(|t| Box::new(t.clone())), crate::reader::NO_POS),
    }
}
