//! S-expressions to register bytecode.
//!
//! Front end: expands macros, desugars derived forms, resolves lexical scope
//! (hygienically, see `expand`) and records which variables are captured by
//! inner closures and which are assigned. Variables that are both live in
//! boxes; everything else lives directly in registers. A named `let` whose name
//! is only used in tail calls compiles to a loop.
//!
//! Back end: one pass per function with stack-discipline register allocation.
//! Calls to never-redefined builtins with fixed arity become inline
//! instructions (`car`, `+`, `<`, ...), and comparisons in `if` tests fuse with
//! the branch. Every instruction records the source position of the enclosing
//! call for error messages.

use std::{borrow::Cow, collections::VecDeque, rc::Rc};

use rustc_hash::FxHashMap;

use crate::{
    code::{CapSrc, Code, Op, Reg},
    expand::Macro,
    heap::{Kind, field, is_kind},
    reader::{self, NO_POS, Pos, Sexp, intern, make_alias, strip, strip_sexp, symbol_name},
    value::Value,
    vm::{Error, GlobalBinding, ROOT_MODULE, Vm},
};

type VarId = usize;
type FnId = usize;
type LoopId = usize;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Prim {
    Add,
    Sub,
    Mul,
    Quo,
    Rem,
    Mod,
    Lt,
    Gt,
    Le,
    Ge,
    NumEq,
    Car,
    Cdr,
    Cons,
    NullP,
    PairP,
    Not,
    EqP,
    VRef,
    VSet,
}

fn prim(name: &str, nargs: usize) -> Option<Prim> {
    use Prim::*;
    Some(match (name, nargs) {
        ("+", 2) => Add,
        ("-", 2) => Sub,
        ("*", 2) => Mul,
        ("quotient", 2) => Quo,
        ("remainder", 2) => Rem,
        ("modulo", 2) => Mod,
        ("<", 2) => Lt,
        (">", 2) => Gt,
        ("<=", 2) => Le,
        (">=", 2) => Ge,
        ("=", 2) => NumEq,
        ("car", 1) => Car,
        ("cdr", 1) => Cdr,
        ("cons", 2) => Cons,
        ("null?", 1) => NullP,
        ("pair?", 1) => PairP,
        ("not", 1) => Not,
        ("eq?", 2) => EqP,
        ("vector-ref", 2) => VRef,
        ("vector-set!", 3) => VSet,
        _ => return None,
    })
}

pub const SPECIAL_FORMS: &[&str] = &[
    "quote",
    "quasiquote",
    "unquote",
    "unquote-splicing",
    "if",
    "define",
    "set!",
    "lambda",
    "begin",
    "let",
    "let*",
    "letrec",
    "letrec*",
    "cond",
    "case",
    "when",
    "unless",
    "and",
    "or",
    "define-syntax",
    "let-syntax",
    "letrec-syntax",
    "define-record-type",
    "guard",
    "require",
    "provide",
    "define-library",
    "import",
    "include",
    "include-ci",
    "cond-expand",
    "match",
    "%with-escape",
];

/// How each special form, and `syntax-rules`, is written, and its
/// documentation: what `help` and the editor's help show for it.
pub const SPECIAL_FORM_DOCS: &[(&str, &str, &str)] = &[
    ("quote", "(quote datum)", "Return DATUM itself, unevaluated; 'DATUM is short for it."),
    (
        "quasiquote",
        "(quasiquote template)",
        "Return TEMPLATE, unevaluated except where it says `unquote`.\nA backquote before TEMPLATE is short for it; ,X and ,@X in it are\n`unquote` and `unquote-splicing`.",
    ),
    ("unquote", "(unquote expression)", "Put the value of EXPRESSION in a `quasiquote` template.\n,EXPRESSION is short for it."),
    (
        "unquote-splicing",
        "(unquote-splicing expression)",
        "Splice the list EXPRESSION gives into a `quasiquote` template.\n,@EXPRESSION is short for it.",
    ),
    (
        "if",
        "(if test consequent [alternate])",
        "Evaluate CONSEQUENT if TEST gives a true value, else ALTERNATE.\nWithout ALTERNATE, the result is then unspecified.",
    ),
    (
        "define",
        "(define name expression) (define (name . params) [doc] body ...)",
        "Bind NAME to the value of EXPRESSION, or to a procedure.\nThe procedure form takes PARAMS as `lambda` does, and a docstring DOC\nbefore a non-empty BODY. At top level it defines a global of the\nmodule; at the start of a body, a local.",
    ),
    ("set!", "(set! name expression)", "Assign the value of EXPRESSION to the variable NAME."),
    (
        "lambda",
        "(lambda params [doc] body ...)",
        "Make a procedure of PARAMS that evaluates BODY.\nPARAMS lists required parameters, then optional ones [x default],\nthen keyword ones #:k x or #:k [x default], and may end in . REST.\nA string DOC before a non-empty BODY is the docstring.",
    ),
    (
        "begin",
        "(begin form ...)",
        "Evaluate the FORMs in order and return the value of the last.\nAt top level, definitions in it are top-level definitions.",
    ),
    (
        "let",
        "(let ((name init) ...) body ...) (let loop ((name init) ...) body ...)",
        "Bind each NAME to the value of its INIT, then evaluate BODY.\nWith a name LOOP, LOOP is a procedure of the NAMEs that runs BODY\nagain: a named let.",
    ),
    ("let*", "(let* ((name init) ...) body ...)", "Bind each NAME in turn, each INIT seeing the ones before it."),
    ("letrec", "(letrec ((name init) ...) body ...)", "Bind the NAMEs, each INIT seeing all of them, for recursion."),
    ("letrec*", "(letrec* ((name init) ...) body ...)", "Bind the NAMEs in order, each INIT seeing all of them."),
    (
        "cond",
        "(cond (test expression ...) ... [(else expression ...)])",
        "Evaluate the EXPRESSIONs of the first clause whose TEST holds.\nA clause (TEST => RECEIVER) calls RECEIVER with the test's value.",
    ),
    (
        "case",
        "(case key ((datum ...) expression ...) ... [(else expression ...)])",
        "Evaluate the clause whose DATUMs include the value of KEY (by `eqv?`).",
    ),
    ("when", "(when test body ...)", "Evaluate BODY if TEST gives a true value."),
    ("unless", "(unless test body ...)", "Evaluate BODY if TEST gives #f."),
    ("and", "(and test ...)", "Return the first false value of the TESTs, else the last value.\nWith no TEST, return #t."),
    ("or", "(or test ...)", "Return the first true value of the TESTs, else #f."),
    (
        "define-syntax",
        "(define-syntax name (syntax-rules (literal ...) [doc] (pattern template) ...))",
        "Define NAME as a hygienic macro.\nA use is rewritten by the first PATTERN it matches into its TEMPLATE.\nA string DOC after the LITERALs is the docstring, as in Guile.",
    ),
    ("let-syntax", "(let-syntax ((name transformer) ...) body ...)", "Evaluate BODY with the macros NAME bound locally."),
    (
        "letrec-syntax",
        "(letrec-syntax ((name transformer) ...) body ...)",
        "Evaluate BODY with the macros NAME bound locally, seeing each other.",
    ),
    (
        "define-record-type",
        "(define-record-type name (constructor field ...) predicate (field accessor [modifier]) ...)",
        "Define the record type NAME and its procedures.\nCONSTRUCTOR makes a record of the FIELDs it lists, PREDICATE tests\nfor one, each ACCESSOR reads a field and each MODIFIER sets it. Each\nprocedure is documented as what it does.",
    ),
    (
        "guard",
        "(guard (var clause ...) body ...)",
        "Evaluate BODY, handling what it raises with the CLAUSEs.\nThe condition raised is bound to VAR and the first CLAUSE whose test\nholds is evaluated, as in `cond`; with none, it is raised again.",
    ),
    ("require", "(require \"file\" ...)", "Load each FILE as a module, once, and import what it provides."),
    ("provide", "(provide name ...)", "Make the NAMEs of this file's module visible to modules requiring it."),
    ("define-library", "(define-library (name ...) declaration ...)", "Define an R7RS library: its exports, imports and body."),
    ("import", "(import import-set ...)", "Import the bindings of R7RS libraries, as each IMPORT-SET selects."),
    ("include", "(include \"file\" ...)", "Read the forms of each FILE and evaluate them here."),
    ("include-ci", "(include-ci \"file\" ...)", "Read the forms of each FILE, case-folded, and evaluate them here."),
    ("cond-expand", "(cond-expand (requirement form ...) ...)", "Use the FORMs of the first clause whose REQUIREMENT the system meets."),
    (
        "match",
        "(match expression (pattern [#:when guard] body ...) ...)",
        "Evaluate the BODY of the first clause whose PATTERN matches.\nPatterns are literals, (quote datum), _, variables, lists (with a\ntrailing ...), dotted lists, vectors, (? pred pattern ...), (and\npattern ...), (or pattern ...), (not pattern), (cons p q), (list\npattern ...), (vector pattern ...) and records by type name, (point\nx y). A GUARD must also hold. It is an error if no clause matches.",
    ),
    ("%with-escape", "(%with-escape f)", "Call F with an escape-only continuation."),
    // Syntax only within other forms.
    (
        "syntax-rules",
        "(syntax-rules [ellipsis] (literal ...) [doc] (pattern template) ...)",
        "Make the transformer of a macro, for `define-syntax` and `let-syntax`.\nA use is rewritten by the first PATTERN it matches into its TEMPLATE;\nLITERALs match only themselves, ELLIPSIS (... by default) repeats\nwhat comes before it, and identifiers the TEMPLATE introduces cannot\ncapture the use's. A string DOC is the macro's docstring.",
    ),
];

/// How the special form `name` is written and its documentation.
pub fn special_form_doc(name: &str) -> Option<(&'static str, &'static str)> {
    SPECIAL_FORM_DOCS.iter().find(|(n, ..)| *n == name).map(|(_, syntax, doc)| (*syntax, *doc))
}

pub fn is_special_form(name: &str) -> bool {
    SPECIAL_FORMS.contains(&name)
}

#[derive(Debug)]
enum Expr {
    Const(Sexp),
    Local(VarId),
    Global(u32),
    SetLocal(VarId, Box<Expr>),
    SetGlobal(u32, Box<Expr>),
    DefGlobal(u32, Box<Expr>),
    If(Box<Expr>, Box<Expr>, Box<Expr>),
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Seq(Vec<Expr>),
    Lambda(FnId),
    /// A heap object known when compiling (an inlined procedure's closure).
    Object(Value),
    Call(Box<Expr>, Vec<Expr>, Pos),
    Prim(Prim, Vec<Expr>, Pos),
    Let(Vec<(VarId, Expr)>, Box<Expr>),
    Letrec(Vec<(VarId, Expr)>, Box<Expr>),
    Loop(LoopId, Vec<Expr>, Box<Expr>),
    LoopCall(LoopId, Vec<Expr>),
    /// `(%with-escape f)`: call `f` with an escape-only continuation.
    Escape(Box<Expr>, Pos),
    /// `guard`: run `body`; on a raise bind the condition to `var` and run `handler`.
    Guard {
        var: VarId,
        body: Box<Expr>,
        handler: Box<Expr>,
    },
    Void,
}

struct VarInfo {
    owner: FnId,
    assigned: bool,
    captured: bool,
}

struct FuncInfo {
    name: Rc<str>,
    pos: Pos,
    param_names: Vec<Rc<str>>,
    doc: Option<Rc<str>>,
    params: Vec<VarId>,
    rest: Option<VarId>,
    body: Option<Expr>,
    free: Vec<VarId>,
    parent: Option<FnId>,
    /// The source file its positions are offsets in.
    file: u32,
    /// For a root-module top-level `define`: the form's position (see
    /// `Code::definition`).
    definition: Option<Pos>,
}

#[derive(Clone)]
enum Binding {
    Var(VarId),
    Loop(LoopId),
    Macro(Rc<Macro>),
    /// An inlined procedure's parameter, standing for the argument written
    /// at the call (see `Compiler::inline_call`).
    Subst(Rc<Subst>),
}

/// What an identifier denotes.
enum Resolved {
    Local(Binding),
    /// A top-level name: `sym` (alias-free) in `module`.
    Global {
        module: u32,
        sym: u32,
    },
}

/// What the head of a compound form is.
enum Head {
    Special(Rc<str>),
    Macro(Rc<Macro>),
    Loop(LoopId),
    Global(u32),
    Subst(Rc<Subst>),
    Other,
}

pub struct Compiler<'v> {
    vm: &'v mut Vm,
    module: u32,
    file: u32,
    vars: Vec<VarInfo>,
    funcs: Vec<FuncInfo>,
    loops: Vec<Vec<VarId>>,
    scopes: Vec<Vec<(u32, Binding)>>,
    fn_stack: Vec<FnId>,
    /// Calls being inlined around the current form.
    inlining: usize,
    /// The symbol `lambda`, which arguments are checked for (`procedure_arg`).
    lambda_sym: u32,
}

type R<T> = Result<T, Error>;

fn err<T>(msg: impl Into<String>) -> R<T> {
    Err(Error::new(msg))
}

fn display_name(sym: u32) -> Rc<str> {
    symbol_name(strip(sym))
}

/// Reference to a root-module binding that user code cannot shadow.
fn core(name: &str) -> Sexp {
    thread_local! {
        static CORE: std::cell::RefCell<FxHashMap<u32, u32>> = Default::default();
    }
    let sym = intern(name);
    Sexp::Sym(CORE.with(|c| *c.borrow_mut().entry(sym).or_insert_with(|| make_alias(sym, 0, ROOT_MODULE))))
}

fn list(items: Vec<Sexp>) -> Sexp {
    Sexp::list_of(items)
}

impl<'v> Compiler<'v> {
    pub fn new(vm: &'v mut Vm, module: u32, file: u32) -> Self {
        Compiler {
            vm,
            module,
            file,
            vars: Vec::new(),
            funcs: Vec::new(),
            loops: Vec::new(),
            scopes: Vec::new(),
            fn_stack: Vec::new(),
            inlining: 0,
            lambda_sym: intern("lambda"),
        }
    }

    /// Compile one top-level form into a zero-argument code object.
    pub fn compile_toplevel(mut self, form: &Sexp) -> R<u32> {
        let f = self.new_fn("toplevel".into(), None);
        self.fn_stack.push(f);
        let body = self.toplevel(form)?;
        self.fn_stack.pop();
        self.funcs[f].body = Some(body);
        self.generate(f)
    }

    fn new_fn(&mut self, name: Rc<str>, parent: Option<FnId>) -> FnId {
        self.funcs.push(FuncInfo {
            name,
            pos: NO_POS,
            param_names: vec![],
            doc: None,
            params: vec![],
            rest: None,
            body: None,
            free: vec![],
            parent,
            file: self.file,
            definition: None,
        });
        self.funcs.len() - 1
    }

    fn new_var(&mut self) -> VarId {
        let owner = *self.fn_stack.last().unwrap();
        self.vars.push(VarInfo { owner, assigned: false, captured: false });
        self.vars.len() - 1
    }

    // ----- name resolution -----

    fn resolve(&self, sym: u32) -> Resolved {
        self.resolve_in(sym, self.scopes.len(), self.module)
    }

    fn resolve_in(&self, sym: u32, depth: usize, module: u32) -> Resolved {
        if let Some(b) = self.scopes[..depth].iter().rev().find_map(|s| s.iter().rev().find(|(n, _)| *n == sym).map(|(_, b)| b.clone())) {
            return Resolved::Local(b);
        }
        match reader::alias(sym) {
            Some(a) => self.resolve_in(a.orig, a.env_depth.min(depth), a.module),
            None => Resolved::Global { module, sym },
        }
    }

    fn head(&mut self, sym: u32) -> Head {
        match self.resolve(sym) {
            Resolved::Local(Binding::Macro(m)) => Head::Macro(m),
            Resolved::Local(Binding::Loop(l)) => Head::Loop(l),
            Resolved::Local(Binding::Var(_)) => Head::Other,
            Resolved::Local(Binding::Subst(s)) => Head::Subst(s),
            Resolved::Global { module, sym } => match self.vm.lookup_global(module, sym) {
                Some(GlobalBinding::Macro(m)) => Head::Macro(m),
                Some(GlobalBinding::Var(g)) => Head::Global(g),
                None => {
                    let name = symbol_name(sym);
                    if SPECIAL_FORMS.contains(&&*name) { Head::Special(name) } else { Head::Global(self.vm.global_var(module, sym)) }
                }
            },
        }
    }

    /// free-identifier=? for macro literals: same binding (or both unbound
    /// with the same name).
    fn same_binding(&self, input: u32, literal: u32, m: &Macro) -> bool {
        let a = self.resolve(input);
        let b = self.resolve_in(literal, m.env_depth.min(self.scopes.len()), m.module);
        match (a, b) {
            (Resolved::Global { module: ma, sym: sa }, Resolved::Global { module: mb, sym: sb }) => {
                sa == sb && (ma == mb || self.vm.lookup_global(ma, sa).is_none())
            }
            (Resolved::Local(Binding::Var(x)), Resolved::Local(Binding::Var(y))) => x == y,
            _ => false,
        }
    }

    fn expand_macro(&self, m: &Macro, form: &Sexp) -> R<Sexp> {
        m.expand(form, &|input, lit| self.same_binding(input, lit, m)).map_err(Error::new)
    }

    /// Expand macro uses at the head of `form` until it is not a macro use.
    fn expand_head<'f>(&mut self, form: &'f Sexp) -> R<Cow<'f, Sexp>> {
        let mut form = Cow::Borrowed(form);
        for _ in 0..10_000 {
            let Sexp::List(items, _, _) = &*form else { return Ok(form) };
            let Some(sym) = items.first().and_then(Sexp::sym) else { return Ok(form) };
            match self.head(sym) {
                Head::Macro(m) => form = Cow::Owned(self.expand_macro(&m, &form)?),
                _ => return Ok(form),
            }
        }
        err("macro expansion does not terminate")
    }

    /// Head special-form name of a (head-expanded) form, if any.
    fn special_of(&mut self, form: &Sexp) -> Option<Rc<str>> {
        let sym = form.list()?.first()?.sym()?;
        match self.head(sym) {
            Head::Special(name) => Some(name),
            _ => None,
        }
    }

    /// Reference a local from the current function, recording captures.
    fn use_var(&mut self, v: VarId) {
        let owner = self.vars[v].owner;
        let mut f = *self.fn_stack.last().unwrap();
        while f != owner {
            self.vars[v].captured = true;
            if !self.funcs[f].free.contains(&v) {
                self.funcs[f].free.push(v);
            }
            f = self.funcs[f].parent.expect("variable owner is an ancestor");
        }
    }

    // ----- top level and bodies -----

    fn toplevel(&mut self, form: &Sexp) -> R<Expr> {
        crate::nested(|| self.toplevel_step(form))
    }

    fn toplevel_step(&mut self, form: &Sexp) -> R<Expr> {
        let form = self.expand_head(form)?;
        let Some(special) = self.special_of(&form) else { return self.expr(&form) };
        let items = form.list().unwrap();
        match &*special {
            "begin" => Ok(Expr::Seq(items[1..].iter().map(|f| self.toplevel(f)).collect::<R<Vec<_>>>()?).or_void()),
            "define" => {
                let (name, value) = define_parts(items)?;
                let g = self.vm.define_var(self.module, strip(name));
                // A variable's docstring follows its value, as in Emacs's
                // `defvar`: `(define name value "doc")`.
                // Defining it again takes the old docstring away.
                match items {
                    [_, Sexp::Sym(_), _, Sexp::Str(doc)] => self.vm.variable_docs.insert(g, doc.clone()),
                    _ => self.vm.variable_docs.remove(&g),
                };
                let procedure = matches!(value, Definiens::Procedure(..));
                let value = self.definiens(name, value)?;
                if let Expr::Lambda(f) = &value
                    && procedure
                    && self.module == ROOT_MODULE
                    && form.pos() != NO_POS
                {
                    self.funcs[*f].definition = Some(form.pos());
                }
                Ok(Expr::DefGlobal(g, Box::new(value)))
            }
            "define-syntax" => {
                let (name, m) = self.parse_macro(items, 0)?;
                self.vm.define_macro(self.module, strip(name), Rc::new(m));
                Ok(Expr::Void)
            }
            "define-record-type" => {
                let expanded = define_record_type(items)?;
                let typed = match (items.get(1), items.len().saturating_sub(4)) {
                    (Some(Sexp::Sym(t)), n) => {
                        let g = self.vm.define_var(self.module, strip(*t));
                        self.vm.record_types.insert(g, n);
                        Some((*t, g))
                    }
                    _ => None,
                };
                // The first definition creates a private descriptor. Capture
                // it lexically, rather than reading the public type name each
                // time a procedure runs: that name may also name a constructor.
                let forms = expanded.list().expect("record expansion");
                let (descriptor, initial) = define_parts(forms[1].list().expect("descriptor definition"))?;
                let initial = self.definiens(descriptor, initial)?;
                let captured = self.new_var();
                self.scopes.push(vec![(descriptor, Binding::Var(captured))]);
                let procedures = self.toplevel(&list([vec![core("begin")], forms[2..].to_vec()].concat()));
                self.scopes.pop();
                let compiled = Expr::Let(vec![(captured, initial)], Box::new(procedures?));
                // The type is documented unless one of its procedures has
                // its name (`(define-record-type point (point x y) ...)`).
                if let Some((t, g)) = typed {
                    let names = items[2..].iter().flat_map(|i| i.list().map_or_else(|| vec![i.clone()], |l| l.to_vec()));
                    if !names.into_iter().any(|n| n.sym().map(strip) == Some(strip(t))) {
                        let shown = symbol_name(strip(t));
                        let shown = shown.trim_start_matches('<').trim_end_matches('>');
                        self.vm.variable_docs.insert(g, format!("The type of `{shown}` records.").into());
                    }
                }
                Ok(compiled)
            }
            "require" => {
                for spec in &items[1..] {
                    match spec {
                        Sexp::Str(path) => self.vm.require(self.module, path)?,
                        other => return err(format!("require: expected a file path string, got {}", reader::display_sexp(other))),
                    }
                }
                Ok(Expr::Void)
            }
            "define-library" => {
                let dir = self.source_dir();
                self.vm.define_library(items, self.file, &dir)?;
                Ok(Expr::Void)
            }
            "import" => {
                let dir = self.source_dir();
                self.vm.import(self.module, &items[1..], &dir)?;
                Ok(Expr::Void)
            }
            "include" | "include-ci" | "cond-expand" => {
                let forms = self.spliced(&special, items)?;
                Ok(Expr::Seq(forms.iter().map(|(f, file)| self.in_file(*file, |c| c.toplevel(f))).collect::<R<Vec<_>>>()?).or_void())
            }
            "provide" => {
                let syms = items[1..]
                    .iter()
                    .map(|s| s.sym().map(strip).ok_or_else(|| Error::new("provide: expected identifiers")))
                    .collect::<R<Vec<_>>>()?;
                self.vm.provide(self.module, syms);
                Ok(Expr::Void)
            }
            _ => self.expr(&form),
        }
    }

    /// The directory of the file being compiled, for `include` and libraries.
    fn source_dir(&self) -> std::path::PathBuf {
        let name = &self.vm.files[self.file as usize].name;
        std::path::Path::new(&**name).parent().filter(|p| p.is_dir()).map_or_else(|| ".".into(), |p| p.to_path_buf())
    }

    /// The forms `include`, `include-ci` or `cond-expand` stand for, each
    /// with the source file it is from.
    fn spliced(&mut self, special: &str, items: &[Sexp]) -> R<Vec<(Sexp, u32)>> {
        let dir = self.source_dir();
        match special {
            "cond-expand" => Ok(self.vm.cond_expand(&items[1..], &dir)?.into_iter().map(|f| (f, self.file)).collect()),
            _ => Ok(self
                .vm
                .include(&items[1..], &dir, special == "include-ci")?
                .into_iter()
                .flat_map(|(forms, file)| forms.into_iter().map(move |f| (f, file)))
                .collect()),
        }
    }

    /// Compile forms read from `file`. Functions they create belong to it;
    /// the enclosing function, from another file, gets no positions for them
    /// (`at`).
    fn in_file<T>(&mut self, file: u32, compile: impl FnOnce(&mut Self) -> R<T>) -> R<T> {
        let saved = std::mem::replace(&mut self.file, file);
        let result = compile(self);
        self.file = saved;
        result
    }

    /// `pos` in the file being compiled, if the current function is from it.
    fn at(&self, pos: Pos) -> Pos {
        let same = self.fn_stack.last().is_none_or(|f| self.funcs[*f].file == self.file);
        if same { pos } else { NO_POS }
    }

    fn parse_macro(&self, items: &[Sexp], env_depth: usize) -> R<(u32, Macro)> {
        match items {
            [_, Sexp::Sym(name), spec] => {
                let m = Macro::parse(strip(*name), spec, env_depth, self.module, self.file).map_err(Error::new)?;
                Ok((*name, m))
            }
            _ => err("define-syntax: expected (define-syntax name (syntax-rules ...))"),
        }
    }

    /// A body: definitions (also from macro expansions) and expressions.
    /// Compiles as `letrec*`; expressions before the last definition become
    /// dummy bindings so mixed definitions and expressions keep their order.
    fn body(&mut self, forms: &[Sexp]) -> R<Expr> {
        self.scopes.push(Vec::new());
        let result = self.body_in_scope(forms);
        self.scopes.pop();
        result
    }

    fn body_in_scope(&mut self, forms: &[Sexp]) -> R<Expr> {
        enum Item<'a> {
            Def(u32, Definiens<'a>),
            Expr(Cow<'a, Sexp>),
        }
        // Forms are borrowed from the source; only macro output is owned.
        let mut queue: VecDeque<Cow<Sexp>> = forms.iter().map(Cow::Borrowed).collect();
        let mut items = Vec::new();
        while let Some(form) = queue.pop_front() {
            let form = match form {
                Cow::Borrowed(f) => self.expand_head(f)?,
                Cow::Owned(f) => Cow::Owned(match self.expand_head(&f)? {
                    Cow::Owned(e) => e,
                    Cow::Borrowed(_) => f,
                }),
            };
            match (self.special_of(&form).as_deref(), form) {
                (Some("begin"), Cow::Borrowed(f)) => {
                    f.list().unwrap_or(&[])[1..].iter().rev().for_each(|x| queue.push_front(Cow::Borrowed(x)));
                }
                (Some("begin"), Cow::Owned(f)) => {
                    f.list().unwrap_or(&[])[1..].iter().rev().for_each(|x| queue.push_front(Cow::Owned(x.clone())));
                }
                (Some("define"), Cow::Borrowed(f)) => {
                    let (name, value) = define_parts(f.list().unwrap_or(&[]))?;
                    items.push(Item::Def(name, value));
                }
                (Some("define"), Cow::Owned(f)) => {
                    let (name, value) = define_parts(f.list().unwrap_or(&[]))?;
                    items.push(Item::Def(name, value.into_owned()));
                }
                (Some("define-syntax"), form) => {
                    let depth = self.scopes.len();
                    let (name, m) = self.parse_macro(form.list().unwrap_or(&[]), depth)?;
                    self.scopes.last_mut().unwrap().push((name, Binding::Macro(Rc::new(m))));
                }
                (Some("define-record-type"), form) => queue.push_front(Cow::Owned(define_record_type(form.list().unwrap_or(&[]))?)),
                (_, form) => items.push(Item::Expr(form)),
            }
        }
        let last_def = items.iter().rposition(|i| matches!(i, Item::Def(..)));
        let Some(last_def) = last_def else {
            let exprs: Vec<Cow<Sexp>> = items.into_iter().map(|i| if let Item::Expr(e) = i { e } else { unreachable!() }).collect();
            return self.seq_of(&exprs);
        };
        // Bind every defined name first (letrec* scope).
        let mut vars = Vec::new();
        for item in &items[..=last_def] {
            let v = self.new_var();
            self.vars[v].assigned = true;
            if let Item::Def(name, _) = item {
                self.scopes.last_mut().unwrap().push((*name, Binding::Var(v)));
            }
            vars.push(v);
        }
        let mut bindings = Vec::new();
        let mut rest = Vec::new();
        for (i, item) in items.into_iter().enumerate() {
            match (i <= last_def, item) {
                (true, Item::Def(name, value)) => bindings.push((vars[i], self.definiens(name, value)?)),
                (true, Item::Expr(e)) => bindings.push((vars[i], self.expr(&e)?)),
                (false, Item::Expr(e)) => rest.push(e),
                (false, Item::Def(..)) => unreachable!(),
            }
        }
        let body = self.seq_of(&rest)?;
        Ok(Expr::Letrec(bindings, Box::new(body)))
    }

    fn seq_of(&mut self, forms: &[Cow<Sexp>]) -> R<Expr> {
        Ok(match forms {
            [] => Expr::Void,
            [one] => self.expr(one)?,
            many => Expr::Seq(many.iter().map(|e| self.expr(e)).collect::<R<_>>()?),
        })
    }

    /// What a definition of `name` binds.
    fn definiens(&mut self, name: u32, value: Definiens) -> R<Expr> {
        match value {
            Definiens::Expr(e) => self.named_expr(name, &e),
            Definiens::Procedure(params, body) => self.lambda(display_name(name), &params, body),
        }
    }

    fn seq(&mut self, forms: &[Sexp]) -> R<Expr> {
        Ok(match forms {
            [] => Expr::Void,
            [one] => self.expr(one)?,
            many => Expr::Seq(many.iter().map(|e| self.expr(e)).collect::<R<_>>()?),
        })
    }

    // ----- expressions -----

    fn expr(&mut self, s: &Sexp) -> R<Expr> {
        match s {
            Sexp::List(..) => crate::nested(|| self.expr_of(s)),
            _ => self.expr_of(s),
        }
    }

    fn expr_of(&mut self, s: &Sexp) -> R<Expr> {
        match s {
            Sexp::Sym(sym) => self.variable(*sym),
            Sexp::List(items, None, pos) if !items.is_empty() => self.compound(s, items, self.at(*pos)),
            // A macro use can be dotted.
            Sexp::List(items, Some(_), _) if items.first().and_then(Sexp::sym).is_some_and(|h| matches!(self.head(h), Head::Macro(_))) => {
                let expanded = self.expand_head(s)?;
                self.expr(&expanded)
            }
            Sexp::List(..) => err(format!("cannot evaluate {}", reader::display_sexp(s))),
            Sexp::Vector(_) => literal(s),
            Sexp::Labeled(_, datum) => {
                let mut datum = &**datum;
                while let Sexp::Labeled(_, inner) = datum {
                    datum = inner;
                }
                if matches!(datum, Sexp::Sym(_) | Sexp::List(..) | Sexp::LabelRef(_)) {
                    err(format!("datum labels belong in literals: {}", reader::display_sexp(s)))
                } else {
                    literal(s)
                }
            }
            Sexp::LabelRef(_) => err(format!("datum labels belong in literals: {}", reader::display_sexp(s))),
            _ => Ok(Expr::Const(s.clone())),
        }
    }

    fn variable(&mut self, sym: u32) -> R<Expr> {
        match self.resolve(sym) {
            Resolved::Local(Binding::Var(v)) => {
                self.use_var(v);
                Ok(Expr::Local(v))
            }
            Resolved::Local(Binding::Subst(s)) => self.subst_value(&s),
            Resolved::Local(Binding::Loop(_)) => err(format!("loop name {} used as a value", display_name(sym))),
            Resolved::Local(Binding::Macro(_)) => err(format!("syntax {} used as a value", display_name(sym))),
            Resolved::Global { module, sym } => match self.vm.lookup_global(module, sym) {
                Some(GlobalBinding::Macro(_)) => err(format!("syntax {} used as a value", display_name(sym))),
                Some(GlobalBinding::Var(g)) => Ok(Expr::Global(g)),
                None if SPECIAL_FORMS.contains(&&*symbol_name(sym)) => err(format!("syntax {} used as a value", display_name(sym))),
                None => Ok(Expr::Global(self.vm.global_var(module, sym))),
            },
        }
    }

    fn compound(&mut self, form: &Sexp, items: &[Sexp], pos: Pos) -> R<Expr> {
        let head = match items[0].sym() {
            Some(sym) => self.head(sym),
            None => Head::Other,
        };
        match head {
            Head::Macro(m) => {
                let expanded = self.expand_macro(&m, form)?;
                self.expr(&expanded)
            }
            Head::Special(name) => self.special(&name, form, items, pos),
            Head::Loop(l) => {
                let args = items[1..].iter().map(|a| self.expr(a)).collect::<R<Vec<_>>>()?;
                Ok(Expr::LoopCall(l, args))
            }
            Head::Global(g) => {
                if let Some(inlined) = self.inline_call(g, &items[1..], pos)? {
                    return Ok(inlined);
                }
                let args = items[1..].iter().map(|a| self.expr(a)).collect::<R<Vec<_>>>()?;
                if self.vm.inlinable(g)
                    && let Some(p) = prim_form(&self.vm.global_name(g), args.len())
                {
                    return Ok(fold(build_prim(p, args, pos)));
                }
                Ok(Expr::Call(Box::new(Expr::Global(g)), args, pos))
            }
            Head::Subst(s) => self.apply(&s.form, &items[1..], pos, Some(&s)),
            Head::Other => self.apply(&items[0], &items[1..], pos, None),
        }
    }

    fn special(&mut self, name: &str, form: &Sexp, items: &[Sexp], pos: Pos) -> R<Expr> {
        let arg = |i: usize| items.get(i).ok_or_else(|| Error::new(format!("{name}: malformed {}", reader::display_sexp(form))));
        match name {
            "quote" => literal(arg(1)?),
            "quasiquote" => {
                let expanded = quasi(arg(1)?, 1)?;
                self.expr(&expanded)
            }
            "unquote" | "unquote-splicing" => err(format!("{name} outside quasiquote")),
            "if" => {
                let c = self.expr(arg(1)?)?;
                let else_branch = |c: &mut Self| match items.get(3) {
                    Some(e) => c.expr(e),
                    None => Ok(Expr::Void),
                };
                // In an inlined definition, a test known from its arguments
                // picks a branch; the other is not compiled (it may use them
                // in ways the chosen one avoids).
                match c {
                    Expr::Const(Sexp::Bool(false)) if self.inlining > 0 => else_branch(self),
                    Expr::Const(_) if self.inlining > 0 => self.expr(arg(2)?),
                    c => {
                        let t = self.expr(arg(2)?)?;
                        let f = else_branch(self)?;
                        Ok(Expr::If(Box::new(c), Box::new(t), Box::new(f)))
                    }
                }
            }
            "set!" => {
                let target = arg(1)?.sym().ok_or(Error::new("set!: target must be an identifier"))?;
                let value = self.expr(arg(2)?)?;
                match self.resolve(target) {
                    Resolved::Local(Binding::Var(v)) => {
                        self.use_var(v);
                        self.vars[v].assigned = true;
                        Ok(Expr::SetLocal(v, Box::new(value)))
                    }
                    Resolved::Local(_) => err(format!("set!: {} is not a variable", display_name(target))),
                    Resolved::Global { module, sym } => {
                        let g = self.vm.global_var(module, sym);
                        // Primitives are sealed: code elsewhere may have them
                        // inlined. A module shadows one by defining it.
                        if self.module != ROOT_MODULE && self.vm.inlinable(g) {
                            return err(format!(
                                "set!: {} is a primitive and cannot be changed; define it in this module to shadow it",
                                display_name(target)
                            ));
                        }
                        Ok(Expr::SetGlobal(g, Box::new(value)))
                    }
                }
            }
            "lambda" => self.lambda("lambda".into(), arg(1)?, &items[2..]),
            "begin" => self.seq(&items[1..]),
            "let" => match arg(1)? {
                Sexp::Sym(loop_name) => self.named_let(*loop_name, arg(2)?, &items[3..], pos),
                bindings => self.let_form(bindings, &items[2..]),
            },
            "let*" => self.let_star(arg(1)?, &items[2..]),
            "letrec" | "letrec*" => self.letrec(arg(1)?, &items[2..]),
            "cond" => self.cond(&items[1..]),
            "case" => self.case(items),
            "when" | "unless" => {
                let c = self.expr(arg(1)?)?;
                let body = self.seq(&items[2..])?;
                let (t, f) = if name == "when" { (body, Expr::Void) } else { (Expr::Void, body) };
                Ok(Expr::If(Box::new(c), Box::new(t), Box::new(f)))
            }
            "and" => self.and(&items[1..]),
            "or" => self.or(&items[1..]),
            "let-syntax" | "letrec-syntax" => {
                let bindings = arg(1)?.list().ok_or(Error::new("let-syntax: bad bindings"))?;
                self.scopes.push(Vec::new());
                // letrec-syntax transformers see each other; let-syntax ones
                // only the enclosing scope.
                let depth = if name == "letrec-syntax" { self.scopes.len() } else { self.scopes.len() - 1 };
                let result = (|| {
                    for b in bindings {
                        let Some([Sexp::Sym(n), spec]) = b.list() else { return err("let-syntax: bad binding") };
                        let m = Macro::parse(strip(*n), spec, depth, self.module, self.file).map_err(Error::new)?;
                        self.scopes.last_mut().unwrap().push((*n, Binding::Macro(Rc::new(m))));
                    }
                    self.body(&items[2..])
                })();
                self.scopes.pop();
                result
            }
            "guard" => self.guard(arg(1)?, &items[2..]),
            "%with-escape" => {
                let f = self.expr(arg(1)?)?;
                Ok(Expr::Escape(Box::new(f), pos))
            }
            "match" => {
                let expanded = self.match_form(items)?;
                self.expr(&expanded)
            }
            "define" | "define-syntax" | "define-record-type" => {
                err(format!("{name} is only allowed at top level or at the start of a body"))
            }
            "require" | "provide" | "define-library" | "import" => err(format!("{name} is only allowed at top level")),
            "include" | "include-ci" | "cond-expand" => {
                let forms = self.spliced(name, items)?;
                let mut exprs = forms.iter().map(|(f, file)| self.in_file(*file, |c| c.expr(f))).collect::<R<Vec<_>>>()?;
                Ok(if exprs.len() == 1 { exprs.pop().unwrap() } else { Expr::Seq(exprs).or_void() })
            }
            _ => unreachable!("special form {name}"),
        }
    }

    fn lambda(&mut self, name: Rc<str>, params: &Sexp, body: &[Sexp]) -> R<Expr> {
        // Docstrings come before the keyword-argument prologue.
        let (doc, body) = match body {
            [Sexp::Str(doc), rest @ ..] if !rest.is_empty() => (Some(doc.clone()), rest),
            _ => (None, body),
        };
        if let Some((desugared, new_body)) = optional_formals(&name, params, body)? {
            let e = self.lambda(name, &desugared, &new_body)?;
            if let Expr::Lambda(f) = e {
                self.funcs[f].param_names = formals_display(params);
                self.funcs[f].pos = params.pos();
                self.funcs[f].doc = doc;
            }
            return Ok(e);
        }
        let parent = *self.fn_stack.last().unwrap();
        let f = self.new_fn(name, Some(parent));
        self.funcs[f].pos = params.pos();
        self.funcs[f].param_names = formals_display(params);
        self.funcs[f].doc = doc;
        self.fn_stack.push(f);
        let (fixed, rest) = match params {
            Sexp::Sym(r) => (vec![], Some(*r)),
            Sexp::List(items, tail, _) => (items.clone(), tail.as_ref().and_then(|t| t.sym())),
            _ => return err("lambda: bad parameter list"),
        };
        let mut scope = Vec::new();
        for p in &fixed {
            let v = self.new_var();
            self.funcs[f].params.push(v);
            scope.push((p.sym().ok_or(Error::new("lambda: parameter must be an identifier"))?, Binding::Var(v)));
        }
        if let Some(r) = rest {
            let v = self.new_var();
            self.funcs[f].rest = Some(v);
            scope.push((r, Binding::Var(v)));
        }
        self.scopes.push(scope);
        let body = self.body(body);
        self.scopes.pop();
        self.fn_stack.pop();
        self.funcs[f].body = Some(body?);
        Ok(Expr::Lambda(f))
    }

    // ----- inlining -----

    /// A call of global `g` passing procedures written in place (lambdas),
    /// expanded into the body of `g`'s root-module definition: there the
    /// calls of those procedures are their bodies, with no closure or call.
    /// Guarded: when `g` holds another value than the closure it held now,
    /// the call is made as written. Not in the root module itself: the
    /// prelude is compiled at every start, and its own such calls are few.
    fn inline_call(&mut self, g: u32, args: &[Sexp], pos: Pos) -> R<Option<Expr>> {
        let closure = self.vm.globals[g as usize];
        if self.module == ROOT_MODULE || self.inlining >= MAX_INLINING || !is_kind(closure, Kind::Closure) {
            return Ok(None);
        }
        let procs: Vec<_> = args.iter().map(|a| self.procedure_arg(a)).collect();
        if procs.iter().all(Option::is_none) {
            return Ok(None);
        }
        // SAFETY: a closure's first field is its code, which is never freed.
        let code = unsafe { &*field(closure.as_ptr(), 0).as_untraced_ptr::<Code>() };
        let Some(t) = self.template(code) else { return Ok(None) };
        if t.params.len() != args.len() {
            return Ok(None);
        }
        // Evaluate the other arguments once, for either branch.
        let (mut lets, mut scope, mut fresh, mut call_args) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for ((&alias, arg), proc) in t.params.iter().zip(args).zip(procs) {
            match proc {
                Some(ProcArg::Passed(s)) => {
                    call_args.push(self.subst_value(&s)?);
                    scope.push((alias, Binding::Subst(s)));
                }
                Some(ProcArg::Lambda(form)) => {
                    call_args.push(self.expr(&form)?);
                    let s = Rc::new(Subst { form, var: Some(self.new_var()), used: std::cell::Cell::new(false) });
                    fresh.push(s.clone());
                    scope.push((alias, Binding::Subst(s)));
                }
                None => {
                    let e = self.expr(arg)?;
                    let v = self.new_var();
                    lets.push((v, e));
                    call_args.push(Expr::Local(v));
                    scope.push((alias, Binding::Var(v)));
                }
            }
        }
        if let Some(r) = t.rest {
            let empty = Subst { form: list(vec![core("quote"), list(vec![])]), var: None, used: std::cell::Cell::new(false) };
            scope.push((r, Binding::Subst(Rc::new(empty))));
        }
        self.scopes.push(scope);
        self.inlining += 1;
        let inlined = self.body(&t.body);
        self.inlining -= 1;
        self.scopes.pop();
        // A procedure the body uses as a value is made once, at its start.
        let mut inlined = inlined?;
        for s in fresh.iter().filter(|s| s.used.get()) {
            let value = self.expr(&s.form)?;
            inlined = Expr::Let(vec![(s.var.unwrap(), value)], Box::new(inlined));
        }
        let same = Expr::Prim(Prim::EqP, vec![Expr::Global(g), Expr::Object(closure)], pos);
        let call = Expr::Call(Box::new(Expr::Global(g)), call_args, pos);
        let e = Expr::If(Box::new(same), Box::new(inlined), Box::new(call));
        Ok(Some(if lets.is_empty() { e } else { Expr::Let(lets, Box::new(e)) }))
    }

    /// `code`'s definition as an inline template, if it has a suitable one.
    fn template(&self, code: &Code) -> Option<Rc<Inline>> {
        code.inline
            .get_or_init(|| {
                let text = &self.vm.files.get(code.file as usize)?.text;
                let (form, _) = reader::read_next(text.get(code.definition? as usize..)?).ok()??;
                Inline::of(&form).map(Rc::new)
            })
            .clone()
    }

    /// `arg` if it is a procedure inlining can call in place: a lambda with
    /// plain parameters, written here or passed on by an enclosing inlining.
    /// Checked on every argument of calls of procedures, so cheaply first.
    fn procedure_arg(&mut self, arg: &Sexp) -> Option<ProcArg> {
        match arg {
            Sexp::Sym(sym) if self.inlining > 0 => match self.resolve(*sym) {
                Resolved::Local(Binding::Subst(s)) if s.var.is_some() => Some(ProcArg::Passed(s)),
                _ => None,
            },
            Sexp::List(items, None, _) if items.first().and_then(Sexp::sym).is_some_and(|h| strip(h) == self.lambda_sym) => {
                self.plain_lambda(arg).is_some().then(|| ProcArg::Lambda(arg.clone()))
            }
            _ => None,
        }
    }

    /// The parameters and body of `f` if it is `(lambda (param ...) body ...)`.
    fn plain_lambda<'s>(&mut self, f: &'s Sexp) -> Option<(Vec<u32>, &'s [Sexp])> {
        if self.special_of(f).as_deref() != Some("lambda") {
            return None;
        }
        match f.list()? {
            [_, Sexp::List(params, None, _), body @ ..] if !body.is_empty() => {
                Some((params.iter().map(Sexp::sym).collect::<Option<_>>()?, body))
            }
            _ => None,
        }
    }

    /// A call of `f` (an inlined procedure's parameter, if `subst`): of a
    /// lambda written in place, its body with the parameters bound.
    fn apply(&mut self, f: &Sexp, args: &[Sexp], pos: Pos, subst: Option<&Rc<Subst>>) -> R<Expr> {
        if let Some((params, body)) = self.plain_lambda(f)
            && params.len() == args.len()
        {
            let inits = args.iter().map(|a| self.expr(a)).collect::<R<Vec<_>>>()?;
            let vars: Vec<_> = params.iter().map(|_| self.new_var()).collect();
            self.scopes.push(params.iter().zip(&vars).map(|(p, v)| (*p, Binding::Var(*v))).collect());
            let body = self.body(body);
            self.scopes.pop();
            return Ok(Expr::Let(vars.into_iter().zip(inits).collect(), Box::new(body?)));
        }
        let callee = match subst {
            Some(s) => self.subst_value(s)?,
            None => self.expr(f)?,
        };
        let args = args.iter().map(|a| self.expr(a)).collect::<R<Vec<_>>>()?;
        Ok(Expr::Call(Box::new(callee), args, pos))
    }

    /// An inlined procedure's parameter used as a value: the procedure made
    /// once for the inlined body (see `inline_call`), or a constant.
    fn subst_value(&mut self, s: &Subst) -> R<Expr> {
        match s.var {
            Some(v) => {
                s.used.set(true);
                self.use_var(v);
                Ok(Expr::Local(v))
            }
            None => self.expr(&s.form),
        }
    }

    fn bindings<'s>(&self, b: &'s Sexp) -> R<Vec<(u32, &'s Sexp)>> {
        b.list()
            .ok_or(Error::new("bad binding list"))?
            .iter()
            .map(|pair| match pair.list() {
                Some([Sexp::Sym(name), value]) => Ok((*name, value)),
                _ => err(format!("bad binding {}", reader::display_sexp(pair))),
            })
            .collect()
    }

    fn let_form(&mut self, bindings: &Sexp, body: &[Sexp]) -> R<Expr> {
        let bindings = self.bindings(bindings)?;
        let inits = bindings.iter().map(|(_, e)| self.expr(e)).collect::<R<Vec<_>>>()?;
        let vars: Vec<_> = bindings.iter().map(|_| self.new_var()).collect();
        self.scopes.push(bindings.iter().zip(&vars).map(|((n, _), v)| (*n, Binding::Var(*v))).collect());
        let body = self.body(body);
        self.scopes.pop();
        Ok(Expr::Let(vars.into_iter().zip(inits).collect(), Box::new(body?)))
    }

    fn let_star(&mut self, bindings: &Sexp, body: &[Sexp]) -> R<Expr> {
        let list = self.bindings(bindings)?;
        let mut out = Vec::new();
        let depth = self.scopes.len();
        let result = (|| {
            for (name, init) in &list {
                let init = self.expr(init)?;
                let v = self.new_var();
                self.scopes.push(vec![(*name, Binding::Var(v))]);
                out.push((v, init));
            }
            self.body(body)
        })();
        self.scopes.truncate(depth);
        Ok(out.into_iter().rev().fold(result?, |acc, (v, init)| Expr::Let(vec![(v, init)], Box::new(acc))))
    }

    fn letrec(&mut self, bindings: &Sexp, body: &[Sexp]) -> R<Expr> {
        let list = self.bindings(bindings)?;
        let vars: Vec<_> = list.iter().map(|_| self.new_var()).collect();
        for v in &vars {
            // Initialised after closures in the group may have captured them.
            self.vars[*v].assigned = true;
        }
        self.scopes.push(list.iter().zip(&vars).map(|((n, _), v)| (*n, Binding::Var(*v))).collect());
        let result = (|| {
            let inits = list.iter().map(|(name, e)| self.named_expr(*name, e)).collect::<R<Vec<_>>>()?;
            let body = self.body(body)?;
            Ok(Expr::Letrec(vars.iter().copied().zip(inits).collect(), Box::new(body)))
        })();
        self.scopes.pop();
        result
    }

    /// Like `expr`, but names a lambda after its binding for error messages.
    fn named_expr(&mut self, name: u32, e: &Sexp) -> R<Expr> {
        let e = self.expand_head(e)?;
        if self.special_of(&e).as_deref() == Some("lambda") {
            let items = e.list().unwrap();
            if items.len() < 2 {
                return err("lambda: missing parameter list");
            }
            return self.lambda(display_name(name), &items[1], &items[2..]);
        }
        self.expr(&e)
    }

    fn named_let(&mut self, name: u32, bindings: &Sexp, body: &[Sexp], pos: Pos) -> R<Expr> {
        let binds = self.bindings(bindings)?;
        if !loopable(name, body) {
            // ((letrec ((name (lambda params body...))) name) inits...)
            let params = list(binds.iter().map(|(n, _)| Sexp::Sym(*n)).collect());
            let mut lambda = vec![core("lambda"), params];
            lambda.extend_from_slice(body);
            let letrec = list(vec![core("letrec"), list(vec![list(vec![Sexp::Sym(name), list(lambda)])]), Sexp::Sym(name)]);
            let mut call = vec![letrec];
            call.extend(binds.iter().map(|(_, e)| (*e).clone()));
            return self.expr(&Sexp::List(call, None, pos));
        }
        let inits = binds.iter().map(|(_, e)| self.expr(e)).collect::<R<Vec<_>>>()?;
        let params: Vec<_> = binds.iter().map(|_| self.new_var()).collect();
        let l = self.loops.len();
        self.loops.push(params.clone());
        let mut scope = vec![(name, Binding::Loop(l))];
        scope.extend(binds.iter().zip(&params).map(|((n, _), v)| (*n, Binding::Var(*v))));
        self.scopes.push(scope);
        let body = self.body(body);
        self.scopes.pop();
        Ok(Expr::Loop(l, inits, Box::new(body?)))
    }

    /// Whether `s` is the auxiliary syntax `name` (`else`, `=>`): that
    /// identifier, not shadowed by a local binding.
    fn aux(&self, s: &Sexp, name: &str) -> bool {
        s.is_sym(name) && s.sym().is_some_and(|sym| !matches!(self.resolve(sym), Resolved::Local(_)))
    }

    fn cond(&mut self, clauses: &[Sexp]) -> R<Expr> {
        crate::nested(|| self.cond_step(clauses))
    }

    fn cond_step(&mut self, clauses: &[Sexp]) -> R<Expr> {
        let Some((first, rest)) = clauses.split_first() else {
            return Ok(Expr::Void);
        };
        let clause = first.list().filter(|c| !c.is_empty()).ok_or(Error::new("cond: bad clause"))?;
        if self.aux(&clause[0], "else") {
            return self.seq(&clause[1..]);
        }
        if clause.len() == 1 {
            // (cond (test) rest...) => (or test (cond rest...))
            let test = self.expr(&clause[0])?;
            let rest = self.cond(rest)?;
            return Ok(self.or_exprs(test, rest));
        }
        if self.aux(&clause[1], "=>") {
            // (let ((t test)) (if t (f t) (cond rest...)))
            let test = self.expr(&clause[0])?;
            let f = self.expr(clause.get(2).ok_or(Error::new("cond: => needs a receiver"))?)?;
            let rest = self.cond(rest)?;
            let v = self.new_var();
            let call = Expr::Call(Box::new(f), vec![Expr::Local(v)], self.at(first.pos()));
            return Ok(Expr::Let(vec![(v, test)], Box::new(Expr::If(Box::new(Expr::Local(v)), Box::new(call), Box::new(rest)))));
        }
        let test = self.expr(&clause[0])?;
        let body = self.seq(&clause[1..])?;
        let rest = self.cond(rest)?;
        Ok(Expr::If(Box::new(test), Box::new(body), Box::new(rest)))
    }

    fn case(&mut self, items: &[Sexp]) -> R<Expr> {
        let key = self.expr(items.get(1).ok_or(Error::new("case: missing key"))?)?;
        let v = self.new_var();
        let mut result = Expr::Void;
        for clause in items[2..].iter().rev() {
            let clause = clause.list().filter(|c| !c.is_empty()).ok_or(Error::new("case: bad clause"))?;
            let body = match &clause[1..] {
                // (datums => f): f applied to the key.
                [arrow, f] if self.aux(arrow, "=>") => Expr::Call(Box::new(self.expr(f)?), vec![Expr::Local(v)], NO_POS),
                body => self.seq(body)?,
            };
            if self.aux(&clause[0], "else") {
                result = body;
                continue;
            }
            let data = clause[0].list().ok_or(Error::new("case: datums must be a list"))?;
            if let Some(n) = reader::dangling_label(&clause[0]) {
                return err(format!("#{n}# refers to a datum label outside these case datums"));
            }
            let eqv = self.variable(intern_core("eqv?"))?;
            let Expr::Global(eqv) = eqv else { unreachable!() };
            let test = data.iter().rev().fold(Expr::Const(Sexp::Bool(false)), |acc, d| {
                let d = strip_sexp(d);
                // Immediates compare with eq?; numbers that may be boxed use eqv?.
                let cmp = match d {
                    Sexp::Int(i) if crate::value::Value::fixnum(i).is_none() => {
                        Expr::Call(Box::new(Expr::Global(eqv)), vec![Expr::Local(v), Expr::Const(d)], NO_POS)
                    }
                    Sexp::Float(_) | Sexp::Str(_) | Sexp::BigInt(_) | Sexp::Ratio(_) | Sexp::Complex(..) => {
                        Expr::Call(Box::new(Expr::Global(eqv)), vec![Expr::Local(v), Expr::Const(d)], NO_POS)
                    }
                    _ => Expr::Prim(Prim::EqP, vec![Expr::Local(v), Expr::Const(d)], NO_POS),
                };
                Expr::If(Box::new(cmp), Box::new(Expr::Const(Sexp::Bool(true))), Box::new(acc))
            });
            result = Expr::If(Box::new(test), Box::new(body), Box::new(result));
        }
        Ok(Expr::Let(vec![(v, key)], Box::new(result)))
    }

    fn and(&mut self, parts: &[Sexp]) -> R<Expr> {
        Ok(match parts {
            [] => Expr::Const(Sexp::Bool(true)),
            [one] => self.expr(one)?,
            _ => Expr::And(parts.iter().map(|part| self.expr(part)).collect::<R<_>>()?),
        })
    }

    fn or(&mut self, parts: &[Sexp]) -> R<Expr> {
        Ok(match parts {
            [] => Expr::Const(Sexp::Bool(false)),
            [one] => self.expr(one)?,
            _ => Expr::Or(parts.iter().map(|part| self.expr(part)).collect::<R<_>>()?),
        })
    }

    fn or_exprs(&mut self, first: Expr, rest: Expr) -> Expr {
        let v = self.new_var();
        Expr::Let(vec![(v, first)], Box::new(Expr::If(Box::new(Expr::Local(v)), Box::new(Expr::Local(v)), Box::new(rest))))
    }

    /// `(guard (var clause...) body...)`: clauses as in `cond`; without a
    /// matching clause the condition is raised again.
    fn guard(&mut self, spec: &Sexp, body: &[Sexp]) -> R<Expr> {
        let spec = spec.list().filter(|s| !s.is_empty()).ok_or(Error::new("guard: expected (var clause ...)"))?;
        let var_sym = spec[0].sym().ok_or(Error::new("guard: expected a variable"))?;
        let body = self.body(body)?;
        let var = self.new_var();
        self.scopes.push(vec![(var_sym, Binding::Var(var))]);
        let mut clauses = spec[1..].to_vec();
        clauses.push(list(vec![core("else"), list(vec![core("raise"), Sexp::Sym(var_sym)])]));
        let handler = self.cond(&clauses);
        self.scopes.pop();
        Ok(Expr::Guard { var, body: Box::new(body), handler: Box::new(handler?) })
    }

    // ----- match -----

    /// `(match e [pattern body ...] [pattern #:when guard body ...] ...)` as a
    /// `cond` over inline tests; bindings are accessor expressions on the
    /// subject, bound with `let*` once a clause's tests pass.
    fn match_form(&mut self, items: &[Sexp]) -> R<Sexp> {
        let subject = items.get(1).ok_or(Error::new("match: missing subject"))?;
        let v = Sexp::Sym(make_alias(intern("subject"), 0, ROOT_MODULE));
        let mut clauses = vec![core("cond")];
        for clause in &items[2..] {
            let parts = clause.list().filter(|p| !p.is_empty()).ok_or(Error::new("match: clause must be [pattern body ...]"))?;
            let (guard, body) = match &parts[1..] {
                [Sexp::Keyword(k), g, body @ ..] if &*symbol_name(*k) == "when" => (Some(g.clone()), body),
                body => (None, body),
            };
            let mut tests = vec![core("and")];
            let mut binds = Vec::new();
            self.pattern(&parts[0], v.clone(), &mut tests, &mut binds)?;
            let let_binds = list(binds.into_iter().map(|(name, e)| list(vec![name, e])).collect());
            if let Some(g) = guard {
                tests.push(list(vec![core("let*"), let_binds.clone(), g]));
            }
            let mut b = vec![core("let*"), let_binds];
            b.extend(body.iter().cloned());
            if body.is_empty() {
                b.push(list(vec![core("void")]));
            }
            clauses.push(list(vec![list(tests), list(b)]));
        }
        clauses.push(list(vec![core("else"), list(vec![core("%match-error"), v.clone()])]));
        Ok(list(vec![core("let"), list(vec![list(vec![v, subject.clone()])]), list(clauses)]))
    }

    fn pattern(&mut self, p: &Sexp, acc: Sexp, tests: &mut Vec<Sexp>, binds: &mut Vec<(Sexp, Sexp)>) -> R<()> {
        crate::nested(|| self.pattern_step(p, acc, tests, binds))
    }

    fn pattern_step(&mut self, p: &Sexp, acc: Sexp, tests: &mut Vec<Sexp>, binds: &mut Vec<(Sexp, Sexp)>) -> R<()> {
        let call = |f: &str, args: Vec<Sexp>| list([vec![core(f)], args].concat());
        match p {
            Sexp::Sym(s) => match &*symbol_name(strip(*s)) {
                "_" => {}
                "..." => return err("match: misplaced ..."),
                _ => {
                    if binds.iter().any(|(b, _)| b == p) {
                        return err(format!("match: duplicate pattern variable {}", display_name(*s)));
                    }
                    binds.push((p.clone(), acc));
                }
            },
            Sexp::Vector(items) => self.vector_pattern(items, acc, tests, binds)?,
            Sexp::List(items, tail, _) => {
                let head = items.first().and_then(Sexp::sym).map(|h| symbol_name(strip(h)));
                match (head.as_deref(), tail) {
                    (Some("quote"), None) => tests.push(call("equal?", vec![acc, p.clone()])),
                    (Some("?"), None) if items.len() >= 2 => {
                        tests.push(list(vec![items[1].clone(), acc.clone()]));
                        for sub in &items[2..] {
                            self.pattern(sub, acc.clone(), tests, binds)?;
                        }
                    }
                    (Some("and"), None) => {
                        for sub in &items[1..] {
                            self.pattern(sub, acc.clone(), tests, binds)?;
                        }
                    }
                    (Some(op @ ("or" | "not")), None) => {
                        let mut alternatives = vec![core(if op == "or" { "or" } else { "and" })];
                        for sub in &items[1..] {
                            let mut t = vec![core("and")];
                            let mut b = Vec::new();
                            self.pattern(sub, acc.clone(), &mut t, &mut b)?;
                            if !b.is_empty() {
                                return err(format!("match: {op} patterns cannot bind variables"));
                            }
                            alternatives.push(list(t));
                        }
                        let test = list(alternatives);
                        tests.push(if op == "not" { call("not", vec![test]) } else { test });
                    }
                    (Some("cons"), None) if items.len() == 3 => {
                        tests.push(call("pair?", vec![acc.clone()]));
                        self.pattern(&items[1], call("car", vec![acc.clone()]), tests, binds)?;
                        self.pattern(&items[2], call("cdr", vec![acc]), tests, binds)?;
                    }
                    (Some("list"), None) => self.list_pattern(&items[1..], None, acc, tests, binds)?,
                    (Some("vector"), None) => self.vector_pattern(&items[1..], acc, tests, binds)?,
                    (Some(_), None) if self.record_pattern(&items[0], items.len() - 1) => {
                        let rtd = items[0].clone();
                        tests.push(call("%record?", vec![acc.clone(), rtd.clone()]));
                        for (i, sub) in items[1..].iter().enumerate() {
                            let field = call("%record-ref", vec![acc.clone(), rtd.clone(), Sexp::Int(i as i64)]);
                            self.pattern(sub, field, tests, binds)?;
                        }
                    }
                    _ => self.list_pattern(items, tail.as_deref(), acc, tests, binds)?,
                }
            }
            literal => tests.push(call("equal?", vec![acc, literal.clone()])),
        }
        Ok(())
    }

    fn record_pattern(&mut self, head: &Sexp, nfields: usize) -> bool {
        let Some(sym) = head.sym() else { return false };
        match self.resolve(sym) {
            Resolved::Global { module, sym } => match self.vm.lookup_global(module, sym) {
                Some(GlobalBinding::Var(g)) => self.vm.record_types.get(&g) == Some(&nfields),
                _ => false,
            },
            Resolved::Local(_) => false,
        }
    }

    fn list_pattern(
        &mut self,
        items: &[Sexp],
        tail: Option<&Sexp>,
        acc: Sexp,
        tests: &mut Vec<Sexp>,
        binds: &mut Vec<(Sexp, Sexp)>,
    ) -> R<()> {
        let call = |f: &str, args: Vec<Sexp>| list([vec![core(f)], args].concat());
        let mut cur = acc;
        for (i, item) in items.iter().enumerate() {
            if items.get(i + 1).is_some_and(|n| n.is_sym("...")) {
                if i + 2 != items.len() || tail.is_some() {
                    return err("match: ... must end the list pattern");
                }
                // Every remaining element matches `item`; variables bind to lists.
                let x = Sexp::Sym(make_alias(intern("x"), 0, ROOT_MODULE));
                let mut t = vec![core("and")];
                let mut b = Vec::new();
                self.pattern(item, x.clone(), &mut t, &mut b)?;
                let lambda = |body: Sexp| list(vec![core("lambda"), list(vec![x.clone()]), body]);
                tests.push(call("%list-all?", vec![lambda(list(t)), cur.clone()]));
                for (name, e) in b {
                    binds.push((name, call("map", vec![lambda(e), cur.clone()])));
                }
                return Ok(());
            }
            tests.push(call("pair?", vec![cur.clone()]));
            self.pattern(item, call("car", vec![cur.clone()]), tests, binds)?;
            cur = call("cdr", vec![cur]);
        }
        match tail {
            Some(t) => self.pattern(t, cur, tests, binds),
            None => {
                tests.push(call("null?", vec![cur]));
                Ok(())
            }
        }
    }

    fn vector_pattern(&mut self, items: &[Sexp], acc: Sexp, tests: &mut Vec<Sexp>, binds: &mut Vec<(Sexp, Sexp)>) -> R<()> {
        let call = |f: &str, args: Vec<Sexp>| list([vec![core(f)], args].concat());
        tests.push(call("vector?", vec![acc.clone()]));
        tests.push(call("=", vec![call("vector-length", vec![acc.clone()]), Sexp::Int(items.len() as i64)]));
        for (i, item) in items.iter().enumerate() {
            self.pattern(item, call("vector-ref", vec![acc.clone(), Sexp::Int(i as i64)]), tests, binds)?;
        }
        Ok(())
    }

    // ----- code generation -----

    fn generate(&mut self, f: FnId) -> R<u32> {
        let body = self.funcs[f].body.take().expect("function body");
        let mut g = Gen::default();
        let info = &self.funcs[f];
        let (params, rest, free, name) = (info.params.clone(), info.rest, info.free.clone(), info.name.clone());
        let (pos, param_names, doc, file, definition) = (info.pos, info.param_names.clone(), info.doc.clone(), info.file, info.definition);
        for p in params.iter().chain(rest.iter()) {
            let r = g.alloc();
            g.locs.insert(*p, Loc::Reg(r));
        }
        if free.len() > usize::from(u16::MAX) {
            return err(format!("{name}: captures more than {} variables", u16::MAX));
        }
        for (i, v) in free.iter().enumerate() {
            g.locs.insert(*v, Loc::Cap(i as u16));
        }
        for p in params.iter().chain(rest.iter()) {
            if self.boxed(*p) {
                let Loc::Reg(r) = g.locs[p] else { unreachable!() };
                g.emit(Op::MkBox { r });
            }
        }
        self.expr_to(&mut g, &body, Dest::Return, &[])?;
        // Parameters and call arguments each take a register, so their
        // counts fit a `Reg` when the frame does.
        let frame_size = Reg::try_from(g.max)
            .map_err(|_| Error::new(format!("{name}: needs more than {} registers for its variables and temporaries", Reg::MAX)))?;
        let code = Code {
            name,
            ops: g.ops,
            consts: g.consts,
            nparams: params.len() as u16,
            rest: rest.is_some(),
            frame_size: frame_size.max(1),
            captures: Vec::new(),
            file,
            spans: g.spans,
            pos,
            params: param_names,
            doc,
            jit: Default::default(),
            generation: self.vm.modules[self.module as usize].generation,
            definition,
            inline: Default::default(),
        };
        Ok(self.vm.add_code(code))
    }

    fn boxed(&self, v: VarId) -> bool {
        self.vars[v].assigned && self.vars[v].captured
    }

    fn constant(&mut self, g: &mut Gen, s: &Sexp, dst: Reg) {
        if let Sexp::Int(i) = s
            && let Ok(i) = i32::try_from(*i)
        {
            g.emit(Op::LoadI { dst, i });
            return;
        }
        let v = self.vm.constant(s);
        g.consts.push(v);
        g.emit(Op::LoadK { dst, k: g.consts.len() as u32 - 1 });
    }

    /// Register holding `e`'s value, avoiding a move for unboxed locals.
    fn operand(&mut self, g: &mut Gen, e: &Expr) -> R<Reg> {
        if let Expr::Local(v) = e
            && let Some(Loc::Reg(r)) = g.locs.get(v)
            && !self.boxed(*v)
        {
            return Ok(*r);
        }
        let r = g.alloc();
        self.expr_to(g, e, Dest::Reg(r), &[])?;
        Ok(r)
    }

    fn expr_to(&mut self, g: &mut Gen, e: &Expr, dest: Dest, tails: &[LoopId]) -> R<()> {
        let mark = g.next;
        let result = match e {
            Expr::Const(_) | Expr::Object(_) | Expr::Void | Expr::Local(_) | Expr::Global(_) => self.expr_inner(g, e, dest, tails),
            _ => crate::nested(|| self.expr_inner(g, e, dest, tails)),
        };
        g.next = mark;
        result
    }

    fn expr_inner(&mut self, g: &mut Gen, e: &Expr, dest: Dest, tails: &[LoopId]) -> R<()> {
        match e {
            Expr::Const(s) => {
                if !matches!(dest, Dest::Effect) {
                    let r = g.target(dest);
                    self.constant(g, s, r);
                    g.finish(r, dest);
                }
            }
            Expr::Void => self.void_result(g, dest),
            Expr::Object(v) => {
                if !matches!(dest, Dest::Effect) {
                    let r = g.target(dest);
                    g.consts.push(*v);
                    g.emit(Op::LoadK { dst: r, k: g.consts.len() as u32 - 1 });
                    g.finish(r, dest);
                }
            }
            Expr::Local(v) => {
                if matches!(dest, Dest::Effect) {
                    return Ok(());
                }
                let boxed = self.boxed(*v);
                match g.locs[v] {
                    Loc::Reg(r) if !boxed => g.finish(r, dest),
                    Loc::Reg(r) => {
                        let d = g.target(dest);
                        g.emit(Op::Unbox { dst: d, r });
                        g.finish(d, dest);
                    }
                    Loc::Cap(i) => {
                        let d = g.target(dest);
                        g.emit(if boxed { Op::GetCB { dst: d, i } } else { Op::GetC { dst: d, i } });
                        g.finish(d, dest);
                    }
                }
            }
            Expr::Global(gi) => {
                let d = g.target(dest);
                g.emit(Op::GetG { dst: d, g: *gi });
                g.finish(d, dest);
            }
            Expr::SetLocal(v, value) => {
                let boxed = self.boxed(*v);
                match g.locs[v] {
                    Loc::Reg(r) if !boxed => self.expr_to(g, value, Dest::Reg(r), &[])?,
                    Loc::Reg(r) => {
                        let x = self.operand(g, value)?;
                        g.emit(Op::SetBox { r, src: x });
                    }
                    Loc::Cap(i) => {
                        let x = self.operand(g, value)?;
                        g.emit(Op::SetCB { i, src: x });
                    }
                }
                self.void_result(g, dest);
            }
            Expr::SetGlobal(gi, value) | Expr::DefGlobal(gi, value) => {
                let x = self.operand(g, value)?;
                g.emit(Op::SetG { g: *gi, src: x });
                self.void_result(g, dest);
            }
            Expr::If(c, t, f) => {
                let (c, t, f) = match &**c {
                    Expr::Prim(Prim::Not, args, _) => (&args[0], f, t),
                    _ => (&**c, t, f),
                };
                let else_jumps = self.test(g, c)?;
                self.expr_to(g, t, dest, tails)?;
                let end = (!matches!(dest, Dest::Return)).then(|| g.emit_jump());
                g.patch_all(&else_jumps);
                self.expr_to(g, f, dest, tails)?;
                if let Some(end) = end {
                    g.patch_all(&[end]);
                }
            }
            Expr::And(es) => {
                let (last, init) = es.split_last().expect("non-empty conjunction");
                let false_value = Expr::Const(Sexp::Bool(false));
                let mut false_jumps = Vec::new();
                let mut end_jumps = Vec::new();
                for e in init {
                    if let Expr::Prim(Prim::Not, args, _) = e {
                        // As for If, invert the branches, not the comparison result.
                        let continue_jumps = self.test(g, &args[0])?;
                        self.expr_to(g, &false_value, dest, &[])?;
                        if !matches!(dest, Dest::Return) {
                            end_jumps.push(g.emit_jump());
                        }
                        g.patch_all(&continue_jumps);
                    } else {
                        false_jumps.extend(self.test(g, e)?);
                    }
                }
                self.expr_to(g, last, dest, tails)?;
                if !false_jumps.is_empty() {
                    if !matches!(dest, Dest::Return) {
                        end_jumps.push(g.emit_jump());
                    }
                    g.patch_all(&false_jumps);
                    self.expr_to(g, &false_value, dest, &[])?;
                }
                g.patch_all(&end_jumps);
            }
            Expr::Or(es) => {
                let (last, init) = es.split_last().expect("non-empty disjunction");
                let r = g.alloc();
                let mut end_jumps = Vec::new();
                for e in init {
                    self.expr_to(g, e, Dest::Reg(r), &[])?;
                    let next = g.ops.len();
                    g.emit(Op::Jf { c: r, t: 0 });
                    g.finish(r, dest);
                    if !matches!(dest, Dest::Return) {
                        end_jumps.push(g.emit_jump());
                    }
                    g.patch_all(&[next]);
                }
                self.expr_to(g, last, dest, tails)?;
                g.patch_all(&end_jumps);
            }
            Expr::Seq(es) => {
                let (last, init) = es.split_last().expect("non-empty sequence");
                for e in init {
                    self.expr_to(g, e, Dest::Effect, &[])?;
                }
                self.expr_to(g, last, dest, tails)?;
            }
            Expr::Lambda(f) => {
                let code = self.generate(*f)?;
                let captures = self.funcs[*f]
                    .free
                    .iter()
                    .map(|v| match g.locs[v] {
                        Loc::Reg(r) => CapSrc::Reg(r),
                        Loc::Cap(i) => CapSrc::Cap(i),
                    })
                    .collect();
                self.vm.set_captures(code, captures);
                if !matches!(dest, Dest::Effect) {
                    let d = g.target(dest);
                    g.emit(Op::Closure { dst: d, code });
                    g.finish(d, dest);
                }
            }
            Expr::Call(f, args, pos) => {
                let saved = g.at(*pos);
                // For global callees, use the destination as the call base when it
                // is the topmost register, so the result needs no move. The callee
                // is only stored at call time, so the destination (which may be a
                // variable read by the arguments) is not clobbered early.
                let base = match dest {
                    Dest::Reg(d) if u32::from(d) + 1 == g.next && matches!(**f, Expr::Global(_)) => d,
                    _ => g.alloc(),
                };
                let global = match **f {
                    Expr::Global(gi) => Some(gi),
                    _ => {
                        self.expr_to(g, f, Dest::Reg(base), &[])?;
                        None
                    }
                };
                for a in args {
                    let slot = g.alloc();
                    self.expr_to(g, a, Dest::Reg(slot), &[])?;
                }
                let n = args.len() as u16;
                let tail = matches!(dest, Dest::Return);
                g.emit(match (global, tail) {
                    (Some(gi), true) => Op::TailCallG { base, n, g: gi },
                    (Some(gi), false) => Op::CallG { base, n, g: gi },
                    (None, true) => Op::TailCall { base, n },
                    (None, false) => Op::Call { base, n },
                });
                if !tail {
                    g.finish(base, dest);
                }
                g.pos = saved;
            }
            Expr::Prim(p, args, pos) => {
                let saved = g.at(*pos);
                self.prim(g, *p, args, dest)?;
                g.pos = saved;
            }
            Expr::Let(bindings, body) => {
                for (v, init) in bindings {
                    let r = g.alloc();
                    self.expr_to(g, init, Dest::Reg(r), &[])?;
                    g.locs.insert(*v, Loc::Reg(r));
                    if self.boxed(*v) {
                        g.emit(Op::MkBox { r });
                    }
                }
                self.expr_to(g, body, dest, tails)?;
            }
            Expr::Letrec(bindings, body) => {
                for (v, _) in bindings {
                    let r = g.alloc();
                    g.locs.insert(*v, Loc::Reg(r));
                    if self.boxed(*v) {
                        self.constant(g, &Sexp::Bool(false), r);
                        g.emit(Op::MkBox { r });
                    }
                }
                for (v, init) in bindings {
                    let Loc::Reg(r) = g.locs[v] else { unreachable!() };
                    if self.boxed(*v) {
                        let x = self.operand(g, init)?;
                        g.emit(Op::SetBox { r, src: x });
                    } else {
                        self.expr_to(g, init, Dest::Reg(r), &[])?;
                    }
                }
                self.expr_to(g, body, dest, tails)?;
            }
            Expr::Loop(l, inits, body) => {
                let params = self.loops[*l].clone();
                let regs: Vec<Reg> = params.iter().map(|_| g.alloc()).collect();
                for (init, r) in inits.iter().zip(&regs) {
                    self.expr_to(g, init, Dest::Reg(*r), &[])?;
                }
                for (v, r) in params.iter().zip(&regs) {
                    g.locs.insert(*v, Loc::Reg(*r));
                }
                let head = g.ops.len() as u32;
                for (v, r) in params.iter().zip(&regs) {
                    if self.boxed(*v) {
                        g.emit(Op::MkBox { r: *r });
                    }
                }
                g.loops.insert(*l, (head, regs));
                let mut inner = tails.to_vec();
                inner.push(*l);
                self.expr_to(g, body, dest, &inner)?;
            }
            Expr::LoopCall(l, args) => {
                if !tails.contains(l) {
                    return err("internal error: loop call outside tail position");
                }
                let (head, regs) = g.loops[l].clone();
                if regs.len() != args.len() {
                    return err(format!("loop expects {} arguments, got {}", regs.len(), args.len()));
                }
                // Evaluate all arguments before assigning (parallel assignment).
                let temps: Vec<Reg> = args.iter().map(|a| self.operand(g, a)).collect::<R<_>>()?;
                let staged: Vec<Reg> = temps
                    .iter()
                    .zip(&regs)
                    .map(|(t, r)| {
                        if t == r || !regs.contains(t) {
                            *t
                        } else {
                            let s = g.alloc();
                            g.emit(Op::Mov { dst: s, src: *t });
                            s
                        }
                    })
                    .collect();
                for (s, r) in staged.iter().zip(&regs) {
                    if s != r {
                        g.emit(Op::Mov { dst: *r, src: *s });
                    }
                }
                g.emit(Op::Loop { t: head });
            }
            Expr::Escape(f, pos) => {
                let saved = g.at(*pos);
                let res = g.alloc();
                let base = g.alloc();
                let k = g.alloc();
                g.emit(Op::PushEscape { k, dst: res, t: 0 });
                let push = g.ops.len() - 1;
                self.expr_to(g, f, Dest::Reg(base), &[])?;
                g.emit(Op::Call { base, n: 1 });
                g.emit(Op::Mov { dst: res, src: base });
                g.emit(Op::PopHandler);
                let here = g.ops.len() as u32;
                if let Op::PushEscape { t, .. } = &mut g.ops[push] {
                    *t = here;
                }
                g.finish(res, dest);
                g.pos = saved;
            }
            Expr::Guard { var, body, handler } => {
                let res = g.alloc();
                let cond = g.alloc();
                g.emit(Op::PushHandler { dst: cond, t: 0 });
                let push = g.ops.len() - 1;
                self.expr_to(g, body, Dest::Reg(res), &[])?;
                g.emit(Op::PopHandler);
                let end = g.emit_jump();
                let here = g.ops.len() as u32;
                if let Op::PushHandler { t, .. } = &mut g.ops[push] {
                    *t = here;
                }
                g.locs.insert(*var, Loc::Reg(cond));
                if self.boxed(*var) {
                    g.emit(Op::MkBox { r: cond });
                }
                self.expr_to(g, handler, Dest::Reg(res), &[])?;
                g.patch_all(&[end]);
                g.finish(res, dest);
            }
        }
        Ok(())
    }

    fn void_result(&mut self, g: &mut Gen, dest: Dest) {
        if !matches!(dest, Dest::Effect) {
            let r = g.target(dest);
            g.consts.push(crate::value::Value::VOID);
            g.emit(Op::LoadK { dst: r, k: g.consts.len() as u32 - 1 });
            g.finish(r, dest);
        }
    }

    fn prim(&mut self, g: &mut Gen, p: Prim, args: &[Expr], dest: Dest) -> R<()> {
        // (+ x k) / (- x k) with a small constant.
        if let (Prim::Add | Prim::Sub, [a, Expr::Const(Sexp::Int(k))]) = (p, args) {
            let k = if p == Prim::Sub { -k } else { *k };
            if let Ok(i) = i16::try_from(k) {
                let a = self.operand(g, a)?;
                let d = g.target(dest);
                g.emit(Op::AddI { dst: d, a, i });
                g.finish(d, dest);
                return Ok(());
            }
        }
        let regs = args.iter().map(|a| self.operand(g, a)).collect::<R<Vec<_>>>()?;
        if p == Prim::VSet {
            g.emit(Op::VSet { v: regs[0], i: regs[1], x: regs[2] });
            self.void_result(g, dest);
            return Ok(());
        }
        let d = g.target(dest);
        let (a, b) = (regs[0], regs.get(1).copied().unwrap_or(0));
        g.emit(match p {
            Prim::Add => Op::Add { dst: d, a, b },
            Prim::Sub => Op::Sub { dst: d, a, b },
            Prim::Mul => Op::Mul { dst: d, a, b },
            Prim::Quo => Op::Quo { dst: d, a, b },
            Prim::Rem => Op::Rem { dst: d, a, b },
            Prim::Mod => Op::Mod { dst: d, a, b },
            Prim::Lt => Op::Lt { dst: d, a, b },
            Prim::Gt => Op::Lt { dst: d, a: b, b: a },
            Prim::Le => Op::Le { dst: d, a, b },
            Prim::Ge => Op::Le { dst: d, a: b, b: a },
            Prim::NumEq => Op::NumEq { dst: d, a, b },
            Prim::Car => Op::Car { dst: d, a },
            Prim::Cdr => Op::Cdr { dst: d, a },
            Prim::Cons => Op::Cons { dst: d, a, b },
            Prim::NullP => Op::NullP { dst: d, a },
            Prim::PairP => Op::PairP { dst: d, a },
            Prim::Not => Op::Not { dst: d, a },
            Prim::EqP => Op::EqP { dst: d, a, b },
            Prim::VRef => Op::VRef { dst: d, v: a, i: b },
            Prim::VSet => unreachable!(),
        });
        g.finish(d, dest);
        Ok(())
    }

    /// Emit a test that falls through when `c` is true; returns the jumps
    /// taken when it is false, to be patched to the else branch.
    fn test(&mut self, g: &mut Gen, c: &Expr) -> R<Vec<usize>> {
        match c {
            Expr::If(..) | Expr::And(_) => crate::nested(|| self.test_step(g, c)),
            _ => self.test_step(g, c),
        }
    }

    fn test_step(&mut self, g: &mut Gen, c: &Expr) -> R<Vec<usize>> {
        let mark = g.next;
        let saved = g.pos;
        if let Expr::Prim(_, _, pos) = c {
            g.at(*pos);
        }
        let jumps = match c {
            Expr::Const(Sexp::Bool(true)) => vec![],
            Expr::Prim(p @ (Prim::Lt | Prim::Gt | Prim::NumEq), args, _) if matches!(&args[1], Expr::Const(Sexp::Int(k)) if i16::try_from(*k).is_ok()) =>
            {
                let Expr::Const(Sexp::Int(k)) = args[1] else { unreachable!() };
                let a = self.operand(g, &args[0])?;
                let i = k as i16;
                g.emit(match p {
                    Prim::Lt => Op::JNLtI { a, i, t: 0 },
                    Prim::Gt => Op::JNGtI { a, i, t: 0 },
                    _ => Op::JNEqI { a, i, t: 0 },
                });
                vec![g.ops.len() - 1]
            }
            Expr::Prim(p @ (Prim::Lt | Prim::Gt | Prim::Le | Prim::Ge | Prim::NumEq | Prim::EqP), args, _) => {
                let a = self.operand(g, &args[0])?;
                let b = self.operand(g, &args[1])?;
                g.emit(match p {
                    Prim::Lt => Op::JNLt { a, b, t: 0 },
                    Prim::Gt => Op::JNLt { a: b, b: a, t: 0 },
                    Prim::Le => Op::JNLe { a, b, t: 0 },
                    Prim::Ge => Op::JNLe { a: b, b: a, t: 0 },
                    Prim::NumEq => Op::JNNumEq { a, b, t: 0 },
                    _ => Op::JNEq { a, b, t: 0 },
                });
                vec![g.ops.len() - 1]
            }
            Expr::Prim(p @ (Prim::NullP | Prim::PairP), args, _) => {
                let a = self.operand(g, &args[0])?;
                g.emit(if *p == Prim::NullP { Op::JNNull { a, t: 0 } } else { Op::JNPair { a, t: 0 } });
                vec![g.ops.len() - 1]
            }
            Expr::Prim(Prim::Not, args, _) => {
                let a = self.operand(g, &args[0])?;
                g.emit(Op::Jt { c: a, t: 0 });
                vec![g.ops.len() - 1]
            }
            Expr::And(es) => {
                let mut jumps = Vec::new();
                for e in es {
                    jumps.extend(self.test(g, e)?);
                }
                jumps
            }
            // Both failures go to else without materializing a boolean.
            Expr::If(a, b, f) if matches!(**f, Expr::Const(Sexp::Bool(false))) => {
                let mut jumps = self.test(g, a)?;
                jumps.extend(self.test(g, b)?);
                jumps
            }
            _ => {
                let r = self.operand(g, c)?;
                g.emit(Op::Jf { c: r, t: 0 });
                vec![g.ops.len() - 1]
            }
        };
        g.next = mark;
        g.pos = saved;
        Ok(jumps)
    }
}

trait OrVoid {
    fn or_void(self) -> Expr;
}
impl OrVoid for Expr {
    fn or_void(self) -> Expr {
        if matches!(&self, Expr::Seq(v) if v.is_empty()) { Expr::Void } else { self }
    }
}

/// Like `Sexp`, use an iterative fallback for deeply nested expressions.
impl Drop for Expr {
    #[inline]
    fn drop(&mut self) {
        if matches!(self, Expr::Const(_) | Expr::Object(_) | Expr::Local(_) | Expr::Global(_) | Expr::Lambda(_) | Expr::Void)
            || stacker::remaining_stack().is_some_and(|remaining| remaining >= crate::STACK_RED_ZONE)
        {
            return;
        }
        drop_expr(self);
    }
}

#[cold]
#[inline(never)]
fn drop_expr(e: &mut Expr) {
    fn detach(e: &mut Expr, stack: &mut Vec<Expr>) {
        let mut take = |x: &mut Expr| {
            if !matches!(x, Expr::Const(_) | Expr::Object(_) | Expr::Local(_) | Expr::Global(_) | Expr::Lambda(_) | Expr::Void) {
                stack.push(std::mem::replace(x, Expr::Void));
            }
        };
        match e {
            Expr::SetLocal(_, x) | Expr::SetGlobal(_, x) | Expr::DefGlobal(_, x) | Expr::Escape(x, _) => take(x),
            Expr::If(c, t, f) => [c, t, f].into_iter().for_each(|x| take(x)),
            // Draining prevents the processed node's drop from scanning its children again.
            Expr::And(xs) | Expr::Or(xs) | Expr::Seq(xs) | Expr::Prim(_, xs, _) | Expr::LoopCall(_, xs) => {
                xs.drain(..).for_each(|mut x| take(&mut x))
            }
            Expr::Call(f, xs, _) | Expr::Loop(_, xs, f) => {
                take(f);
                xs.drain(..).for_each(|mut x| take(&mut x));
            }
            Expr::Let(bs, body) | Expr::Letrec(bs, body) => {
                take(body);
                bs.drain(..).for_each(|(_, mut x)| take(&mut x));
            }
            Expr::Guard { body, handler, .. } => {
                take(body);
                take(handler);
            }
            Expr::Const(_) | Expr::Object(_) | Expr::Local(_) | Expr::Global(_) | Expr::Lambda(_) | Expr::Void => {}
        }
    }
    let mut stack = Vec::new();
    detach(e, &mut stack);
    while let Some(mut e) = stack.pop() {
        detach(&mut e, &mut stack);
    }
}

fn intern_core(name: &str) -> u32 {
    core(name).sym().unwrap()
}

enum PrimForm {
    Direct(Prim),
    Fold(Prim),
    Negate,
    Zero,
}

fn prim_form(name: &str, nargs: usize) -> Option<PrimForm> {
    match (name, nargs) {
        ("+" | "*", n) if n > 2 => Some(PrimForm::Fold(prim(name, 2)?)),
        ("-", 1) => Some(PrimForm::Negate),
        ("zero?", 1) => Some(PrimForm::Zero),
        _ => prim(name, nargs).map(PrimForm::Direct),
    }
}

/// What an inlined procedure's parameter stands for (`Binding::Subst`).
struct Subst {
    /// A lambda, called in place, or a constant.
    form: Sexp,
    /// For a lambda: the variable holding it made as a procedure, if `used`.
    var: Option<VarId>,
    used: std::cell::Cell<bool>,
}

/// A procedure argument to an inlined call (`Compiler::procedure_arg`).
enum ProcArg {
    Lambda(Sexp),
    /// One passed on by an enclosing inlined call.
    Passed(Rc<Subst>),
}

/// Inlined calls nest at most this deep (`map` inlines `%map1`; a lambda
/// passed to it may call `map` again).
const MAX_INLINING: usize = 4;
/// Definitions larger than this (in nodes) are not inlined.
const MAX_INLINE_SIZE: usize = 80;

/// A small procedure defined at the root module's top level, for inlining
/// calls that pass it lambdas (`Compiler::inline_call`). Renamed as a
/// macro's template is: its free identifiers keep meaning the root module's
/// bindings, and those it binds cannot capture the caller's. Nested copies
/// share these names; each shadows the enclosing one, as a lexical scope.
#[derive(Debug)]
pub struct Inline {
    params: Vec<u32>,
    rest: Option<u32>,
    body: Vec<Sexp>,
}

impl Inline {
    /// The template of `(define (name param ...) body ...)`: none for a
    /// definition that assigns (its parameters would need to be variables),
    /// refers to itself (inlining would not end) or is large.
    fn of(form: &Sexp) -> Option<Inline> {
        let (name, Definiens::Procedure(params, body)) = define_parts(form.list()?).ok()? else { return None };
        let (params, rest) = match &params {
            Sexp::Sym(r) => (vec![], Some(*r)),
            Sexp::List(items, tail, _) => {
                let rest = match tail.as_deref() {
                    Some(t) => Some(t.sym()?),
                    None => None,
                };
                (items.iter().map(Sexp::sym).collect::<Option<_>>()?, rest)
            }
            _ => return None,
        };
        let body = match body {
            [Sexp::Str(_), rest @ ..] if !rest.is_empty() => rest,
            body => body,
        };
        fn size(s: &Sexp, name: u32) -> Option<usize> {
            match s {
                Sexp::Sym(x) if *x == name || &*symbol_name(*x) == "set!" => None,
                Sexp::List(items, tail, _) => items.iter().chain(tail.as_deref()).try_fold(1, |n, i| Some(n + size(i, name)?)),
                Sexp::Vector(items) => items.iter().try_fold(1, |n, i| Some(n + size(i, name)?)),
                _ => Some(1),
            }
        }
        let n = body.iter().try_fold(0, |n, f| Some(n + size(f, name)?))?;
        if n > MAX_INLINE_SIZE {
            return None;
        }
        let mut renames = FxHashMap::default();
        let mut alias = |sym: u32| *renames.entry(sym).or_insert_with(|| make_alias(sym, 0, ROOT_MODULE));
        let (params, rest) = (params.into_iter().map(&mut alias).collect(), rest.map(&mut alias));
        Some(Inline { params, rest, body: body.iter().map(|f| rename(f, &mut alias)).collect() })
    }
}

/// `s` with each symbol replaced by its `alias` (see `Inline`). Its
/// positions are in another file, so they are dropped.
fn rename(s: &Sexp, alias: &mut impl FnMut(u32) -> u32) -> Sexp {
    match s {
        Sexp::Sym(x) => Sexp::Sym(alias(*x)),
        Sexp::List(items, tail, _) => {
            Sexp::List(items.iter().map(|i| rename(i, alias)).collect(), tail.as_ref().map(|t| Box::new(rename(t, alias))), NO_POS)
        }
        Sexp::Vector(items) => Sexp::Vector(items.iter().map(|i| rename(i, alias)).collect()),
        Sexp::Labeled(n, d) => Sexp::Labeled(*n, Box::new(rename(d, alias))),
        other => other.clone(),
    }
}

/// `e` with a test of a known value computed (from inlining: `(null? '())`).
fn fold(e: Expr) -> Expr {
    let known = match &e {
        Expr::Prim(Prim::NullP, args, _) => match &args[0] {
            Expr::Const(Sexp::List(items, None, _)) => Some(items.is_empty()),
            Expr::Const(_) => Some(false),
            _ => None,
        },
        Expr::Prim(Prim::Not, args, _) => match &args[0] {
            Expr::Const(c) => Some(matches!(c, Sexp::Bool(false))),
            _ => None,
        },
        _ => None,
    };
    match known {
        Some(b) => Expr::Const(Sexp::Bool(b)),
        None => e,
    }
}

fn build_prim(p: PrimForm, mut args: Vec<Expr>, pos: Pos) -> Expr {
    match p {
        PrimForm::Direct(p) => Expr::Prim(p, args, pos),
        PrimForm::Fold(p) => {
            let rest = args.split_off(1);
            let first = args.pop().unwrap();
            rest.into_iter().fold(first, |acc, a| Expr::Prim(p, vec![acc, a], pos))
        }
        PrimForm::Negate => Expr::Prim(Prim::Sub, vec![Expr::Const(Sexp::Int(0)), args.pop().unwrap()], pos),
        PrimForm::Zero => Expr::Prim(Prim::NumEq, vec![args.pop().unwrap(), Expr::Const(Sexp::Int(0))], pos),
    }
}

/// Optional (`[x default]`) and keyword (`#:k x`, `#:k [x default]`) formals
/// become a rest parameter parsed by `%parse-args`, then `let*` bindings so
/// defaults can refer to earlier parameters. `None` if the formals are plain.
fn optional_formals(name: &str, params: &Sexp, body: &[Sexp]) -> R<Option<(Sexp, Vec<Sexp>)>> {
    let Sexp::List(items, tail, _) = params else { return Ok(None) };
    if !items.iter().any(|i| matches!(i, Sexp::Keyword(_) | Sexp::List(..))) {
        return Ok(None);
    }
    let bad = |what: &str| Error::new(format!("{name}: {what}"));
    let mut required = Vec::new();
    let mut optional: Vec<(Sexp, Sexp)> = Vec::new();
    let mut keywords: Vec<(u32, Sexp, Option<Sexp>)> = Vec::new();
    let mut i = 0;
    while i < items.len() {
        match &items[i] {
            Sexp::Keyword(k) => {
                let spec = items.get(i + 1).ok_or_else(|| bad("keyword without a parameter"))?;
                let (var, default) = match spec {
                    Sexp::Sym(_) => (spec.clone(), None),
                    Sexp::List(d, None, _) if d.len() == 2 && d[0].sym().is_some() => (d[0].clone(), Some(d[1].clone())),
                    _ => return Err(bad("keyword parameter must be x or [x default]")),
                };
                keywords.push((*k, var, default));
                i += 2;
                continue;
            }
            Sexp::List(d, None, _) if d.len() == 2 && d[0].sym().is_some() => optional.push((d[0].clone(), d[1].clone())),
            Sexp::Sym(_) if optional.is_empty() && keywords.is_empty() => required.push(items[i].clone()),
            Sexp::Sym(_) => return Err(bad("required parameter after optional or keyword parameters")),
            _ => return Err(bad("malformed parameter")),
        }
        i += 1;
    }
    let rest_var = tail.as_deref().cloned();
    let rest = Sexp::Sym(make_alias(intern("rest"), 0, ROOT_MODULE));
    let args = Sexp::Sym(make_alias(intern("args"), 0, ROOT_MODULE));
    let x = Sexp::Sym(make_alias(intern("x"), 0, ROOT_MODULE));
    let quote = |s: Sexp| list(vec![core("quote"), s]);
    let call = |f: &str, a: Vec<Sexp>| list([vec![core(f)], a].concat());
    let parse = call(
        "%parse-args",
        vec![
            rest.clone(),
            Sexp::Int(optional.len() as i64),
            quote(list(keywords.iter().map(|(k, _, _)| Sexp::Keyword(*k)).collect())),
            quote(Sexp::Sym(intern(name))),
            Sexp::Bool(rest_var.is_some()),
        ],
    );
    let slot = |i: usize, default: Sexp| {
        list(vec![
            core("let"),
            list(vec![list(vec![x.clone(), call("vector-ref", vec![args.clone(), Sexp::Int(i as i64)])])]),
            list(vec![core("if"), call("%unset?", vec![x.clone()]), default, x.clone()]),
        ])
    };
    let mut binds = vec![list(vec![args.clone(), parse])];
    for (i, (var, default)) in optional.iter().enumerate() {
        binds.push(list(vec![var.clone(), slot(i, default.clone())]));
    }
    for (j, (k, var, default)) in keywords.iter().enumerate() {
        let default = default.clone().unwrap_or_else(|| call("%missing-keyword", vec![quote(Sexp::Sym(intern(name))), Sexp::Keyword(*k)]));
        binds.push(list(vec![var.clone(), slot(optional.len() + j, default)]));
    }
    if let Some(r) = rest_var {
        binds.push(list(vec![r, call("vector-ref", vec![args.clone(), Sexp::Int((optional.len() + keywords.len()) as i64)])]));
    }
    let mut new_body = vec![core("let*"), list(binds)];
    new_body.extend(body.iter().cloned());
    Ok(Some((Sexp::List(required, Some(Box::new(rest)), NO_POS), vec![list(new_body)])))
}

/// Parameter list as written, for `help`.
fn formals_display(params: &Sexp) -> Vec<Rc<str>> {
    match params {
        Sexp::Sym(r) => vec![format!(". {}", display_name(*r)).into()],
        Sexp::List(items, tail, _) => items
            .iter()
            .map(|p| reader::display_sexp(p).into())
            .chain(tail.iter().map(|t| format!(". {}", reader::display_sexp(t)).into()))
            .collect(),
        _ => vec![],
    }
}

/// `(define name value)` or `(define (name . params) body...)`, also curried
/// `(define ((name a) b) ...)`.
/// What a definition binds: an expression, or a procedure's parameters and
/// body, which are not copied into a `lambda` form.
enum Definiens<'a> {
    Expr(Cow<'a, Sexp>),
    Procedure(Sexp, &'a [Sexp]),
}

impl Definiens<'_> {
    fn into_owned(self) -> Definiens<'static> {
        match self {
            Definiens::Expr(e) => Definiens::Expr(Cow::Owned(e.into_owned())),
            Definiens::Procedure(params, body) => {
                let lambda = [core("lambda"), params].into_iter().chain(body.iter().cloned()).collect();
                Definiens::Expr(Cow::Owned(list(lambda)))
            }
        }
    }
}

/// The name and definiens of `(define name value [doc])` or `(define (name
/// . params) body ...)`, also curried: `(define ((name a) b) ...)`.
fn define_parts(items: &[Sexp]) -> R<(u32, Definiens<'_>)> {
    crate::nested(|| define_parts_step(items))
}

fn define_parts_step(items: &[Sexp]) -> R<(u32, Definiens<'_>)> {
    match items.get(1) {
        Some(Sexp::Sym(_)) if items.len() > 4 || (items.len() == 4 && !matches!(items[3], Sexp::Str(_))) => {
            err("define: expected (define name value [doc])")
        }
        Some(Sexp::Sym(name)) => {
            Ok((*name, Definiens::Expr(items.get(2).map_or_else(|| Cow::Owned(list(vec![core("void")])), Cow::Borrowed))))
        }
        Some(Sexp::List(sig, rest, pos)) if !sig.is_empty() => {
            let params = Sexp::List(sig[1..].to_vec(), rest.clone(), *pos);
            match &sig[0] {
                Sexp::Sym(name) => Ok((*name, Definiens::Procedure(params, &items[2..]))),
                inner @ Sexp::List(..) => {
                    let lambda = [core("lambda"), params].into_iter().chain(items[2..].iter().cloned()).collect();
                    let outer = [items[0].clone(), inner.clone(), Sexp::List(lambda, None, *pos)];
                    let (name, value) = define_parts(&outer)?;
                    Ok((name, value.into_owned()))
                }
                _ => err("define: bad name"),
            }
        }
        _ => err("define: malformed definition"),
    }
}

/// `(define-record-type name (ctor field ...) pred (field accessor [modifier]) ...)`
fn define_record_type(items: &[Sexp]) -> R<Sexp> {
    let bad = || Error::new("define-record-type: expected (define-record-type name (ctor field ...) pred (field accessor [modifier]) ...)");
    let (type_name, ctor, pred) = match items {
        [_, Sexp::Sym(t), c, Sexp::Sym(p), ..] => (*t, c, *p),
        _ => return Err(bad()),
    };
    let specs = items[4..].iter().map(|f| f.list().filter(|l| !l.is_empty()).ok_or_else(bad)).collect::<R<Vec<_>>>()?;
    let fields: Vec<u32> = specs.iter().map(|s| s[0].sym().ok_or_else(bad)).collect::<R<_>>()?;
    let rtd = Sexp::Sym(make_alias(type_name, 0, ROOT_MODULE));
    let quote = |s: Sexp| list(vec![core("quote"), s]);
    let field_syms = list(fields.iter().map(|f| Sexp::Sym(strip(*f))).collect());
    let shown_name = symbol_name(strip(type_name));
    let shown_name = shown_name.trim_start_matches('<').trim_end_matches('>');
    let mut out = vec![
        core("begin"),
        list(vec![core("define"), rtd.clone(), list(vec![core("%make-rtd"), quote(Sexp::Sym(intern(shown_name))), quote(field_syms)])]),
        list(vec![core("define"), Sexp::Sym(type_name), rtd.clone()]),
    ];
    let (ctor_name, ctor_fields) = match ctor {
        Sexp::Sym(c) => (Some(*c), fields.clone()),
        Sexp::List(c, None, _) if !c.is_empty() => {
            (Some(c[0].sym().ok_or_else(bad)?), c[1..].iter().map(|f| f.sym().ok_or_else(bad)).collect::<R<_>>()?)
        }
        Sexp::Bool(false) => (None, vec![]),
        _ => return Err(bad()),
    };
    // Each procedure is documented as what it does, and defined where its
    // part of the form is (the predicate where the constructor is).
    let doc = |text: String| Sexp::Str(text.into());
    let at = |s: &Sexp| s.pos();
    let header = |pos, items: Vec<Sexp>| Sexp::List(items, None, pos);
    let ctor_pos = if ctor.pos() != reader::NO_POS { ctor.pos() } else { specs.first().map_or(reader::NO_POS, |s| s[0].pos()) };
    let shown = format!("`{shown_name}`");
    if let Some(ctor_name) = ctor_name {
        let params: Vec<Sexp> = ctor_fields.iter().map(|f| Sexp::Sym(make_alias(*f, 0, ROOT_MODULE))).collect();
        let mut make = vec![core("%record"), rtd.clone()];
        for f in &fields {
            make.push(match ctor_fields.iter().position(|c| c == f) {
                Some(i) => params[i].clone(),
                None => Sexp::Bool(false),
            });
        }
        let names: Vec<String> = ctor_fields.iter().map(|f| symbol_name(strip(*f)).to_uppercase()).collect();
        let text = match names.as_slice() {
            [] => format!("Make a {shown} record."),
            [one] => format!("Make a {shown} record of {one}."),
            [init @ .., last] => format!("Make a {shown} record.\n{}", wrap(&format!("Its fields are {} and {last}.", init.join(", ")))),
        };
        out.push(list(vec![core("define"), header(ctor_pos, [vec![Sexp::Sym(ctor_name)], params].concat()), doc(text), list(make)]));
    }
    let v = Sexp::Sym(make_alias(intern("record"), 0, ROOT_MODULE));
    let obj = Sexp::Sym(make_alias(intern("obj"), 0, ROOT_MODULE));
    let x = Sexp::Sym(make_alias(intern("value"), 0, ROOT_MODULE));
    out.push(list(vec![
        core("define"),
        header(ctor_pos, vec![Sexp::Sym(pred), obj.clone()]),
        doc(format!("Return #t if OBJ is a {shown} record.")),
        list(vec![core("%record?"), obj, rtd.clone()]),
    ]));
    for (i, spec) in specs.iter().enumerate() {
        let idx = Sexp::Int(i as i64);
        let field_name = symbol_name(strip(fields[i]));
        if let Some(acc) = spec.get(1) {
            out.push(list(vec![
                core("define"),
                header(at(&items[4 + i]), vec![acc.clone(), v.clone()]),
                doc(format!("Return the `{field_name}` field of RECORD, a {shown} record.")),
                list(vec![core("%record-ref"), v.clone(), rtd.clone(), idx.clone()]),
            ]));
        }
        if let Some(modifier) = spec.get(2) {
            out.push(list(vec![
                core("define"),
                header(at(&items[4 + i]), vec![modifier.clone(), v.clone(), x.clone()]),
                doc(format!("Set the `{field_name}` field of RECORD, a {shown} record, to VALUE.")),
                list(vec![core("%record-set!"), v.clone(), rtd.clone(), idx, x.clone()]),
            ]));
        }
    }
    Ok(list(out))
}

/// `text` broken into lines of at most `doc::FIRST_LINE_MAX` characters at
/// spaces.
fn wrap(text: &str) -> String {
    let mut lines: Vec<String> = vec![String::new()];
    for word in text.split(' ') {
        let line = lines.last_mut().unwrap();
        if !line.is_empty() && line.len() + 1 + word.len() > crate::doc::FIRST_LINE_MAX {
            lines.push(word.to_string());
        } else {
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
    }
    lines.join("\n")
}

/// A literal datum; its label references must be to labels within it.
fn literal(s: &Sexp) -> R<Expr> {
    match reader::dangling_label(s) {
        Some(n) => err(format!("#{n}# refers to a datum label outside this literal")),
        None => Ok(Expr::Const(strip_sexp(s))),
    }
}

/// Quasiquote expansion into list construction with root-module procedures.
fn quasi(s: &Sexp, depth: usize) -> R<Sexp> {
    crate::nested(|| quasi_step(s, depth))
}

fn quasi_step(s: &Sexp, depth: usize) -> R<Sexp> {
    let quote = |s: &Sexp| list(vec![core("quote"), s.clone()]);
    match s {
        Sexp::List(items, None, _) if items.len() == 2 && items[0].is_sym("unquote") => {
            if depth == 1 {
                Ok(items[1].clone())
            } else {
                Ok(list(vec![core("list"), quote(&items[0]), quasi(&items[1], depth - 1)?]))
            }
        }
        Sexp::List(items, None, _) if items.len() == 2 && items[0].is_sym("quasiquote") => {
            Ok(list(vec![core("list"), quote(&items[0]), quasi(&items[1], depth + 1)?]))
        }
        Sexp::List(items, tail, _) => {
            let mut acc = match tail {
                Some(t) => quasi(t, depth)?,
                None => quote(&Sexp::list_of(vec![])),
            };
            for item in items.iter().rev() {
                acc = match item.list() {
                    Some([h, e]) if h.is_sym("unquote-splicing") && depth == 1 => list(vec![core("append"), e.clone(), acc]),
                    _ => list(vec![core("cons"), quasi(item, depth)?, acc]),
                };
            }
            Ok(acc)
        }
        Sexp::Vector(items) => Ok(list(vec![core("list->vector"), quasi(&Sexp::list_of(items.clone()), depth)?])),
        other => Ok(quote(other)),
    }
}

#[derive(Clone, Copy)]
enum Dest {
    Reg(Reg),
    Return,
    Effect,
}

#[derive(Clone, Copy)]
enum Loc {
    Reg(Reg),
    Cap(u16),
}

struct Gen {
    ops: Vec<Op>,
    spans: Vec<u32>,
    pos: Pos,
    consts: Vec<crate::value::Value>,
    /// The next free register and the most in use, which may exceed what a
    /// `Reg` holds: `generate` then rejects the function.
    next: u32,
    max: u32,
    locs: FxHashMap<VarId, Loc>,
    loops: FxHashMap<LoopId, (u32, Vec<Reg>)>,
}

impl Default for Gen {
    fn default() -> Self {
        Gen {
            ops: Vec::new(),
            spans: Vec::new(),
            pos: NO_POS,
            consts: Vec::new(),
            next: 0,
            max: 0,
            locs: FxHashMap::default(),
            loops: FxHashMap::default(),
        }
    }
}

impl Gen {
    fn emit(&mut self, op: Op) {
        self.ops.push(op);
        self.spans.push(self.pos);
    }
    /// Set the current source position (if known); returns the previous one.
    fn at(&mut self, pos: Pos) -> Pos {
        let saved = self.pos;
        if pos != NO_POS {
            self.pos = pos;
        }
        saved
    }
    /// A fresh register; past the last one, a placeholder (see `next`).
    fn alloc(&mut self) -> Reg {
        let r = self.next;
        self.next += 1;
        self.max = self.max.max(self.next);
        Reg::try_from(r).unwrap_or(Reg::MAX)
    }
    fn target(&mut self, dest: Dest) -> Reg {
        match dest {
            Dest::Reg(r) => r,
            _ => self.alloc(),
        }
    }
    fn finish(&mut self, r: Reg, dest: Dest) {
        match dest {
            Dest::Reg(d) if d != r => self.emit(Op::Mov { dst: d, src: r }),
            Dest::Return => self.emit(Op::Ret { r }),
            _ => {}
        }
    }
    fn emit_jump(&mut self) -> usize {
        self.emit(Op::Jmp { t: 0 });
        self.ops.len() - 1
    }
    fn patch_all(&mut self, jumps: &[usize]) {
        let here = self.ops.len() as u32;
        for &j in jumps {
            match &mut self.ops[j] {
                Op::Jmp { t }
                | Op::Jf { t, .. }
                | Op::Jt { t, .. }
                | Op::JNLt { t, .. }
                | Op::JNLe { t, .. }
                | Op::JNNumEq { t, .. }
                | Op::JNEq { t, .. }
                | Op::JNNull { t, .. }
                | Op::JNLtI { t, .. }
                | Op::JNGtI { t, .. }
                | Op::JNEqI { t, .. }
                | Op::JNPair { t, .. } => *t = here,
                other => panic!("patching non-jump {other:?}"),
            }
        }
    }
}

/// A named let is a loop when its name only appears as the operator of calls
/// in tail position of its body (syntactically, under this file's desugaring).
fn loopable(name: u32, body: &[Sexp]) -> bool {
    body_tail_only(body, name, true)
}

fn body_tail_only(forms: &[Sexp], name: u32, tail: bool) -> bool {
    let n = forms.len();
    forms.iter().enumerate().all(|(i, f)| tail_only(f, name, tail && i + 1 == n))
}

fn tail_only(s: &Sexp, name: u32, tail: bool) -> bool {
    match s {
        Sexp::List(..) => crate::nested(|| tail_only_step(s, name, tail)),
        _ => tail_only_step(s, name, tail),
    }
}

fn tail_only_step(s: &Sexp, name: u32, tail: bool) -> bool {
    let none = |xs: &[Sexp]| xs.iter().all(|x| tail_only(x, name, false));
    match s {
        Sexp::Sym(x) => *x != name,
        Sexp::Vector(_) => true,
        Sexp::List(items, Some(rest), _) => none(items) && tail_only(rest, name, false),
        Sexp::List(items, None, _) => {
            let Some((head, args)) = items.split_first() else { return true };
            if head.sym() == Some(name) {
                return tail && none(args);
            }
            let Some(h) = head.sym() else { return none(items) };
            match &*symbol_name(strip(h)) {
                "quote" => true,
                "if" => none(&args[..1.min(args.len())]) && args.iter().skip(1).all(|a| tail_only(a, name, tail)),
                "begin" => body_tail_only(args, name, tail),
                "when" | "unless" => none(&args[..1.min(args.len())]) && body_tail_only(&args[1.min(args.len())..], name, tail),
                "and" | "or" => body_tail_only(args, name, tail),
                "cond" => args.iter().all(|clause| match clause.list() {
                    Some([test]) => tail_only(test, name, false),
                    Some([test, arrow, f]) if arrow.is_sym("=>") => tail_only(test, name, false) && tail_only(f, name, false),
                    Some([test, body @ ..]) => (test.is_sym("else") || tail_only(test, name, false)) && body_tail_only(body, name, tail),
                    _ => false,
                }),
                "case" => {
                    none(&args[..1.min(args.len())])
                        && args.iter().skip(1).all(|clause| match clause.list() {
                            Some([_, body @ ..]) => body_tail_only(body, name, tail),
                            _ => false,
                        })
                }
                "let" | "let*" | "letrec" | "letrec*" => match args {
                    [Sexp::Sym(inner), bindings, body @ ..] => {
                        *inner != name && bindings_ok(bindings, name) && body_tail_only(body, name, tail && loopable(*inner, body))
                    }
                    [bindings, body @ ..] => bindings_ok(bindings, name) && body_tail_only(body, name, tail),
                    _ => false,
                },
                _ => none(items),
            }
        }
        _ => true,
    }
}

fn bindings_ok(bindings: &Sexp, name: u32) -> bool {
    bindings.list().is_some_and(|bs| {
        bs.iter().all(|b| match b.list() {
            Some([_, init]) => tail_only(init, name, false),
            _ => false,
        })
    })
}
