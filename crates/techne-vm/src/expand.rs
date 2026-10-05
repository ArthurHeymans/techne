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
}

#[derive(Clone, Debug)]
enum Bound {
    One(Sexp),
    Many(Vec<Bound>),
}

type Binds = FxHashMap<u32, Bound>;

impl Macro {
    /// Parse `(syntax-rules [ellipsis] (literal ...) (pattern template) ...)`.
    pub fn parse(name: u32, spec: &Sexp, env_depth: usize, module: u32) -> Result<Macro, String> {
        let items = spec.list().ok_or("syntax-rules: expected a list")?;
        if items.first().is_none_or(|h| !h.is_sym("syntax-rules")) {
            return Err("only syntax-rules transformers are supported".into());
        }
        let (ellipsis, rest) = match items.get(1) {
            Some(Sexp::Sym(e)) => (*e, &items[2..]),
            _ => (intern("..."), &items[1..]),
        };
        let literals = rest
            .first()
            .and_then(|l| l.list())
            .ok_or("syntax-rules: expected a literal list")?
            .iter()
            .map(|l| l.sym().ok_or_else(|| "syntax-rules: literals must be symbols".to_string()))
            .collect::<Result<_, _>>()?;
        let rules = rest[1..]
            .iter()
            .map(|r| match r.list() {
                Some([pattern, template]) => Ok((pattern.clone(), template.clone())),
                _ => Err(format!("{}: malformed syntax rule", symbol_name(name))),
            })
            .collect::<Result<_, _>>()?;
        Ok(Macro { name, ellipsis, literals, rules, env_depth, module })
    }

    /// Expand a use of this macro. `same_literal(input, literal)` decides
    /// whether an input identifier denotes the literal (free-identifier=?).
    pub fn expand(&self, form: &Sexp, same_literal: &dyn Fn(u32, u32) -> bool) -> Result<Sexp, String> {
        let input = form.list().ok_or("macro use must be a list")?;
        for (pattern, template) in &self.rules {
            let Some(pat) = pattern.list() else { continue };
            let mut binds = Binds::default();
            // The keyword position is ignored.
            let pat_rest = Sexp::List(pat[1..].to_vec(), tail_of(pattern), pattern.pos());
            let in_rest = Sexp::List(input[1..].to_vec(), None, form.pos());
            if self.matches(&pat_rest, &in_rest, &mut binds, same_literal) {
                let mut renames = FxHashMap::default();
                return self.instantiate(template, &binds, &mut renames, form.pos());
            }
        }
        Err(format!("{}: no syntax rule matches {}", symbol_name(self.name), crate::reader::display_sexp(form)))
    }

    fn is_ellipsis(&self, s: &Sexp) -> bool {
        s.sym() == Some(self.ellipsis)
    }

    fn matches(&self, pat: &Sexp, form: &Sexp, binds: &mut Binds, lit: &dyn Fn(u32, u32) -> bool) -> bool {
        match pat {
            Sexp::Sym(p) if symbol_name(*p).as_ref() == "_" => true,
            Sexp::Sym(p) if self.literals.contains(p) => matches!(form, Sexp::Sym(f) if lit(*f, *p)),
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
            Sexp::Sym(p) if !self.literals.contains(p) && *p != self.ellipsis && symbol_name(*p).as_ref() != "_" => {
                out.push(*p)
            }
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

    fn instantiate(&self, t: &Sexp, binds: &Binds, renames: &mut FxHashMap<u32, u32>, pos: Pos) -> Result<Sexp, String> {
        match t {
            Sexp::Sym(s) => match binds.get(s) {
                Some(Bound::One(v)) => Ok(v.clone()),
                Some(Bound::Many(_)) => Err(format!("{}: pattern variable {} used without ellipsis", symbol_name(self.name), symbol_name(*s))),
                None => Ok(Sexp::Sym(*renames.entry(*s).or_insert_with(|| make_alias(*s, self.env_depth, self.module)))),
            },
            // (... template): ellipses inside are literal.
            Sexp::List(items, None, _) if items.len() == 2 && self.is_ellipsis(&items[0]) => {
                Ok(self.literal_template(&items[1], renames))
            }
            Sexp::List(items, tail, _) => {
                let out = self.instantiate_seq(items, binds, renames, pos)?;
                let tail = tail.as_ref().map(|t| self.instantiate(t, binds, renames, pos).map(Box::new)).transpose()?;
                Ok(Sexp::List(out, tail, pos))
            }
            Sexp::Vector(items) => Ok(Sexp::Vector(self.instantiate_seq(items, binds, renames, pos)?)),
            datum => Ok(datum.clone()),
        }
    }

    fn instantiate_seq(&self, items: &[Sexp], binds: &Binds, renames: &mut FxHashMap<u32, u32>, pos: Pos) -> Result<Vec<Sexp>, String> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < items.len() {
            let item = &items[i];
            let depth = items[i + 1..].iter().take_while(|x| self.is_ellipsis(x)).count();
            if depth == 0 {
                out.push(self.instantiate(item, binds, renames, pos)?);
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
                out.push(self.instantiate(item, &b, renames, pos)?);
            }
        }
        Ok(out)
    }

    fn literal_template(&self, t: &Sexp, renames: &mut FxHashMap<u32, u32>) -> Sexp {
        match t {
            Sexp::Sym(s) if *s == self.ellipsis => t.clone(),
            Sexp::Sym(s) => Sexp::Sym(*renames.entry(*s).or_insert_with(|| make_alias(*s, self.env_depth, self.module))),
            Sexp::List(items, tail, p) => Sexp::List(
                items.iter().map(|i| self.literal_template(i, renames)).collect(),
                tail.as_ref().map(|x| Box::new(self.literal_template(x, renames))),
                *p,
            ),
            other => other.clone(),
        }
    }
}

fn tail_of(s: &Sexp) -> Option<Box<Sexp>> {
    match s {
        Sexp::List(_, t, _) => t.clone(),
        _ => None,
    }
}

fn rest_list(items: &[Sexp], tail: Option<&Sexp>) -> Sexp {
    match (items, tail) {
        ([], Some(t)) => t.clone(),
        _ => Sexp::List(items.to_vec(), tail.map(|t| Box::new(t.clone())), crate::reader::NO_POS),
    }
}
