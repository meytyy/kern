//! Connects editor widgets with language servers. Column conversions happen
//! here: the server speaks its negotiated encoding, the widget Unicode scalars.
use super::app::*;
use super::editor_area::Tab;
use crate::lang;
use crate::lsp::client::StartOptions;
use crate::lsp::position::{Encoding, byte_to_server_col, server_col_to_scalar};
use crate::lsp::{self, LspClient, LspEvent, ServerStatus};
use crate::platform::tools;
use anyhow::{Result, anyhow};
use gpui_kit::component::input::{
    CompletionProvider, DefinitionProvider, HoverProvider, Rope, RopeExt,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use lsp_types::{
    CompletionContext, CompletionResponse, Hover, LocationLink, Position, Range as LspRange,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

fn line_text(rope: &Rope, line: u32) -> String {
    let line = line as usize;
    if line >= rope.lines_len() {
        return String::new();
    }
    let start = rope.line_start_offset(line);
    let end = rope.line_end_offset(line);
    rope.slice(start..end).to_string()
}

/// Server position → widget position, against the document text.
pub fn to_widget(rope: &Rope, pos: Position, enc: Encoding) -> Position {
    let text = line_text(rope, pos.line);
    Position::new(pos.line, server_col_to_scalar(&text, pos.character, enc))
}

pub fn range_to_widget(rope: &Rope, r: LspRange, enc: Encoding) -> LspRange {
    LspRange::new(to_widget(rope, r.start, enc), to_widget(rope, r.end, enc))
}

/// Byte offset → server position.
pub fn offset_to_server(rope: &Rope, offset: usize, enc: Encoding) -> Position {
    let pt = rope.offset_to_point(offset);
    let text = line_text(rope, pt.row as u32);
    Position::new(pt.row as u32, byte_to_server_col(&text, pt.column, enc))
}

pub struct LspProviders {
    client: Arc<LspClient>,
    uri: String,
}

impl LspProviders {
    fn doc_position(&self, text: &Rope, offset: usize) -> Value {
        let pos = offset_to_server(text, offset, self.client.encoding());
        json!({"textDocument": {"uri": self.uri}, "position": {"line": pos.line, "character": pos.character}})
    }
}

impl CompletionProvider for LspProviders {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        trigger: CompletionContext,
        _w: &mut Window,
        cx: &mut App,
    ) -> Task<Result<CompletionResponse>> {
        if !self.client.supports("completionProvider") {
            return Task::ready(Ok(CompletionResponse::Array(vec![])));
        }
        let mut params = self.doc_position(text, offset);
        params["context"] = serde_json::to_value(&trigger).unwrap_or(Value::Null);
        let rx = self.client.request("textDocument/completion", params);
        let rope = text.clone();
        let client = self.client.clone();
        cx.background_spawn(async move {
            let value = rx
                .await
                .map_err(|_| anyhow!("cancelled"))?
                .map_err(|e| anyhow!(e))?;
            if value.is_null() {
                return Ok(CompletionResponse::Array(vec![]));
            }
            let mut response: CompletionResponse = serde_json::from_value(value)?;
            let enc = client.encoding();
            let fix = |items: &mut Vec<lsp_types::CompletionItem>| {
                for item in items.iter_mut() {
                    if let Some(edit) = &mut item.text_edit {
                        match edit {
                            lsp_types::CompletionTextEdit::Edit(e) => {
                                e.range = range_to_widget(&rope, e.range, enc)
                            }
                            lsp_types::CompletionTextEdit::InsertAndReplace(e) => {
                                e.insert = range_to_widget(&rope, e.insert, enc);
                                e.replace = range_to_widget(&rope, e.replace, enc);
                            }
                        }
                    }
                }
            };
            match &mut response {
                CompletionResponse::Array(items) => fix(items),
                CompletionResponse::List(list) => fix(&mut list.items),
            }
            Ok(response)
        })
    }

    fn is_completion_trigger(&self, _offset: usize, new_text: &str, _cx: &mut App) -> bool {
        if new_text.is_empty() || new_text.contains('\n') {
            return false;
        }
        let last = new_text.chars().last().unwrap_or(' ');
        if last.is_alphanumeric() || last == '_' || last == '$' {
            return true;
        }
        let caps = self.client.capabilities();
        caps.get("completionProvider")
            .and_then(|c| c.get("triggerCharacters"))
            .and_then(Value::as_array)
            .is_some_and(|tc| {
                tc.iter()
                    .any(|t| t.as_str().is_some_and(|s| new_text.ends_with(s)))
            })
    }
}

impl HoverProvider for LspProviders {
    fn hover(
        &self,
        text: &Rope,
        offset: usize,
        _w: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Option<Hover>>> {
        if !self.client.supports("hoverProvider") {
            return Task::ready(Ok(None));
        }
        let rx = self
            .client
            .request("textDocument/hover", self.doc_position(text, offset));
        let rope = text.clone();
        let client = self.client.clone();
        cx.background_spawn(async move {
            let value = rx
                .await
                .map_err(|_| anyhow!("cancelled"))?
                .map_err(|e| anyhow!(e))?;
            if value.is_null() {
                return Ok(None);
            }
            let mut hover: Hover = serde_json::from_value(value)?;
            if let Some(r) = hover.range {
                hover.range = Some(range_to_widget(&rope, r, client.encoding()));
            }
            Ok(Some(hover))
        })
    }
}

impl DefinitionProvider for LspProviders {
    fn definitions(
        &self,
        text: &Rope,
        offset: usize,
        _w: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<LocationLink>>> {
        if !self.client.supports("definitionProvider") {
            return Task::ready(Ok(vec![]));
        }
        let rx = self
            .client
            .request("textDocument/definition", self.doc_position(text, offset));
        let rope = text.clone();
        let client = self.client.clone();
        cx.background_spawn(async move {
            let value = rx
                .await
                .map_err(|_| anyhow!("cancelled"))?
                .map_err(|e| anyhow!(e))?;
            Ok(parse_definition(value, &rope, client.encoding()))
        })
    }
}

/// Definition replies come as a Location, an array of Locations, or LocationLinks.
/// Target ranges stay in the server's encoding (see `show_document`), while the
/// origin range refers to this document and is converted for the widget.
pub fn parse_definition(value: Value, rope: &Rope, enc: Encoding) -> Vec<LocationLink> {
    let items: Vec<Value> = match value {
        Value::Null => vec![],
        Value::Array(a) => a,
        one => vec![one],
    };
    items
        .into_iter()
        .filter_map(|v| {
            if let Ok(mut link) = serde_json::from_value::<LocationLink>(v.clone()) {
                link.origin_selection_range = link
                    .origin_selection_range
                    .map(|r| range_to_widget(rope, r, enc));
                return Some(link);
            }
            let loc: lsp_types::Location = serde_json::from_value(v).ok()?;
            Some(LocationLink {
                origin_selection_range: None,
                target_uri: loc.uri,
                target_range: loc.range,
                target_selection_range: loc.range,
            })
        })
        .collect()
}

impl KernApp {
    /// Bind the file to a language server, starting it if needed. Lazy: the
    /// server starts when the first file of its language is opened.
    pub fn attach_lsp(
        &mut self,
        path: &Path,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(p) = &mut self.project else { return };
        if !p.trusted {
            return;
        }
        let Some(config) = lang::server_for(path, &p.root, &self.settings) else {
            return;
        };
        let lang_def = lang::resolve(path, Some(&p.root), &self.settings);
        let key = (config.id.clone(), config.root.clone());
        let client = match p.lsp_clients.get(&key) {
            Some(c) => c.clone(),
            None => {
                let program = config.candidates.iter().find_map(|c| {
                    if c.contains('/') {
                        Some(PathBuf::from(c))
                    } else {
                        tools::find(c)
                    }
                });
                let Some(program) = program else {
                    let tab = p.tabs.iter_mut().find_map(|t| match t {
                        Tab::File(f) if f.path == path => Some(f),
                        _ => None,
                    });
                    if let Some(f) = tab {
                        f.lsp_failed = Some((
                            config.id.clone(),
                            format!("not found · {}", config.install_hint),
                        ));
                    }
                    p.lsp_status.insert(
                        config.id.clone(),
                        ServerStatus::Missing {
                            hint: config.install_hint.clone(),
                        },
                    );
                    cx.notify();
                    return;
                };
                let (tx, mut rx) = futures::channel::mpsc::unbounded();
                let started = LspClient::start(
                    StartOptions {
                        server_id: &config.id,
                        program: &program,
                        args: &config.args,
                        root: &config.root,
                        init_options: config.init_options.clone(),
                        settings: config.settings.clone(),
                        install_hint: &config.install_hint,
                    },
                    tx,
                );
                let client = match started {
                    Ok(c) => c,
                    Err(e) => {
                        p.lsp_status
                            .insert(config.id.clone(), ServerStatus::Failed(e.to_string()));
                        cx.notify();
                        return;
                    }
                };
                p.lsp_status
                    .insert(config.id.clone(), ServerStatus::Starting);
                p.lsp_clients.insert(key, client.clone());
                cx.spawn_in(window, async move |this, cx| {
                    use futures::StreamExt;
                    while let Some(ev) = rx.next().await {
                        if this
                            .update_in(cx, |this, window, cx| this.on_lsp_event(ev, window, cx))
                            .is_err()
                        {
                            break;
                        }
                    }
                })
                .detach();
                client
            }
        };
        let uri = lsp::path_to_uri(path);
        client.did_open(&uri, lang_def.lsp_id, 1, text);
        let providers = Rc::new(LspProviders {
            client: client.clone(),
            uri,
        });
        let this = cx.entity().downgrade();
        let enc_client = client.clone();
        let show: gpui_kit::component::input::ShowDocumentHandler =
            Rc::new(move |params, window, cx| {
                let Some(path) = uri_to_path(params.uri.as_str()) else {
                    return false;
                };
                let pos = params.selection.map(|r| r.start).unwrap_or_default();
                let enc = enc_client.encoding();
                this.update(cx, |this, cx| {
                    this.open_at_server_position(path, pos, enc, window, cx)
                })
                .is_ok()
            });
        let tab = p.tabs.iter_mut().find_map(|t| match t {
            Tab::File(f) if f.path == path => Some(f),
            _ => None,
        });
        let mut pull_for = false;
        if let Some(f) = tab {
            f.lsp = Some(client);
            f.lsp_failed = None;
            pull_for = true;
            f.editor.update(cx, |e, _| {
                let l = e.lsp_mut();
                l.completion_provider = Some(providers.clone());
                l.hover_provider = Some(providers.clone());
                l.definition_provider = Some(providers.clone());
                l.show_document = Some(show);
            });
        }
        if pull_for {
            self.schedule_pull_diagnostics(path.to_path_buf(), cx);
        }
        cx.notify();
    }

    /// LSP 3.17 pull diagnostics (used by TypeScript 7), debounced per file.
    /// Servers with `publishDiagnostics` simply never advertise this.
    pub fn schedule_pull_diagnostics(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(p) = &mut self.project else { return };
        let Some(f) = p.tabs.iter_mut().find_map(|t| match t {
            Tab::File(f) if f.path == path => Some(f),
            _ => None,
        }) else {
            return;
        };
        let Some(client) = f.lsp.clone() else { return };
        f.pull_generation += 1;
        let generation = f.pull_generation;
        let uri = lsp::path_to_uri(&path);
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(250))
                .await;
            let current = this
                .update(cx, |this, _| {
                    this.project
                        .as_ref()
                        .and_then(|p| {
                            p.tabs
                                .iter()
                                .find_map(|t| t.as_file().filter(|f| f.path == path))
                        })
                        .map(|f| f.pull_generation)
                })
                .ok()
                .flatten();
            if current != Some(generation) || !client.supports("diagnosticProvider") {
                return;
            }
            let rx = client.request(
                "textDocument/diagnostic",
                json!({"textDocument": {"uri": uri}}),
            );
            let Ok(Ok(result)) = rx.await else { return };
            if result.get("kind").and_then(Value::as_str) != Some("full") {
                return;
            }
            let items = result.get("items").cloned().unwrap_or(json!([]));
            this.update(cx, |this, cx| {
                this.apply_diagnostics(&uri, items, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn reattach_lsp_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let files: Vec<(PathBuf, String)> = self
            .project
            .as_ref()
            .map(|p| {
                p.tabs
                    .iter()
                    .filter_map(|t| t.as_file())
                    .map(|f| (f.path.clone(), f.editor.read(cx).text().to_string()))
                    .collect()
            })
            .unwrap_or_default();
        for (path, text) in files {
            self.attach_lsp(&path, text, window, cx);
        }
    }

    /// Open a file and place the cursor from a position in server encoding.
    pub fn open_at_server_position(
        &mut self,
        path: PathBuf,
        pos: Position,
        enc: Encoding,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Column conversion needs the target line.
        let scalar = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| {
                t.lines()
                    .nth(pos.line as usize)
                    .map(|l| server_col_to_scalar(l.trim_end_matches('\r'), pos.character, enc))
            })
            .unwrap_or(0);
        self.open_file_async(path, true, Some((pos.line, scalar)), window, cx);
    }

    fn on_lsp_event(&mut self, ev: LspEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = &mut self.project else { return };
        match ev {
            LspEvent::Status { server, status } => {
                if matches!(status, ServerStatus::Exited | ServerStatus::Failed(_)) {
                    p.lsp_clients.retain(|(id, _), _| *id != server);
                    for t in &mut p.tabs {
                        if let Tab::File(f) = t
                            && f.lsp.as_ref().is_some_and(|c| c.server_id == server)
                        {
                            f.lsp = None;
                            f.lsp_failed = Some((
                                server.clone(),
                                match &status {
                                    ServerStatus::Failed(e) => format!("failed: {e}"),
                                    _ => "stopped; reopen the file to restart".into(),
                                },
                            ));
                        }
                    }
                }
                p.lsp_status.insert(server, status);
            }
            LspEvent::Diagnostics { uri, diagnostics } => {
                self.apply_diagnostics(&uri, diagnostics, cx)
            }
        }
        cx.notify();
    }

    fn apply_diagnostics(&mut self, uri: &str, diagnostics: Value, cx: &mut Context<Self>) {
        let Some(p) = &mut self.project else { return };
        let Some(path) = uri_to_path(uri) else { return };
        for t in &mut p.tabs {
            let Tab::File(f) = t else { continue };
            if f.path != path {
                continue;
            }
            let enc = f.lsp.as_ref().map(|c| c.encoding()).unwrap_or_default();
            let items: Vec<lsp_types::Diagnostic> =
                serde_json::from_value(diagnostics).unwrap_or_default();
            let errors = items
                .iter()
                .filter(|d| d.severity == Some(lsp_types::DiagnosticSeverity::ERROR))
                .count();
            let warnings = items
                .iter()
                .filter(|d| d.severity == Some(lsp_types::DiagnosticSeverity::WARNING))
                .count();
            f.diagnostic_counts = (errors, warnings);
            f.editor.update(cx, |e, cx| {
                let rope = e.text().clone();
                if let Some(set) = e.diagnostics_mut() {
                    set.clear();
                    set.extend(items.into_iter().map(|mut d| {
                        d.range = range_to_widget(&rope, d.range, enc);
                        d
                    }));
                }
                cx.notify();
            });
            break;
        }
    }

    pub fn go_to_definition(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(f) = self.active_file() {
            let focus = f.editor.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
            window.dispatch_action(Box::new(gpui_kit::component::input::GoToDefinition), cx);
        }
    }
}

pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    lsp::uri_to_path(uri)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn converts_positions_against_rope_text() {
        let rope = Rope::from("first\nконст 😀 = x;\n");
        // UTF-16: "конст " is 6 units, the emoji 2, so `=` is at 9.
        let w = to_widget(&rope, Position::new(1, 9), Encoding::Utf16);
        assert_eq!(w, Position::new(1, 8));
        let offset = rope.position_to_offset(&w);
        assert_eq!(rope.slice(offset..offset + 1).to_string(), "=");
        assert_eq!(
            offset_to_server(&rope, offset, Encoding::Utf16),
            Position::new(1, 9)
        );
        // Out of range line and column clamp rather than panic.
        assert_eq!(
            to_widget(&rope, Position::new(99, 5), Encoding::Utf16),
            Position::new(99, 0)
        );
        assert_eq!(
            to_widget(&rope, Position::new(0, 500), Encoding::Utf16),
            Position::new(0, 5)
        );
    }

    #[::core::prelude::v1::test]
    fn definition_reply_shapes() {
        let rope = Rope::from("x\n");
        let loc = json!({"uri":"file:///a.ts","range":{"start":{"line":1,"character":2},"end":{"line":1,"character":3}}});
        assert_eq!(
            parse_definition(loc.clone(), &rope, Encoding::Utf16).len(),
            1
        );
        assert_eq!(
            parse_definition(json!([loc]), &rope, Encoding::Utf16).len(),
            1
        );
        assert!(parse_definition(Value::Null, &rope, Encoding::Utf16).is_empty());
        let link = json!([{"targetUri":"file:///a.ts","targetRange":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},"targetSelectionRange":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}}}]);
        assert_eq!(parse_definition(link, &rope, Encoding::Utf16).len(), 1);
    }
}
