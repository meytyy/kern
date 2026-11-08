//! Parameter hints: while typing a call's arguments the language server's
//! `textDocument/signatureHelp` is shown above the caret, the current
//! parameter highlighted. `(` and `,` (or the server's trigger characters) ask
//! for it; `)`, Esc, leaving the editor or another tab close it.
use super::app::*;
use super::theme::pal;
use crate::lsp;
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};
use std::ops::Range;

#[derive(Clone)]
pub struct SignatureHint {
    pub editor: Entity<gpui_kit::component::input::EditorState>,
    pub label: String,
    /// Byte range of the active parameter in `label`.
    pub active: Option<Range<usize>>,
    pub doc: Option<String>,
    pub anchor: Point<Pixels>,
}

/// Byte range in `label` of a parameter given as text or as UTF-16 offsets.
fn param_range(label: &str, param: &Value) -> Option<Range<usize>> {
    match &param["label"] {
        Value::String(s) => label.find(s.as_str()).map(|i| i..i + s.len()),
        Value::Array(a) if a.len() == 2 => {
            let (start, end) = (a[0].as_u64()? as usize, a[1].as_u64()? as usize);
            let mut units = 0;
            let (mut s, mut e) = (None, None);
            for (i, ch) in label.char_indices().chain([(label.len(), ' ')]) {
                if units == start && s.is_none() {
                    s = Some(i);
                }
                if units == end && e.is_none() {
                    e = Some(i);
                }
                units += ch.len_utf16();
            }
            Some(s?..e?)
        }
        _ => None,
    }
}

/// The signature to show from a `SignatureHelp` reply.
pub fn parse(reply: &Value) -> Option<(String, Option<Range<usize>>, Option<String>)> {
    let sigs = reply["signatures"].as_array()?;
    let ix = reply["activeSignature"].as_u64().unwrap_or(0) as usize;
    let sig = sigs.get(ix).or_else(|| sigs.first())?;
    let label = sig["label"].as_str()?.to_string();
    let active = sig["activeParameter"]
        .as_u64()
        .or_else(|| reply["activeParameter"].as_u64())
        .unwrap_or(0) as usize;
    let range = sig["parameters"]
        .as_array()
        .and_then(|ps| ps.get(active))
        .and_then(|p| param_range(&label, p));
    let doc = match &sig["documentation"] {
        Value::String(s) => Some(s.clone()),
        Value::Object(o) => o.get("value").and_then(Value::as_str).map(str::to_string),
        _ => None,
    }
    .map(|d| {
        d.lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(160)
            .collect::<String>()
    })
    .filter(|d| !d.trim().is_empty());
    Some((label, range, doc))
}

impl KernApp {
    /// After an edit in `editor`: open, refresh or close the parameter hint.
    pub fn signature_after_edit(
        &mut self,
        editor: &Entity<gpui_kit::component::input::EditorState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(f) = self.active_file().filter(|f| &f.editor == editor) else {
            return;
        };
        let Some(client) = f.lsp.clone() else { return };
        if !client.supports("signatureHelpProvider") {
            return;
        }
        let uri = lsp::path_to_uri(&f.path);
        let (text, cursor) = {
            let e = editor.read(cx);
            (e.text().clone(), e.cursor())
        };
        let before = text.to_string()[..cursor].chars().last().unwrap_or(' ');
        if before == ')' || before == '\n' || before == ';' {
            self.signature = None;
            return;
        }
        let triggers: Vec<String> =
            client.capabilities()["signatureHelpProvider"]["triggerCharacters"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_else(|| vec!["(".into(), ",".into()]);
        let is_trigger = triggers.iter().any(|t| t.starts_with(before));
        // Typing inside an open hint refreshes it; otherwise only triggers open one.
        if !is_trigger && self.signature.is_none() {
            return;
        }
        let pos = super::lsp_bridge::offset_to_server(&text, cursor, client.encoding());
        let rx = client.request(
            "textDocument/signatureHelp",
            json!({"textDocument": {"uri": uri}, "position": {"line": pos.line, "character": pos.character}}),
        );
        let editor = editor.clone();
        cx.spawn_in(window, async move |this, cx| {
            let reply = rx.await;
            this.update_in(cx, |this, _, cx| {
                let parsed = match reply {
                    Ok(Ok(v)) if !v.is_null() => parse(&v),
                    _ => None,
                };
                this.signature = parsed.and_then(|(label, active, doc)| {
                    let e = editor.read(cx);
                    let c = e.cursor();
                    let b = e.range_to_bounds(&(c..c))?;
                    Some(SignatureHint {
                        editor: editor.clone(),
                        label,
                        active,
                        doc,
                        anchor: b.origin,
                    })
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn render_signature(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let hint = self.signature.clone()?;
        // Gone once the editor is not the active one or lost the focus.
        let still = self.active_file().is_some_and(|f| f.editor == hint.editor)
            && hint.editor.read(cx).focus_handle(cx).is_focused(window);
        if !still {
            self.signature = None;
            return None;
        }
        let p = pal(cx);
        let (a, b) = match &hint.active {
            Some(r) => (r.start, r.end),
            None => (hint.label.len(), hint.label.len()),
        };
        let label = hint.label.clone();
        let win = window.viewport_size();
        let width = px(520.).min(win.width - px(16.));
        let left = hint.anchor.x.min(win.width - width - px(8.)).max(px(8.));
        let top = (hint.anchor.y - px(if hint.doc.is_some() { 52. } else { 32. })).max(px(40.));
        Some(
            div()
                .id("signature-hint")
                .absolute()
                .left(left)
                .top(top)
                .max_w(width)
                .px_2()
                .py_1()
                .rounded(px(6.))
                .bg(p.bar)
                .border_1()
                .border_color(p.border)
                .shadow_md()
                .text_size(px(12.))
                .font_family(crate::core::settings::MONO_FONT)
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .child(div().text_color(p.muted).child(label[..a].to_string()))
                        .child(
                            div()
                                .text_color(p.fg)
                                .font_weight(FontWeight::BOLD)
                                .underline()
                                .child(label[a..b].to_string()),
                        )
                        .child(div().text_color(p.muted).child(label[b..].to_string())),
                )
                .when_some(hint.doc.clone(), |d, doc| {
                    d.child(
                        div()
                            .pt_1()
                            .font_family(".SystemUIFont")
                            .text_size(px(11.))
                            .text_color(p.muted)
                            .child(doc),
                    )
                })
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn picks_the_active_parameter() {
        let reply = json!({
            "signatures": [{
                "label": "fn add(a: i32, b: i32) -> i32",
                "documentation": {"kind": "markdown", "value": "Adds two numbers.\n\nMore."},
                "parameters": [{"label": [7, 13]}, {"label": "b: i32"}]
            }],
            "activeSignature": 0,
            "activeParameter": 1
        });
        let (label, range, doc) = parse(&reply).unwrap();
        assert_eq!(&label[range.unwrap()], "b: i32");
        assert_eq!(doc.as_deref(), Some("Adds two numbers."));
        let first = json!({"signatures": [{"label": "f(x: Строка, y)", "parameters": [{"label": [2, 11]}]}]});
        let (label, range, _) = parse(&first).unwrap();
        assert_eq!(&label[range.unwrap()], "x: Строка");
    }
}
