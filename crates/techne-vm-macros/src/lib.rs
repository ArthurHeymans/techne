//! Natives defined with their signature and documentation, as Emacs's
//! `DEFUN` defines a primitive with its docstring:
//!
//! ```ignore
//! natives! { vm;
//!     /// Return the first element of PAIR.
//!     "(car pair)" => |vm: &mut Vm, a, _| ...;
//! }
//! ```
//!
//! The signature names the native and its parameters as Lisp writes them
//! (`x`, `[x]` optional, `. rest`), and gives its arity; the doc comment is
//! its docstring; the definition's file and line are where help and
//! find-definition send the user. `natives!` takes raw natives
//! (`NativeFn`), `procedures!` typed closures as `Vm::register_fn` does
//! (`Vm::register_fn_vm` for an entry marked `#[vm]`), and `document!`
//! documents natives defined otherwise (`Vm::register_async`), an entry
//! being only a signature: `"(name param ...)";`.

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use syn::{
    Attribute, Expr, LitStr, Token,
    parse::{Parse, ParseStream},
    parse_macro_input,
};

struct Entry {
    doc: String,
    with_vm: bool,
    signature: LitStr,
    /// The native, unless only documented.
    f: Option<Expr>,
}

struct Table {
    vm: Expr,
    entries: Vec<Entry>,
}

impl Parse for Table {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let vm: Expr = input.parse()?;
        input.parse::<Token![;]>()?;
        let mut entries = Vec::new();
        while !input.is_empty() {
            let attrs = input.call(Attribute::parse_outer)?;
            let mut doc = Vec::new();
            let mut with_vm = false;
            for attr in &attrs {
                if attr.path().is_ident("doc") {
                    if let syn::Meta::NameValue(nv) = &attr.meta
                        && let Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) = &nv.value
                    {
                        let line = s.value();
                        doc.push(line.strip_prefix(' ').unwrap_or(&line).to_string());
                    }
                } else if attr.path().is_ident("vm") {
                    with_vm = true;
                } else {
                    return Err(syn::Error::new_spanned(attr, "expected a doc comment or #[vm]"));
                }
            }
            let signature: LitStr = input.parse()?;
            let f = if input.parse::<Option<Token![=>]>>()?.is_some() { Some(input.parse::<Expr>()?) } else { None };
            input.parse::<Token![;]>()?;
            entries.push(Entry { doc: doc.join("\n").trim().to_string(), with_vm, signature, f });
        }
        Ok(Table { vm, entries })
    }
}

/// A signature `(name param ...)`: the name, the parameters as written, and
/// how many arguments it takes, at least and at most (`None`: any number).
struct Signature {
    name: String,
    params: Vec<String>,
    min: usize,
    max: Option<usize>,
}

fn parse_signature(lit: &LitStr) -> syn::Result<Signature> {
    let text = lit.value();
    let bad = |what: &str| syn::Error::new(lit.span(), format!("signature {text:?}: {what}"));
    let inner = text.strip_prefix('(').and_then(|t| t.strip_suffix(')')).ok_or_else(|| bad("expected (name param ...)"))?;
    // Words, keeping `[x default]` and `. rest` whole.
    let mut words: Vec<String> = Vec::new();
    let mut depth = 0;
    for c in inner.chars() {
        match c {
            ' ' if depth == 0 => {
                if words.last().is_some_and(|w| !w.is_empty()) {
                    words.push(String::new());
                }
                continue;
            }
            '[' => depth += 1,
            ']' => depth -= 1,
            _ => {}
        }
        match words.last_mut() {
            Some(w) => w.push(c),
            None => words.push(c.to_string()),
        }
    }
    words.retain(|w| !w.is_empty());
    let (name, rest) = words.split_first().ok_or_else(|| bad("no name"))?;
    let mut params = Vec::new();
    let (mut min, mut optional, mut variadic) = (0, 0, false);
    let mut it = rest.iter();
    while let Some(w) = it.next() {
        if variadic {
            return Err(bad("nothing may follow the rest parameter"));
        }
        if w == "." {
            let r = it.next().ok_or_else(|| bad("a rest parameter after ."))?;
            params.push(format!(". {r}"));
            variadic = true;
        } else if w.starts_with('[') {
            params.push(w.clone());
            optional += 1;
        } else if optional > 0 {
            return Err(bad("a required parameter after an optional one"));
        } else {
            params.push(w.clone());
            min += 1;
        }
    }
    Ok(Signature { name: name.clone(), params, min, max: (!variadic).then_some(min + optional) })
}

/// The absolute path of the file `span` is in, and its line.
fn location(span: Span) -> (String, u32) {
    let span = span.unwrap();
    let file = span
        .local_file()
        .map(|p| if p.is_absolute() { p } else { std::env::current_dir().map(|d| d.join(&p)).unwrap_or(p) })
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| span.file());
    (file, span.line() as u32)
}

#[derive(PartialEq)]
enum Mode {
    Raw,
    Typed,
    DocumentOnly,
}

fn expand(input: TokenStream, mode: Mode) -> TokenStream {
    let Table { vm, entries } = parse_macro_input!(input as Table);
    let mut out = proc_macro2::TokenStream::new();
    for Entry { doc, with_vm, signature, f } in entries {
        let sig = match parse_signature(&signature) {
            Ok(sig) => sig,
            Err(e) => return e.to_compile_error().into(),
        };
        // Internal natives (`%name`) may go undocumented.
        if doc.is_empty() && !sig.name.starts_with('%') {
            return syn::Error::new(signature.span(), format!("{}: document it with a doc comment", sig.name)).to_compile_error().into();
        }
        let (file, line) = location(signature.span());
        let Signature { name, params, min, max } = sig;
        let max = match max {
            Some(m) => quote!(Some(#m)),
            None => quote!(None),
        };
        let native_doc = quote! {
            static DOC: ::techne_vm::vm::NativeDoc =
                ::techne_vm::vm::NativeDoc { params: &[#(#params),*], doc: #doc, file: #file, line: #line };
        };
        let Some(f) = f.filter(|_| mode != Mode::DocumentOnly) else {
            if mode != Mode::DocumentOnly {
                return syn::Error::new(signature.span(), "expected => and the native").to_compile_error().into();
            }
            out.extend(quote! { { #native_doc (#vm).document_native(#name, #min, #max, &DOC); } });
            continue;
        };
        out.extend(if mode == Mode::Typed {
            let register = if with_vm { quote!(register_documented_vm) } else { quote!(register_documented) };
            quote! { { #native_doc (#vm).#register(#name, &DOC, #f); } }
        } else {
            quote! { {
                #native_doc
                let f: ::techne_vm::vm::NativeFn = #f;
                (#vm).define_native(::techne_vm::vm::Native {
                    name: #name.into(),
                    f: ::techne_vm::vm::NativeImpl::Plain(f),
                    min: #min,
                    max: #max,
                    doc: Some(&DOC),
                });
            } }
        });
    }
    out.into()
}

/// Define raw natives (`NativeFn`) in a VM, each with its signature and
/// documentation.
#[proc_macro]
pub fn natives(input: TokenStream) -> TokenStream {
    expand(input, Mode::Raw)
}

/// Register typed closures in a VM as `Vm::register_fn` does, each with
/// its signature and documentation; `#[vm]` before an entry registers it as
/// `Vm::register_fn_vm` does.
#[proc_macro]
pub fn procedures(input: TokenStream) -> TokenStream {
    expand(input, Mode::Typed)
}

/// Document natives already defined, each entry only a signature; the
/// arity the signature gives must be the native's.
#[proc_macro]
pub fn document(input: TokenStream) -> TokenStream {
    expand(input, Mode::DocumentOnly)
}
