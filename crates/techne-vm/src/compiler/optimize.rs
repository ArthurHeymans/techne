//! Bounded, resolved-tree experiments. No assumptions about mutable globals.
//! TECHNE_IR selects comma-separated passes (or 0/all) once per process.
//! Disabled by default pending representative workload and GC-pause wins.
//! See runtime/bench/ir/END-TO-END.md and CACHED.md for measured tradeoffs.

use super::*;
use rustc_hash::FxHashSet;
use std::sync::OnceLock;

const BINDINGS: u16 = 1;
const LOCAL: u16 = 2;
const CONTIFY: u16 = 4;
const SCALAR: u16 = 8;
const CSE: u16 = 16;
const LICM: u16 = 32;
const DEAD: u16 = 64;
const HOIST: u16 = 128;
const ROOT: u16 = 256;

fn flags() -> u16 {
    static FLAGS: OnceLock<u16> = OnceLock::new();
    *FLAGS.get_or_init(|| {
        let setting = std::env::var("TECHNE_IR").unwrap_or_else(|_| "0".into());
        setting.split(',').fold(0, |flags, name| {
            flags
                | match name {
                    "all" => 255,
                    "bindings" => BINDINGS,
                    "local" => LOCAL,
                    "contify" => CONTIFY,
                    "scalar" => SCALAR,
                    "cse" => CSE,
                    "licm" => LICM,
                    "dead" => DEAD,
                    "hoist" => HOIST,
                    "root" => ROOT,
                    "0" => 0,
                    _ => panic!("unknown TECHNE_IR pass {name}"),
                }
        })
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Atom {
    Local(VarId),
    Int(i64),
    Bool(bool),
    Char(char),
    Sym(u32),
    Nil,
}

impl Atom {
    fn of(e: &Expr) -> Option<Self> {
        Some(match e {
            Expr::Local(v) => Self::Local(*v),
            Expr::Const(Sexp::Int(n)) => Self::Int(*n),
            Expr::Const(Sexp::Bool(b)) => Self::Bool(*b),
            Expr::Const(Sexp::Char(c)) => Self::Char(*c),
            Expr::Const(Sexp::Sym(s)) => Self::Sym(*s),
            Expr::Const(Sexp::List(xs, None, _)) if xs.is_empty() => Self::Nil,
            _ => return None,
        })
    }
    fn expr(&self) -> Expr {
        match self {
            Self::Local(v) => Expr::Local(*v),
            Self::Int(n) => Expr::Const(Sexp::Int(*n)),
            Self::Bool(b) => Expr::Const(Sexp::Bool(*b)),
            Self::Char(c) => Expr::Const(Sexp::Char(*c)),
            Self::Sym(s) => Expr::Const(Sexp::Sym(*s)),
            Self::Nil => Expr::Const(Sexp::list_of(vec![])),
        }
    }
}

/// Immediate children, deliberately not following function IDs.
fn children<'a>(e: &'a Expr, f: &mut impl FnMut(&'a Expr)) {
    match e {
        Expr::Inlined { body, .. } => f(body),
        Expr::SetLocal(_, x) | Expr::SetGlobal(_, x) | Expr::DefGlobal(_, x) | Expr::Escape(x, _) | Expr::Tick(x) => f(x),
        Expr::If(c, t, a) => [c, t, a].into_iter().for_each(|e| f(e)),
        Expr::Seq(xs) | Expr::And(xs) | Expr::Or(xs) | Expr::Prim(_, xs, _) | Expr::LoopCall(_, xs) => xs.iter().for_each(f),
        Expr::Call(c, xs, _) | Expr::Loop(_, xs, c) => {
            f(c);
            xs.iter().for_each(f);
        }
        Expr::Let(bs, body) | Expr::Letrec(bs, body) => {
            bs.iter().for_each(|(_, e)| f(e));
            f(body);
        }
        Expr::Guard { body, handler, .. } => {
            f(body);
            f(handler);
        }
        _ => {}
    }
}

fn children_mut(e: &mut Expr, f: &mut impl FnMut(&mut Expr)) {
    match e {
        Expr::Inlined { body, .. } => f(body),
        Expr::SetLocal(_, x) | Expr::SetGlobal(_, x) | Expr::DefGlobal(_, x) | Expr::Escape(x, _) | Expr::Tick(x) => f(x),
        Expr::If(c, t, a) => [c, t, a].into_iter().for_each(|e| f(e)),
        Expr::Seq(xs) | Expr::And(xs) | Expr::Or(xs) | Expr::Prim(_, xs, _) | Expr::LoopCall(_, xs) => xs.iter_mut().for_each(f),
        Expr::Call(c, xs, _) | Expr::Loop(_, xs, c) => {
            f(c);
            xs.iter_mut().for_each(f);
        }
        Expr::Let(bs, body) | Expr::Letrec(bs, body) => {
            bs.iter_mut().for_each(|(_, e)| f(e));
            f(body);
        }
        Expr::Guard { body, handler, .. } => {
            f(body);
            f(handler);
        }
        _ => {}
    }
}

fn any(e: &Expr, test: &impl Fn(&Expr) -> bool) -> bool {
    crate::nested(|| {
        if test(e) {
            return true;
        }
        let mut found = false;
        children(e, &mut |e| found |= any(e, test));
        found
    })
}

fn uses(e: &Expr, v: VarId, c: &Compiler<'_>) -> bool {
    any(e, &|e| match e {
        Expr::Local(x) | Expr::SetLocal(x, _) => *x == v,
        Expr::Lambda(f) => c.funcs[*f].free.contains(&v),
        _ => false,
    })
}

fn integer(e: &Expr, ints: &FxHashSet<VarId>) -> bool {
    crate::nested(|| match e {
        Expr::Const(Sexp::Int(_)) => true,
        Expr::Local(v) => ints.contains(v),
        Expr::Prim(Prim::Add | Prim::Sub | Prim::Mul | Prim::Quo | Prim::Rem | Prim::Mod, xs, _) => xs.iter().all(|e| integer(e, ints)),
        _ => false,
    })
}

fn discardable(e: &Expr, ints: &FxHashSet<VarId>) -> bool {
    crate::nested(|| match e {
        Expr::Const(_) | Expr::Local(_) | Expr::Object(_) | Expr::Lambda(_) | Expr::Void => true,
        Expr::Prim(p, xs, _) => {
            let total = match p {
                Prim::Not | Prim::EqP | Prim::NullP | Prim::PairP => true,
                Prim::Add | Prim::Sub | Prim::Mul => xs.iter().all(|e| integer(e, ints)),
                _ => false,
            };
            total && xs.iter().all(|e| discardable(e, ints))
        }
        _ => false,
    })
}

/// Pure primitive trees, including reads, but no writes, calls or fresh identity.
/// These keys are only used for adjacent equal operands, not across effects.
#[derive(PartialEq)]
enum Key {
    Atom(Atom),
    Prim(Prim, Vec<Key>),
}
fn key(e: &Expr, depth: usize) -> Option<Key> {
    if depth == 0 {
        return None;
    }
    if let Some(a) = Atom::of(e) {
        return Some(Key::Atom(a));
    }
    match e {
        Expr::Prim(p, xs, _) if !matches!(p, Prim::Cons | Prim::VSet) => {
            Some(Key::Prim(*p, xs.iter().map(|e| key(e, depth - 1)).collect::<Option<_>>()?))
        }
        _ => None,
    }
}

/// Remember source tail position before a function's body is copied into a
/// non-tail register destination. Physical and logical frames then differ.
fn mark_tail(e: &mut Expr, tail: bool, c: &mut Compiler<'_>) -> R<()> {
    c.step()?;
    crate::nested(|| match e {
        Expr::Call(f, xs, site) => {
            site.tail = tail;
            mark_tail(f, false, c)?;
            xs.iter_mut().try_for_each(|e| mark_tail(e, false, c))
        }
        Expr::If(test, yes, no) => {
            mark_tail(test, false, c)?;
            mark_tail(yes, tail, c)?;
            mark_tail(no, tail, c)
        }
        Expr::Let(bs, body) | Expr::Letrec(bs, body) => {
            bs.iter_mut().try_for_each(|(_, e)| mark_tail(e, false, c))?;
            mark_tail(body, tail, c)
        }
        Expr::Seq(xs) | Expr::And(xs) | Expr::Or(xs) => {
            let n = xs.len();
            xs.iter_mut().enumerate().try_for_each(|(i, e)| mark_tail(e, tail && i + 1 == n, c))
        }
        Expr::Loop(_, inits, body) => {
            inits.iter_mut().try_for_each(|e| mark_tail(e, false, c))?;
            mark_tail(body, tail, c)
        }
        Expr::Tick(body) => mark_tail(body, tail, c),
        Expr::Inlined { .. } => Ok(()), // Its body is in another source function.
        _ => {
            let mut result = Ok(());
            children_mut(e, &mut |e| {
                if result.is_ok() {
                    result = mark_tail(e, false, c);
                }
            });
            result
        }
    })
}

struct Optimizer<'a, 'v> {
    c: &'a mut Compiler<'v>,
    flags: u16,
    owner: FnId,
    constants: FxHashMap<VarId, Atom>,
    functions: FxHashMap<VarId, FnId>,
    ints: FxHashSet<VarId>,
    active: Vec<FnId>,
    /// Bounds total copied nodes per top-level compilation, not per call site.
    copies: usize,
}

impl Compiler<'_> {
    pub(super) fn optimize(&mut self, root: FnId) -> R<()> {
        let flags = flags();
        // These experimental tree analyses revisit lexical bodies. Leave
        // large generated forms to the linear original compiler, including
        // its register-limit diagnostics, instead of doing quadratic work.
        if flags & 255 == 0 || self.vars.len() > 1024 || (self.module == ROOT_MODULE && flags & ROOT == 0) {
            return Ok(());
        }
        let mut o = Optimizer {
            c: self,
            flags,
            owner: root,
            constants: FxHashMap::default(),
            functions: FxHashMap::default(),
            ints: FxHashSet::default(),
            active: vec![],
            copies: 10_000,
        };
        for f in (0..o.c.funcs.len()).rev() {
            o.owner = f;
            let Some(mut body) = o.c.funcs[f].body.take() else { continue };
            if flags & (LOCAL | CONTIFY) != 0 {
                mark_tail(&mut body, true, o.c)?;
            }
            o.walk(&mut body)?;
            o.c.funcs[f].body = Some(body);
            o.constants.clear();
            o.functions.clear();
            o.ints.clear();
        }
        // Inlining/contification can remove captures or introduce references
        // from a new owner. Regenerate the live closure tree's environments.
        for v in &mut o.c.vars {
            v.captured = false;
            v.captured_from = usize::MAX;
        }
        for f in &mut o.c.funcs {
            f.free.clear();
        }
        o.c.capture_function(root);
        Ok(())
    }

    fn capture_function(&mut self, f: FnId) {
        self.fn_stack.push(f);
        let body = self.funcs[f].body.take().unwrap();
        self.capture_expr(&body);
        self.funcs[f].body = Some(body);
        self.fn_stack.pop();
    }
    fn capture_expr(&mut self, e: &Expr) {
        crate::nested(|| {
            match e {
                Expr::Local(v) | Expr::SetLocal(v, _) => self.use_var(*v),
                Expr::Lambda(f) => self.capture_function(*f),
                _ => {}
            }
            children(e, &mut |e| self.capture_expr(e));
        });
    }
}

impl Optimizer<'_, '_> {
    fn enabled(&self, f: u16) -> bool {
        self.flags & f != 0
    }
    fn var(&mut self) -> VarId {
        let v = self.c.vars.len();
        self.c.vars.push(VarInfo { owner: self.owner, assigned: false, captured: false, captured_from: usize::MAX });
        v
    }
    fn walk(&mut self, e: &mut Expr) -> R<()> {
        self.c.step()?;
        crate::nested(|| self.walk_inner(e))
    }
    fn walk_inner(&mut self, e: &mut Expr) -> R<()> {
        if self.enabled(CONTIFY) && self.contify(e)? {
            return self.walk(e);
        }
        match e {
            Expr::Local(v) if self.enabled(BINDINGS) => {
                if let Some(a) = self.constants.get(v) {
                    *e = a.expr();
                }
            }
            Expr::Let(bs, body) | Expr::Letrec(bs, body) => {
                // VarIds are unique; scope entries can stay until the end of
                // this function. They cannot occur outside their lexical scope.
                for (v, init) in bs.iter_mut() {
                    self.walk(init)?;
                    if !self.c.vars[*v].assigned {
                        if self.enabled(BINDINGS)
                            && let Some(a) = Atom::of(init)
                            && !matches!(a, Atom::Local(x) if self.c.vars[x].assigned)
                        {
                            self.constants.insert(*v, a);
                        }
                        if self.enabled(LOCAL)
                            && let Expr::Lambda(f) = init
                        {
                            self.functions.insert(*v, *f);
                        }
                        if integer(init, &self.ints) {
                            self.ints.insert(*v);
                        }
                    }
                }
                self.walk(body)?;
            }
            Expr::Loop(l, inits, body) => {
                for init in inits.iter_mut() {
                    self.walk(init)?;
                }
                let params = self.c.loops[*l].clone();
                // Inductive integer facts: begin with integer initializers,
                // then remove candidates whose back-edge arguments disagree.
                let mut candidates: FxHashSet<_> = params
                    .iter()
                    .zip(inits.iter())
                    .filter(|(v, e)| !self.c.vars[**v].assigned && integer(e, &self.ints))
                    .map(|(v, _)| *v)
                    .collect();
                loop {
                    let mut facts = self.ints.clone();
                    facts.extend(&candidates);
                    let len = candidates.len();
                    candidates.retain(|v| {
                        let index = params.iter().position(|p| p == v).unwrap();
                        !any(body, &|e| {
                            matches!(e, Expr::LoopCall(target, args)
                            if target == l && args.get(index).is_none_or(|arg| !integer(arg, &facts)))
                        })
                    });
                    if candidates.len() == len {
                        break;
                    }
                }
                self.ints.extend(candidates);
                self.walk(body)?;
            }
            Expr::Call(f, xs, _) => {
                self.walk(f)?;
                for x in xs {
                    self.walk(x)?;
                }
            }
            _ => {
                let mut result = Ok(());
                children_mut(e, &mut |e| {
                    if result.is_ok() {
                        result = self.walk(e);
                    }
                });
                result?;
            }
        }
        if self.enabled(LOCAL) && self.inline(e)? {
            return self.walk(e);
        }
        if self.enabled(SCALAR) && self.scalar(e) {
            return self.walk(e);
        }
        if self.enabled(CSE) {
            self.cse(e);
        }
        if self.enabled(LICM) && self.licm(e)? {
            return self.walk(e);
        }
        if self.enabled(HOIST) {
            self.hoist(e);
        }
        self.simplify(e);
        Ok(())
    }

    fn simplify(&mut self, e: &mut Expr) {
        if self.enabled(BINDINGS) {
            if matches!(e, Expr::Prim(..)) {
                *e = fold(std::mem::replace(e, Expr::Void));
            }
            if let Expr::If(c, t, f) = e
                && let Expr::Const(s) = &**c
                && let Some(truth) = const_truth(s)
            {
                *e = std::mem::replace(if truth { t } else { f }, Expr::Void);
            }
        }
        if self.enabled(DEAD) {
            if let Expr::Seq(xs) = e {
                let last = xs.len().saturating_sub(1);
                let mut i = 0;
                xs.retain(|x| {
                    let keep = i == last || !discardable(x, &self.ints);
                    i += 1;
                    keep
                });
                if xs.len() == 1 {
                    *e = xs.pop().unwrap();
                }
            }
            if let Expr::Let(bs, body) | Expr::Letrec(bs, body) = e {
                let used: Vec<_> = bs.iter().map(|(v, _)| uses(body, *v, self.c) || bs.iter().any(|(_, x)| uses(x, *v, self.c))).collect();
                let mut i = 0;
                bs.retain(|(_, x)| {
                    let keep = used[i] || !discardable(x, &self.ints);
                    i += 1;
                    keep
                });
                if bs.is_empty() {
                    *e = std::mem::replace(body, Expr::Void);
                }
            }
        }
    }

    /// Clone only small, first-order trees. Every bound variable is renamed;
    /// captured outer values retain their resolved identities. Nested lambdas,
    /// handlers and loops are left to their original function, not duplicated.
    fn copy(&mut self, e: &Expr, vars: &mut FxHashMap<VarId, VarId>) -> R<Expr> {
        self.c.step()?;
        self.copies = self.copies.saturating_sub(1);
        crate::nested(|| {
            Ok(match e {
                Expr::Const(c) => Expr::Const(c.clone()),
                Expr::Local(v) => Expr::Local(*vars.get(v).unwrap_or(v)),
                Expr::Global(g) => Expr::Global(*g),
                Expr::Object(v) => Expr::Object(*v),
                Expr::Void => Expr::Void,
                Expr::Prim(p, xs, pos) => Expr::Prim(*p, xs.iter().map(|x| self.copy(x, vars)).collect::<R<_>>()?, *pos),
                Expr::Call(f, xs, pos) => {
                    Expr::Call(Box::new(self.copy(f, vars)?), xs.iter().map(|x| self.copy(x, vars)).collect::<R<_>>()?, *pos)
                }
                Expr::If(c, t, f) => Expr::If(Box::new(self.copy(c, vars)?), Box::new(self.copy(t, vars)?), Box::new(self.copy(f, vars)?)),
                Expr::Seq(xs) => Expr::Seq(xs.iter().map(|x| self.copy(x, vars)).collect::<R<_>>()?),
                Expr::And(xs) => Expr::And(xs.iter().map(|x| self.copy(x, vars)).collect::<R<_>>()?),
                Expr::Or(xs) => Expr::Or(xs.iter().map(|x| self.copy(x, vars)).collect::<R<_>>()?),
                Expr::Tick(x) => Expr::Tick(Box::new(self.copy(x, vars)?)),
                Expr::Inlined { name, site, body } => {
                    Expr::Inlined { name: name.clone(), site: *site, body: Box::new(self.copy(body, vars)?) }
                }
                Expr::SetLocal(v, x) => Expr::SetLocal(*vars.get(v).unwrap_or(v), Box::new(self.copy(x, vars)?)),
                Expr::SetGlobal(g, x) => Expr::SetGlobal(*g, Box::new(self.copy(x, vars)?)),
                Expr::Let(bs, body) => {
                    let mut copied = Vec::new();
                    for (v, x) in bs {
                        let x = self.copy(x, vars)?;
                        let new = self.var();
                        self.c.vars[new].assigned = self.c.vars[*v].assigned;
                        vars.insert(*v, new);
                        copied.push((new, x));
                    }
                    Expr::Let(copied, Box::new(self.copy(body, vars)?))
                }
                _ => unreachable!("copyable tree checked before copying"),
            })
        })
    }
    fn size(e: &Expr) -> Option<usize> {
        crate::nested(|| {
            if matches!(
                e,
                Expr::Lambda(_)
                    | Expr::Loop(..)
                    | Expr::LoopCall(..)
                    | Expr::Letrec(..)
                    | Expr::Guard { .. }
                    | Expr::Escape(..)
                    | Expr::DefGlobal(..)
            ) {
                return None;
            }
            let mut n = Some(1);
            children(e, &mut |e| n = n.and_then(|n| Self::size(e).filter(|s| n + s <= 80).map(|s| n + s)));
            n
        })
    }
    fn inline(&mut self, e: &mut Expr) -> R<bool> {
        let Expr::Call(callee, args, site) = e else { return Ok(false) };
        let site = *site;
        let f = match &**callee {
            Expr::Local(v) => self.functions.get(v).copied(),
            Expr::Lambda(f) => Some(*f),
            _ => None,
        };
        let Some(f) = f else { return Ok(false) };
        if self.active.contains(&f) || self.active.len() >= 4 || self.c.funcs[f].file != self.c.funcs[self.owner].file {
            return Ok(false);
        }
        let info = &self.c.funcs[f];
        if args.len() > 16 || args.len() < info.params.len() || (info.rest.is_none() && args.len() != info.params.len()) {
            return Ok(false);
        }
        let Some(body) = &info.body else { return Ok(false) };
        let Some(size) = Self::size(body) else { return Ok(false) };
        if size > self.copies {
            return Ok(false);
        }
        let params = info.params.clone();
        let rest = info.rest;
        let name = info.name.clone();
        let mut renames = FxHashMap::default();
        let mut bindings = Vec::new();
        // Stage all arguments first, before constructing the rest list, just
        // as a call does. No argument is duplicated or evaluated lazily.
        let mut staged = Vec::new();
        for arg in args.iter_mut() {
            let v = self.var();
            bindings.push((v, std::mem::replace(arg, Expr::Void)));
            staged.push(v);
        }
        for (p, a) in params.iter().zip(&staged) {
            let v = self.var();
            self.c.vars[v].assigned = self.c.vars[*p].assigned;
            renames.insert(*p, v);
            bindings.push((v, Expr::Local(*a)));
        }
        if let Some(r) = rest {
            let v = self.var();
            self.c.vars[v].assigned = self.c.vars[r].assigned;
            renames.insert(r, v);
            let list = staged[params.len()..]
                .iter()
                .rev()
                .fold(Atom::Nil.expr(), |tail, a| Expr::Prim(Prim::Cons, vec![Expr::Local(*a), tail], NO_POS));
            bindings.push((v, list));
        }
        let body = self.c.funcs[f].body.take().unwrap();
        let copied = self.copy(&body, &mut renames);
        self.c.funcs[f].body = Some(body);
        let mut copied = copied?;
        self.active.push(f);
        self.walk(&mut copied)?;
        self.active.pop();
        *e = Expr::Let(bindings, Box::new(Expr::Inlined { name, site, body: Box::new(Expr::Tick(Box::new(copied))) }));
        Ok(true)
    }

    fn cse(&mut self, e: &mut Expr) {
        let Expr::Prim(_, xs, _) = e else { return };
        if xs.len() != 2 || !matches!(xs[0], Expr::Prim(..)) {
            return;
        }
        if let (Some(a), Some(b)) = (key(&xs[0], 8), key(&xs[1], 8))
            && a == b
        {
            let v = self.var();
            let value = std::mem::replace(&mut xs[0], Expr::Local(v));
            xs[1] = Expr::Local(v);
            let body = std::mem::replace(e, Expr::Void);
            *e = Expr::Let(vec![(v, value)], Box::new(body));
        }
    }

    fn scalar(&mut self, e: &mut Expr) -> bool {
        let Expr::Let(bs, body) = e else { return false };
        // One allocation at a time; preserve every initializer's order.
        let Some(index) = bs.iter().position(|(v, x)| !self.c.vars[*v].assigned && self.fields(x).is_some()) else { return false };
        let (v, init) = &bs[index];
        let v = *v;
        let pair = matches!(init, Expr::Prim(Prim::Cons, _, _));
        let len = self.fields(init).unwrap();
        if bs[index + 1..].iter().any(|(_, x)| uses(x, v, self.c)) || !self.field_uses(body, v, len, pair) {
            return false;
        }
        let mut values = match &mut bs[index].1 {
            Expr::Prim(_, xs, _) | Expr::Call(_, xs, _) => std::mem::take(xs),
            _ => unreachable!(),
        };
        let fields: Vec<_> = values.iter().map(|_| self.var()).collect();
        self.replace_fields(body, v, &fields, pair);
        let bindings = fields.into_iter().zip(values.drain(..)).collect::<Vec<_>>();
        bs.splice(index..=index, bindings);
        true
    }
    fn fields(&self, e: &Expr) -> Option<usize> {
        match e {
            Expr::Prim(Prim::Cons, _, _) => Some(2),
            Expr::Call(f, xs, _) if xs.len() <= 8 => match &**f {
                Expr::Global(g) if self.c.vm.inlinable(*g) && self.c.vm.global_name(*g).as_ref() == "vector" => Some(xs.len()),
                _ => None,
            },
            _ => None,
        }
    }
    fn field_index(p: Prim, xs: &[Expr], v: VarId, len: usize, pair: bool) -> Option<usize> {
        if !matches!(xs.first(), Some(Expr::Local(x)) if *x == v) {
            return None;
        }
        match (pair, p) {
            (true, Prim::Car) => Some(0),
            (true, Prim::Cdr) => Some(1),
            (false, Prim::VRef | Prim::VSet) => match xs.get(1) {
                Some(Expr::Const(Sexp::Int(n))) => usize::try_from(*n).ok().filter(|n| *n < len),
                _ => None,
            },
            _ => None,
        }
    }
    fn field_uses(&self, e: &Expr, v: VarId, len: usize, pair: bool) -> bool {
        crate::nested(|| {
            if let Expr::Prim(p, xs, _) = e
                && Self::field_index(*p, xs, v, len, pair).is_some()
            {
                return xs.iter().skip(1).all(|x| self.field_uses(x, v, len, pair));
            }
            if matches!(e, Expr::Local(x) | Expr::SetLocal(x, _) if *x == v) {
                return false;
            }
            if let Expr::Lambda(f) = e
                && self.c.funcs[*f].free.contains(&v)
            {
                return false;
            }
            let mut okay = true;
            children(e, &mut |e| okay &= self.field_uses(e, v, len, pair));
            okay
        })
    }
    fn replace_fields(&mut self, e: &mut Expr, v: VarId, fields: &[VarId], pair: bool) {
        crate::nested(|| {
            if let Expr::Prim(p, xs, _) = e
                && let Some(i) = Self::field_index(*p, xs, v, fields.len(), pair)
            {
                if *p == Prim::VSet {
                    let mut value = xs.pop().unwrap();
                    self.replace_fields(&mut value, v, fields, pair);
                    self.c.vars[fields[i]].assigned = true;
                    *e = Expr::SetLocal(fields[i], Box::new(value));
                } else {
                    *e = Expr::Local(fields[i]);
                }
                return;
            }
            children_mut(e, &mut |e| self.replace_fields(e, v, fields, pair));
        });
    }

    fn operator_only(&self, e: &Expr, vars: &[VarId]) -> bool {
        crate::nested(|| match e {
            Expr::Call(f, xs, _) if matches!(&**f, Expr::Local(v) if vars.contains(v)) => xs.iter().all(|e| self.operator_only(e, vars)),
            Expr::Local(v) | Expr::SetLocal(v, _) if vars.contains(v) => false,
            Expr::Lambda(f) if self.c.funcs[*f].free.iter().any(|v| vars.contains(v)) => false,
            _ => {
                let mut okay = true;
                children(e, &mut |e| okay &= self.operator_only(e, vars));
                okay
            }
        })
    }

    fn hoist(&mut self, e: &mut Expr) {
        let Expr::Loop(l, _, body) = e else { return };
        let params = &self.c.loops[*l];
        let mut hoisted = Vec::new();
        self.find_hoists(body, params.clone().as_slice(), &mut hoisted);
        if !hoisted.is_empty() {
            let loop_ = std::mem::replace(e, Expr::Void);
            *e = Expr::Let(hoisted, Box::new(loop_));
        }
    }
    fn find_hoists(&mut self, e: &mut Expr, params: &[VarId], out: &mut Vec<(VarId, Expr)>) {
        crate::nested(|| {
            // Do not cross a nested loop: its own parameters may be captures.
            if matches!(e, Expr::Loop(..)) {
                return;
            }
            if let Expr::Let(bs, body) = e {
                let mut i = 0;
                while i < bs.len() {
                    let (v, init) = &bs[i];
                    let yes = if let Expr::Lambda(f) = init {
                        !self.c.vars[*v].assigned
                            && self.c.funcs[*f].free.iter().all(|v| {
                                !params.contains(v)
                                    && !self.c.vars[*v].assigned
                                    && (self.c.vars[*v].owner != self.owner || self.c.funcs[self.owner].params.contains(v))
                            })
                            && self.operator_only(body, &[*v])
                            && !bs.iter().enumerate().any(|(j, (_, e))| j != i && uses(e, *v, self.c))
                    } else {
                        false
                    };
                    // Only outer-function captures and immutable parameters
                    // dominate the loop; body-local captures do not.
                    if yes {
                        out.push(bs.remove(i));
                    } else {
                        i += 1;
                    }
                }
            }
            children_mut(e, &mut |e| self.find_hoists(e, params, out));
        });
    }

    fn contify(&mut self, e: &mut Expr) -> R<bool> {
        let Expr::Letrec(bs, body) = e else { return Ok(false) };
        if bs.is_empty() || bs.len() > 8 {
            return Ok(false);
        }
        let mut fs = Vec::new();
        for (v, init) in bs.iter() {
            if let Expr::Lambda(f) = init {
                fs.push((*v, *f));
            } else {
                return Ok(false);
            }
        }
        let Expr::Call(entry, initial, site) = &mut **body else { return Ok(false) };
        let entry_site = *site;
        let Expr::Local(entry) = &**entry else { return Ok(false) };
        let Some(start) = fs.iter().position(|(v, _)| v == entry) else { return Ok(false) };
        let arity = initial.len();
        let ids: Vec<_> = fs.iter().map(|(v, _)| *v).collect();
        for (_, f) in &fs {
            let info = &self.c.funcs[*f];
            if info.rest.is_some()
                || info.params.len() != arity
                || info.file != self.c.funcs[self.owner].file
                || info.params.iter().any(|v| self.c.vars[*v].assigned) || !info.body.as_ref().is_some_and(|e| {
                Self::size(e).is_some_and(|n| n * fs.len() <= self.copies)
                    && self.tail_group(e, &ids, true)
                    && !any(
                        e,
                        &|e| matches!(e, Expr::Call(f, xs, _) if matches!(&**f, Expr::Local(v) if ids.contains(v)) && xs.len() != arity),
                    )
            }) {
                return Ok(false);
            }
        }
        if initial.iter().any(|e| !self.operator_only(e, &ids) || ids.iter().any(|v| uses(e, *v, self.c))) {
            return Ok(false);
        }
        let state = self.var();
        let params: Vec<_> = (0..arity).map(|_| self.var()).collect();
        let l = self.c.loops.len();
        let mut loop_params = vec![state];
        loop_params.extend(&params);
        self.c.loops.push(loop_params);
        let mut branches = Vec::new();
        for (_, f) in &fs {
            let mut map: FxHashMap<_, _> = self.c.funcs[*f].params.iter().copied().zip(params.iter().copied()).collect();
            let original = self.c.funcs[*f].body.take().unwrap();
            let copied = self.copy(&original, &mut map);
            self.c.funcs[*f].body = Some(original);
            let mut copied = copied?;
            Self::group_calls(&mut copied, &ids, l);
            branches.push(Expr::Inlined {
                name: self.c.funcs[*f].name.clone(),
                site: CallSite { tail: true, ..entry_site },
                body: Box::new(copied),
            });
        }
        let mut dispatch = branches.pop().unwrap();
        for (i, branch) in branches.into_iter().enumerate().rev() {
            dispatch = Expr::If(
                Box::new(Expr::Prim(Prim::NumEq, vec![Expr::Local(state), Atom::Int(i as i64).expr()], NO_POS)),
                Box::new(branch),
                Box::new(dispatch),
            );
        }
        let mut inits = vec![Atom::Int(start as i64).expr()];
        inits.extend(std::mem::take(initial));
        *e = Expr::Inlined {
            name: self.c.funcs[fs[start].1].name.clone(),
            site: entry_site,
            body: Box::new(Expr::Tick(Box::new(Expr::Loop(l, inits, Box::new(dispatch))))),
        };
        Ok(true)
    }
    fn tail_group(&self, e: &Expr, ids: &[VarId], tail: bool) -> bool {
        crate::nested(|| match e {
            Expr::Call(f, xs, _) if matches!(&**f, Expr::Local(v) if ids.contains(v)) => {
                tail && xs.iter().all(|e| !ids.iter().any(|v| uses(e, *v, self.c)))
            }
            Expr::If(c, t, f) => self.tail_group(c, ids, false) && self.tail_group(t, ids, tail) && self.tail_group(f, ids, tail),
            Expr::Let(bs, body) => bs.iter().all(|(_, e)| self.tail_group(e, ids, false)) && self.tail_group(body, ids, tail),
            Expr::Seq(xs) => xs.iter().enumerate().all(|(i, e)| self.tail_group(e, ids, tail && i + 1 == xs.len())),
            _ => !ids.iter().any(|v| uses(e, *v, self.c)),
        })
    }
    fn group_calls(e: &mut Expr, ids: &[VarId], l: LoopId) {
        crate::nested(|| {
            if let Expr::Call(f, xs, _) = e
                && let Expr::Local(v) = &**f
                && let Some(i) = ids.iter().position(|x| x == v)
            {
                let mut args = vec![Atom::Int(i as i64).expr()];
                args.extend(std::mem::take(xs));
                *e = Expr::LoopCall(l, args);
                return;
            }
            children_mut(e, &mut |e| Self::group_calls(e, ids, l));
        });
    }

    fn licm(&mut self, e: &mut Expr) -> R<bool> {
        let Expr::Loop(l, inits, body) = e else { return Ok(false) };
        let params = self.c.loops[*l].clone();
        // A deliberately small guarded LICM experiment: integer induction
        // variables, one exit test and one effect-free numeric recurrence.
        if !inits.iter().all(|e| matches!(e, Expr::Const(Sexp::Int(_)))) || params.iter().any(|v| self.c.vars[*v].assigned) {
            return Ok(false);
        }
        let Expr::If(test, exit, step) = &mut **body else { return Ok(false) };
        if !matches!(&**exit, Expr::Local(_) | Expr::Const(_))
            || key(test, 8).is_none()
            || any(test, &|e| matches!(e, Expr::Local(v) if self.c.vars[*v].assigned))
        {
            return Ok(false);
        }
        let Expr::LoopCall(target, args) = &mut **step else { return Ok(false) };
        if target != l {
            return Ok(false);
        }
        let mut candidate = None;
        for arg in args.iter() {
            self.invariant(arg, &params, &mut candidate);
        }
        let Some(candidate) = candidate else { return Ok(false) };
        let Some(wanted) = key(candidate, 8) else { return Ok(false) };
        // All other recurrence expressions must be exact integer arithmetic
        // on induction variables and constants; no competing error/effect.
        if !args.iter().all(|e| Self::recurrence(e, &wanted, &params)) {
            return Ok(false);
        }
        let mut renames = FxHashMap::default();
        let invariant = self.copy(candidate, &mut renames)?;
        // The dominating initial-value bindings are distinct from the loop
        // parameters, whose values change on every back edge.
        let outer: Vec<_> = params.iter().map(|_| self.var()).collect();
        renames.extend(params.iter().copied().zip(outer.iter().copied()));
        let test_copy = self.copy(test, &mut renames)?;
        let exit_copy = self.copy(exit, &mut renames)?;
        let temp = self.var();
        Self::replace_key(body, &wanted, temp);
        let initial = outer.iter().copied().zip(std::mem::take(inits)).collect();
        let loop_ = Expr::Loop(*l, outer.iter().map(|v| Expr::Local(*v)).collect(), std::mem::replace(body, Box::new(Expr::Void)));
        // The original test runs before the invariant: zero-trip loops and
        // errors in the test retain their behavior. The first loop test is
        // pure and can safely run again after the invariant succeeds.
        *e = Expr::Let(
            initial,
            Box::new(Expr::If(Box::new(test_copy), Box::new(exit_copy), Box::new(Expr::Let(vec![(temp, invariant)], Box::new(loop_))))),
        );
        Ok(true)
    }
    fn invariant<'a>(&self, e: &'a Expr, params: &[VarId], out: &mut Option<&'a Expr>) {
        if let Expr::Prim(Prim::Add | Prim::Sub | Prim::Mul | Prim::Rem | Prim::Quo | Prim::Mod, _, _) = e
            && key(e, 8).is_some()
            && !any(e, &|e| matches!(e, Expr::Local(v) if params.contains(v) || self.c.vars[*v].assigned))
        {
            if any(e, &|e| matches!(e, Expr::Local(_))) {
                *out = Some(e);
            }
            return;
        }
        children(e, &mut |e| self.invariant(e, params, out));
    }
    fn recurrence(e: &Expr, wanted: &Key, params: &[VarId]) -> bool {
        if key(e, 8).as_ref() == Some(wanted) {
            return true;
        }
        match e {
            Expr::Const(Sexp::Int(_)) => true,
            Expr::Local(v) => params.contains(v),
            Expr::Prim(Prim::Add | Prim::Sub | Prim::Mul, xs, _) => xs.iter().all(|e| Self::recurrence(e, wanted, params)),
            _ => false,
        }
    }
    fn replace_key(e: &mut Expr, wanted: &Key, v: VarId) {
        crate::nested(|| {
            if key(e, 8).as_ref() == Some(wanted) {
                *e = Expr::Local(v);
                return;
            }
            children_mut(e, &mut |e| Self::replace_key(e, wanted, v));
        });
    }
}
