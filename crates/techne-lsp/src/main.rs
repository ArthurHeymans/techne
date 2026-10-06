//! Language server for techne Lisp (stdio).
//!
//! Diagnostics (reader errors, unbound identifiers, missing requires),
//! go-to-definition, hover (signature, docstring, built-in descriptions),
//! completion and document symbols. Analysis is syntactic (see `analysis`);
//! no user code is run.

mod analysis;

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Mutex,
};

use analysis::{Analysis, Def, DefKind, Span, analyze, unbound};
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
}

impl Backend {
    fn text(&self, uri: &Url) -> Option<String> {
        self.docs.lock().unwrap().get(uri).cloned()
    }

    /// Exported top-level definitions of the files `a` requires.
    fn externals(&self, uri: &Url, a: &Analysis) -> (Vec<External>, Vec<(String, Span)>) {
        let base = uri.to_file_path().ok().and_then(|p| p.parent().map(PathBuf::from)).unwrap_or_default();
        let mut out = Vec::new();
        let mut missing = Vec::new();
        for (spec, span) in &a.requires {
            let path = base.join(spec);
            let Ok(text) = std::fs::read_to_string(&path) else {
                missing.push((spec.clone(), *span));
                continue;
            };
            let Ok(file_uri) = Url::from_file_path(path.canonicalize().unwrap_or(path)) else { continue };
            let other = analyze(&text);
            for def in other.top_level() {
                if other.provides.is_empty() || other.provides.contains(&def.name) {
                    out.push(External { uri: file_uri.clone(), text: text.clone(), def: def.clone() });
                }
            }
        }
        (out, missing)
    }

    async fn publish(&self, uri: Url, text: String) {
        let a = analyze(&text);
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
            let (externals, missing) = self.externals(&uri, &a);
            for (spec, span) in missing {
                diagnostics.push(Diagnostic {
                    range: range(&text, span),
                    severity: Some(DiagnosticSeverity::ERROR),
                    message: format!("cannot find module {spec}"),
                    source: Some("techne".into()),
                    ..Diagnostic::default()
                });
            }
            let known: HashSet<String> = self.builtins.keys().cloned().chain(externals.iter().map(|e| e.def.name.clone())).collect();
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
        self.client.publish_diagnostics(uri, diagnostics, None).await;
    }
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
        let a = analyze(&text);
        let pos = offset(&text, p.text_document_position_params.position);
        let Some((name, def)) = a.at(pos) else { return Ok(None) };
        let value = match def {
            Some(d) => hover_text(d),
            None => match self.externals(&uri, &a).0.into_iter().find(|e| e.def.name == name) {
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
        let a = analyze(&text);
        let pos = offset(&text, p.text_document_position_params.position);
        let Some((name, def)) = a.at(pos) else { return Ok(None) };
        Ok(match def {
            Some(d) => Some(GotoDefinitionResponse::Scalar(Location { uri, range: range(&text, d.span) })),
            None => self
                .externals(&uri, &a)
                .0
                .into_iter()
                .find(|e| e.def.name == name)
                .map(|e| GotoDefinitionResponse::Scalar(Location { uri: e.uri, range: range(&e.text, e.def.span) })),
        })
    }

    async fn completion(&self, p: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = p.text_document_position.text_document.uri;
        let Some(text) = self.text(&uri) else { return Ok(None) };
        let a = analyze(&text);
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
        items.extend(self.externals(&uri, &a).0.into_iter().map(|e| CompletionItem {
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
        let a = analyze(&text);
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

/// Descriptions of everything the root and user modules define, from a VM.
fn builtin_descriptions() -> HashMap<String, String> {
    let mut vm = techne_vm::vm::Vm::new();
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
    let builtins = builtin_descriptions();
    let (service, socket) = LspService::new(|client| Backend { client, docs: Mutex::new(HashMap::new()), builtins: builtins.clone() });
    Server::new(tokio::io::stdin(), tokio::io::stdout(), socket).serve(service).await;
}
