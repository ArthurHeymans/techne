//! Language server for techne Lisp (stdio).
//!
//! For editors other than Techne's own, which asks the running VM instead
//! (as nREPL clients do). Diagnostics (reader errors, unbound identifiers,
//! missing requires, libraries and included files, docstrings as checkdoc
//! checks them), go-to-definition, hover
//! (signature, docstring, built-in descriptions), completion and document
//! symbols. Files are read by the VM's reader and analysed syntactically
//! (see `analysis`); no user code is run. What the runtime defines comes
//! from a VM with the runtime's libraries installed.

mod analysis;

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Mutex,
};

use analysis::{Analysis, Def, DefKind, Span, analyze, checkdoc, head, ident, library_name, list, unbound};
use techne_vm::reader::Syntax;
use tower_lsp::{Client, LanguageServer, LspService, Server, jsonrpc::Result, lsp_types::*};

/// Byte offset to an LSP position (UTF-16 columns).
fn position(text: &str, offset: u32) -> Position {
    let offset = (offset as usize).min(text.len());
    let before = &text[..offset];
    let line = before.matches('\n').count() as u32;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let character = text[line_start..offset].encode_utf16().count() as u32;
    Position { line, character }
}

fn offset(text: &str, pos: Position) -> u32 {
    let mut line_start = 0;
    for _ in 0..pos.line {
        match text[line_start..].find('\n') {
            Some(i) => line_start += i + 1,
            None => return text.len() as u32,
        }
    }
    let line = &text[line_start..text[line_start..].find('\n').map_or(text.len(), |i| line_start + i)];
    let mut units = 0;
    for (i, c) in line.char_indices() {
        if units >= pos.character {
            return (line_start + i) as u32;
        }
        units += c.len_utf16() as u32;
    }
    (line_start + line.len()) as u32
}

fn range(text: &str, span: Span) -> Range {
    Range { start: position(text, span.0), end: position(text, span.1) }
}

/// A definition from another (required) file.
struct External {
    uri: Url,
    text: String,
    def: Def,
}

struct Backend {
    client: Client,
    docs: Mutex<HashMap<Url, String>>,
    builtins: HashMap<String, String>,
    /// The runtime's macros (from `builtins`).
    builtin_macros: HashSet<String>,
    /// Every name the runtime defines, internal ones included: not unbound.
    runtime_names: HashSet<String>,
}

impl Backend {
    fn text(&self, uri: &Url) -> Option<String> {
        self.docs.lock().unwrap().get(uri).cloned()
    }

    /// The analysis of a file, with the definitions of the files it
    /// requires (their macros taken into account) and the requires that
    /// were not found.
    fn analysis(&self, uri: &Url, text: &str) -> (Analysis, Vec<External>, Vec<(String, Span)>) {
        let a = analyze(text, &self.builtin_macros);
        let (externals, missing) = self.externals(uri, &a);
        let external_macros: Vec<String> = externals.iter().filter(|e| e.def.kind == DefKind::Macro).map(|e| e.def.name.clone()).collect();
        if external_macros.is_empty() {
            return (a, externals, missing);
        }
        let macros: HashSet<String> = self.builtin_macros.iter().cloned().chain(external_macros).collect();
        (analyze(text, &macros), externals, missing)
    }

    /// Exported top-level definitions of the files `a` requires.
    fn externals(&self, uri: &Url, a: &Analysis) -> (Vec<External>, Vec<(String, Span)>) {
        let base = uri.to_file_path().ok().and_then(|p| p.parent().map(PathBuf::from)).unwrap_or_default();
        let mut out = Vec::new();
        let mut missing = Vec::new();
        for (spec, span) in &a.requires {
            let path = base.join(spec);
            let Ok(text) = std::fs::read_to_string(&path) else {
                missing.push((format!("cannot find module {spec}"), *span));
                continue;
            };
            let Ok(file_uri) = Url::from_file_path(path.canonicalize().unwrap_or(path)) else { continue };
            let other = analyze(&text, &self.builtin_macros);
            for def in other.top_level() {
                if other.provides.is_empty() || other.provides.contains(&def.name) {
                    out.push(External { uri: file_uri.clone(), text: text.clone(), def: def.clone() });
                }
            }
        }
        for (name, span) in &a.includes {
            match included(&base, name, &self.builtin_macros) {
                Some(defs) => out.extend(defs),
                None => missing.push((format!("cannot find included file {name}"), *span)),
            }
        }
        let here = Here { uri, text: &self.text(uri).unwrap_or_default(), a };
        for set in &a.imports {
            match self.import_set(&base, &here, set) {
                Ok(defs) => out.extend(defs),
                Err(name) => missing.push((name, set.span)),
            }
        }
        (out, missing)
    }

    /// The definitions an R7RS import set brings in, as the VM finds them:
    /// a library defined in this file, or `a/b.sld` beside it or on
    /// `TECHNE_LIBRARY_PATH`. The runtime's libraries (`(scheme ...)`,
    /// `(techne)`) bring builtins, known anyway. Err: a library not found.
    fn import_set(&self, base: &std::path::Path, here: &Here, set: &Syntax) -> std::result::Result<Vec<External>, String> {
        let items = list(set).unwrap_or(&[]);
        let names = |from: usize| -> Vec<String> { items[from.min(items.len())..].iter().filter_map(ident).map(|(n, _)| n).collect() };
        let inner = || items.get(1).map_or(Ok(vec![]), |s| self.import_set(base, here, s));
        let renamed = |defs: Vec<External>, f: &dyn Fn(&str) -> Option<String>| -> Vec<External> {
            defs.into_iter()
                .filter_map(|mut e| {
                    e.def.name = f(&e.def.name)?;
                    Some(e)
                })
                .collect()
        };
        Ok(match head(items).as_deref() {
            Some("only") => {
                let keep = names(2);
                renamed(inner()?, &|n| keep.iter().any(|k| k == n).then(|| n.to_string()))
            }
            Some("except") => {
                let drop = names(2);
                renamed(inner()?, &|n| (!drop.iter().any(|k| k == n)).then(|| n.to_string()))
            }
            Some("prefix") => {
                let prefix = names(2).pop().unwrap_or_default();
                renamed(inner()?, &|n| Some(format!("{prefix}{n}")))
            }
            Some("rename") => {
                let pairs: Vec<(String, String)> = items[2.min(items.len())..]
                    .iter()
                    .filter_map(|p| match list(p).map(|l| l.iter().filter_map(ident).map(|(n, _)| n).collect::<Vec<_>>()) {
                        Some(v) if v.len() == 2 => Some((v[0].clone(), v[1].clone())),
                        _ => None,
                    })
                    .collect();
                renamed(inner()?, &|n| Some(pairs.iter().find(|(f, _)| f == n).map_or(n.to_string(), |(_, t)| t.clone())))
            }
            _ => {
                let parts = library_name(set);
                let written = format!("({})", parts.join(" "));
                if matches!(parts.first().map(String::as_str), Some("scheme" | "techne")) {
                    return Ok(vec![]);
                }
                // A library this file defines: its definitions are here.
                if let Some((_, exports)) = here.a.libraries.iter().find(|(n, _)| *n == parts) {
                    let defs: Vec<External> = here
                        .a
                        .top_level()
                        .map(|d| External { uri: here.uri.clone(), text: here.text.to_string(), def: d.clone() })
                        .collect();
                    return Ok(exported(exports, defs));
                }
                let rel = format!("{}.sld", parts.join("/"));
                let path_dirs = std::env::var("TECHNE_LIBRARY_PATH").unwrap_or_default();
                let file = std::iter::once(base.to_path_buf())
                    .chain(std::env::split_paths(&path_dirs))
                    .map(|d| d.join(&rel))
                    .find(|p| p.is_file())
                    .ok_or_else(|| format!("cannot find library {written}"))?;
                let text = std::fs::read_to_string(&file).map_err(|_| format!("cannot read library {written}"))?;
                let uri = Url::from_file_path(file.canonicalize().unwrap_or(file)).map_err(|_| written.clone())?;
                let other = analyze(&text, &self.builtin_macros);
                let exports = other.libraries.iter().find(|(n, _)| *n == parts).map(|(_, e)| e.clone()).unwrap_or_default();
                let dir = uri.to_file_path().ok().and_then(|p| p.parent().map(PathBuf::from)).unwrap_or_default();
                let mut defs: Vec<External> =
                    other.top_level().map(|d| External { uri: uri.clone(), text: text.clone(), def: d.clone() }).collect();
                for (name, _) in &other.includes {
                    defs.extend(included(&dir, name, &self.builtin_macros).unwrap_or_default());
                }
                exported(&exports, defs)
            }
        })
    }

    async fn publish(&self, uri: Url, text: String) {
        let diagnostics = self.diagnostics(&uri, &text);
        self.client.publish_diagnostics(uri, diagnostics, None).await;
    }

    fn diagnostics(&self, uri: &Url, text: &str) -> Vec<Diagnostic> {
        let text = text.to_string();
        let (a, externals, missing) = self.analysis(uri, &text);
        let mut diagnostics = Vec::new();
        if let Some((msg, pos)) = &a.error {
            diagnostics.push(Diagnostic {
                range: Range { start: position(&text, *pos), end: position(&text, *pos + 1) },
                severity: Some(DiagnosticSeverity::ERROR),
                message: msg.clone(),
                source: Some("techne".into()),
                ..Diagnostic::default()
            });
        } else {
            for (spec, span) in missing {
                diagnostics.push(Diagnostic {
                    range: range(&text, span),
                    severity: Some(DiagnosticSeverity::ERROR),
                    message: spec,
                    source: Some("techne".into()),
                    ..Diagnostic::default()
                });
            }
            let known: HashSet<String> = self
                .runtime_names
                .iter()
                .chain(self.builtins.keys())
                .cloned()
                .chain(externals.iter().map(|e| e.def.name.clone()))
                .collect();
            for (message, span) in checkdoc(&text, &a) {
                diagnostics.push(Diagnostic {
                    range: range(&text, span),
                    severity: Some(DiagnosticSeverity::WARNING),
                    message,
                    source: Some("checkdoc".into()),
                    ..Diagnostic::default()
                });
            }
            for r in unbound(&a, &known) {
                diagnostics.push(Diagnostic {
                    range: range(&text, r.span),
                    severity: Some(DiagnosticSeverity::WARNING),
                    message: format!("unbound identifier: {}", r.name),
                    source: Some("techne".into()),
                    ..Diagnostic::default()
                });
            }
        }
        diagnostics
    }
}

/// The file being analysed, for libraries it defines itself.
struct Here<'a> {
    uri: &'a Url,
    text: &'a str,
    a: &'a Analysis,
}

/// The top-level definitions of an included file (relative to `dir`).
fn included(dir: &std::path::Path, name: &str, macros: &HashSet<String>) -> Option<Vec<External>> {
    let path = dir.join(name);
    let text = std::fs::read_to_string(&path).ok()?;
    let uri = Url::from_file_path(path.canonicalize().unwrap_or(path)).ok()?;
    let a = analyze(&text, macros);
    Some(a.top_level().map(|d| External { uri: uri.clone(), text: text.clone(), def: d.clone() }).collect())
}

/// The definitions a library's exports `(inside, outside)` name, by the
/// names importers see.
fn exported(exports: &[(String, String)], defs: Vec<External>) -> Vec<External> {
    exports
        .iter()
        .filter_map(|(inside, outside)| {
            let e = defs.iter().find(|e| e.def.name == *inside)?;
            Some(External { uri: e.uri.clone(), text: e.text.clone(), def: Def { name: outside.clone(), ..e.def.clone() } })
        })
        .collect()
}

fn hover_text(def: &Def) -> String {
    let header = def.signature.clone().unwrap_or_else(|| def.name.clone());
    let kind = match def.kind {
        DefKind::Function => "procedure",
        DefKind::Variable => "variable",
        DefKind::Macro => "syntax",
        DefKind::Record => "record type",
        DefKind::Generic => "generic function",
        DefKind::Local => "local",
    };
    let mut s = format!("```scheme\n{header}\n```\n{kind}");
    if let Some(doc) = &def.doc {
        s.push_str("\n\n");
        s.push_str(doc);
    }
    s
}

fn completion_kind(kind: DefKind) -> CompletionItemKind {
    match kind {
        DefKind::Function | DefKind::Generic => CompletionItemKind::FUNCTION,
        DefKind::Macro => CompletionItemKind::KEYWORD,
        DefKind::Record => CompletionItemKind::STRUCT,
        DefKind::Variable | DefKind::Local => CompletionItemKind::VARIABLE,
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                completion_provider: Some(CompletionOptions::default()),
                document_symbol_provider: Some(OneOf::Left(true)),
                ..ServerCapabilities::default()
            },
            server_info: Some(ServerInfo { name: "techne-lsp".into(), version: None }),
        })
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, p: DidOpenTextDocumentParams) {
        let uri = p.text_document.uri;
        self.docs.lock().unwrap().insert(uri.clone(), p.text_document.text.clone());
        self.publish(uri, p.text_document.text).await;
    }

    async fn did_change(&self, p: DidChangeTextDocumentParams) {
        if let Some(change) = p.content_changes.into_iter().last() {
            let uri = p.text_document.uri;
            self.docs.lock().unwrap().insert(uri.clone(), change.text.clone());
            self.publish(uri, change.text).await;
        }
    }

    async fn did_close(&self, p: DidCloseTextDocumentParams) {
        self.docs.lock().unwrap().remove(&p.text_document.uri);
    }

    async fn hover(&self, p: HoverParams) -> Result<Option<Hover>> {
        let uri = p.text_document_position_params.text_document.uri;
        let Some(text) = self.text(&uri) else { return Ok(None) };
        let (a, externals, _) = self.analysis(&uri, &text);
        let pos = offset(&text, p.text_document_position_params.position);
        let Some((name, def)) = a.at(pos) else { return Ok(None) };
        let value = match def {
            Some(d) => hover_text(d),
            None => match externals.into_iter().find(|e| e.def.name == name) {
                Some(e) => hover_text(&e.def),
                None => match self.builtins.get(name) {
                    Some(desc) => format!("```text\n{desc}\n```"),
                    None => return Ok(None),
                },
            },
        };
        Ok(Some(Hover { contents: HoverContents::Markup(MarkupContent { kind: MarkupKind::Markdown, value }), range: None }))
    }

    async fn goto_definition(&self, p: GotoDefinitionParams) -> Result<Option<GotoDefinitionResponse>> {
        let uri = p.text_document_position_params.text_document.uri;
        let Some(text) = self.text(&uri) else { return Ok(None) };
        let (a, externals, _) = self.analysis(&uri, &text);
        let pos = offset(&text, p.text_document_position_params.position);
        let Some((name, def)) = a.at(pos) else { return Ok(None) };
        Ok(match def {
            Some(d) => Some(GotoDefinitionResponse::Scalar(Location { uri, range: range(&text, d.span) })),
            None => externals
                .into_iter()
                .find(|e| e.def.name == name)
                .map(|e| GotoDefinitionResponse::Scalar(Location { uri: e.uri, range: range(&e.text, e.def.span) })),
        })
    }

    async fn completion(&self, p: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = p.text_document_position.text_document.uri;
        let Some(text) = self.text(&uri) else { return Ok(None) };
        let (a, externals, _) = self.analysis(&uri, &text);
        let pos = offset(&text, p.text_document_position.position);
        let mut items: Vec<CompletionItem> = a
            .visible_at(pos)
            .map(|d| CompletionItem {
                label: d.name.clone(),
                kind: Some(completion_kind(d.kind)),
                detail: d.signature.clone(),
                ..CompletionItem::default()
            })
            .collect();
        items.extend(externals.into_iter().map(|e| CompletionItem {
            label: e.def.name.clone(),
            kind: Some(completion_kind(e.def.kind)),
            detail: e.def.signature.clone(),
            ..CompletionItem::default()
        }));
        items.extend(self.builtins.iter().map(|(name, desc)| CompletionItem {
            label: name.clone(),
            kind: Some(CompletionItemKind::FUNCTION),
            detail: desc.lines().next().map(str::to_string),
            ..CompletionItem::default()
        }));
        let mut seen = HashSet::new();
        items.retain(|i| seen.insert(i.label.clone()));
        Ok(Some(CompletionResponse::Array(items)))
    }

    async fn document_symbol(&self, p: DocumentSymbolParams) -> Result<Option<DocumentSymbolResponse>> {
        let uri = p.text_document.uri;
        let Some(text) = self.text(&uri) else { return Ok(None) };
        let a = analyze(&text, &self.builtin_macros);
        #[allow(deprecated)]
        let symbols = a
            .top_level()
            .map(|d| SymbolInformation {
                name: d.name.clone(),
                kind: match d.kind {
                    DefKind::Function | DefKind::Generic => SymbolKind::FUNCTION,
                    DefKind::Macro => SymbolKind::KEY,
                    DefKind::Record => SymbolKind::STRUCT,
                    _ => SymbolKind::VARIABLE,
                },
                tags: None,
                deprecated: None,
                location: Location { uri: uri.clone(), range: range(&text, d.span) },
                container_name: None,
            })
            .collect();
        Ok(Some(DocumentSymbolResponse::Flat(symbols)))
    }
}

/// A VM with what the Techne runtime defines: the language, processes and
/// nodes, the editor's procedures and its Lisp interface. No user code runs
/// in it.
fn runtime() -> techne_vm::vm::Vm {
    let mut vm = techne_vm::vm::Vm::new();
    techne_node::install(&mut vm).expect("the node library installs");
    techne_editor::install(&mut vm);
    // The editor's interface for extensions, (techne editor): its names
    // are known to the user module as to code the editor evaluates.
    let api = techne_editor::runtime::lisp_dir().join("api.scm");
    if let Err(e) = vm.eval_source(&format!("(require {:?})", api.display().to_string())) {
        eprintln!("techne-lsp: {}: {e}", api.display());
    }
    vm
}

/// Descriptions of everything the root and user modules define, from a VM.
fn builtin_descriptions(vm: &mut techne_vm::vm::Vm) -> HashMap<String, String> {
    let user = techne_vm::vm::USER_MODULE;
    vm.global_names(user)
        .into_iter()
        .map(|name| {
            let desc = vm.describe_binding(user, techne_vm::reader::intern(&name));
            (name.to_string(), desc)
        })
        .collect()
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut vm = runtime();
    let builtins = builtin_descriptions(&mut vm);
    let runtime_names: HashSet<String> = vm.root_names().iter().map(|n| n.to_string()).collect();
    drop(vm);
    let builtin_macros: HashSet<String> = builtins.iter().filter(|(_, d)| d.contains("syntax (macro)")).map(|(n, _)| n.clone()).collect();
    let (service, socket) = LspService::new(|client| Backend {
        client,
        docs: Mutex::new(HashMap::new()),
        builtins: builtins.clone(),
        builtin_macros: builtin_macros.clone(),
        runtime_names: runtime_names.clone(),
    });
    Server::new(tokio::io::stdin(), tokio::io::stdout(), socket).serve(service).await;
}
