//! Format Document (⇧⌥F / Shift+Alt+F): the language server's
//! `textDocument/formatting` on the whole buffer, unsaved edits included. The
//! result replaces the text as one undo step; the cursor stays on its line and
//! column, and line endings stay the file's own (the buffer is LF, the file
//! format remembers CRLF). Project settings (rustfmt.toml, .prettierrc, …)
//! are read by the server itself.
use super::app::*;
use crate::lsp::{self, Encoding};
use gpui_kit::*;
use serde_json::{Value, json};

/// Byte offset of an LSP position in LF text, in the server's encoding.
fn offset_of(text: &str, line: u64, character: u64, enc: Encoding) -> usize {
    let mut start = 0usize;
    for _ in 0..line {
        match text[start..].find('\n') {
            Some(i) => start += i + 1,
            None => return text.len(),
        }
    }
    let end = text[start..].find('\n').map_or(text.len(), |i| start + i);
    let line_text = &text[start..end];
    let mut units = 0u64;
    for (i, ch) in line_text.char_indices() {
        if units >= character {
            return start + i;
        }
        units += match enc {
            Encoding::Utf8 => ch.len_utf8() as u64,
            Encoding::Utf16 => ch.len_utf16() as u64,
            Encoding::Utf32 => 1,
        };
    }
    end
}

/// Applies `TextEdit[]` to `text`. Edits never overlap (LSP guarantees it), so
/// they are applied from the last to the first.
pub fn apply_text_edits(text: &str, edits: &[Value], enc: Encoding) -> Result<String, String> {
    apply_text_edits_at(text, edits, enc, 0).map(|(t, _)| t)
}

/// The same, and where byte offset `cursor` of `text` ends up in the result:
/// edits before it (or inserting at it) move it by their change in length; in
/// a replaced range it keeps its distance from the range's start.
pub fn apply_text_edits_at(
    text: &str,
    edits: &[Value],
    enc: Encoding,
    cursor: usize,
) -> Result<(String, usize), String> {
    let mut spans = Vec::with_capacity(edits.len());
    for e in edits {
        let r = &e["range"];
        let n = |v: &Value| v.as_u64().ok_or("bad position in a formatting edit");
        let start = offset_of(
            text,
            n(&r["start"]["line"])?,
            n(&r["start"]["character"])?,
            enc,
        );
        let end = offset_of(text, n(&r["end"]["line"])?, n(&r["end"]["character"])?, enc);
        // The buffer is LF; a server that answers with CRLF gets LF here and the
        // file keeps its own line endings when saved.
        let new_text = e["newText"].as_str().unwrap_or("").replace("\r\n", "\n");
        spans.push((start.min(end), end.max(start), new_text));
    }
    spans.sort_by_key(|s| std::cmp::Reverse(s.0));
    let mut out = text.to_string();
    let mut limit = usize::MAX;
    let mut moved = cursor as isize;
    for (start, end, new_text) in spans {
        if end > limit {
            return Err("the formatter returned overlapping edits".into());
        }
        let grow = new_text.len() as isize - (end - start) as isize;
        if end <= cursor {
            moved += grow;
        } else if start < cursor {
            moved += (cursor - start).min(new_text.len()) as isize - (cursor - start) as isize;
        }
        out.replace_range(start..end, &new_text);
        limit = start;
    }
    let mut at = (moved.max(0) as usize).min(out.len());
    while !out.is_char_boundary(at) {
        at -= 1;
    }
    Ok((out, at))
}

impl KernApp {
    /// Ctrl+Space and the palette's Trigger Suggest: the completion list at the caret.
    pub fn trigger_suggest(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.active_file() else { return };
        let (editor, no_server, lang) = (f.editor.clone(), f.lsp.is_none(), f.lang.display);
        if no_server {
            self.toast(
                format!(
                    "No suggestions for {lang}: no language server is running (Settings → Languages)"
                ),
                false,
                cx,
            );
        }
        editor.update(cx, |e, cx| {
            e.focus(window, cx);
            e.show_completions(window, cx)
        });
    }

    pub fn format_document(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.active_file() else {
            self.toast("Open a file to format it", false, cx);
            return;
        };
        let (path, editor, lang) = (f.path.clone(), f.editor.clone(), f.lang);
        let trusted = self.project.as_ref().is_some_and(|p| p.trusted);
        let Some(client) = f.lsp.clone() else {
            let reason = match (lang.server, &f.lsp_failed) {
                (None, _) => format!(
                    "No formatter for {}: KERN has no language server for it",
                    lang.display
                ),
                (Some(_), _) if !trusted => format!(
                    "Formatting {} uses its language server, which runs after you trust this folder (Settings → Languages)",
                    lang.display
                ),
                (Some(s), Some((_, why))) => {
                    format!("The {} language server is not running: {why}", s.info().id)
                }
                (Some(s), None) => format!(
                    "The {} language server is not running: see Settings → Languages",
                    s.info().id
                ),
            };
            self.toast(reason, false, cx);
            return;
        };
        if !client.supports("documentFormattingProvider") {
            self.toast(
                format!(
                    "{} cannot format {} files",
                    lang.server
                        .map(|s| s.info().id)
                        .unwrap_or("The language server"),
                    lang.display
                ),
                false,
                cx,
            );
            return;
        }
        let text = editor.read(cx).text().to_string();
        let indent = crate::core::indent::detect(&path, &text, self.settings.editor.tab_size);
        let params = json!({
            "textDocument": {"uri": lsp::path_to_uri(&path)},
            "options": {"tabSize": indent.size, "insertSpaces": !indent.hard_tabs},
        });
        let rx = client.request("textDocument/formatting", params);
        let enc = client.encoding();
        cx.spawn_in(window, async move |this, cx| {
            let reply = rx.await;
            this.update_in(cx, |this, window, cx| {
                let edits = match reply {
                    Ok(Ok(Value::Array(edits))) => edits,
                    Ok(Ok(_)) => vec![],
                    Ok(Err(e)) => {
                        this.toast(format!("Format Document failed: {e}"), true, cx);
                        return;
                    }
                    Err(_) => {
                        this.toast("Format Document: the language server stopped", true, cx);
                        return;
                    }
                };
                // Typing while the server worked: its edits are for older text.
                if *editor.read(cx).text() != text {
                    this.toast("The file changed while formatting; try again", false, cx);
                    return;
                }
                let cursor = editor.read(cx).cursor();
                let (formatted, at) = match apply_text_edits_at(&text, &edits, enc, cursor) {
                    Ok(r) => r,
                    Err(e) => {
                        this.toast(format!("Format Document failed: {e}"), true, cx);
                        return;
                    }
                };
                if formatted == text {
                    this.toast("Already formatted", false, cx);
                    return;
                }
                // The same code under the cursor, in the widget's line and
                // Unicode-scalar column.
                let before = &formatted[..at];
                let line = before.matches('\n').count() as u32;
                let column = before[before.rfind('\n').map_or(0, |i| i + 1)..]
                    .chars()
                    .count() as u32;
                editor.update(cx, |e, cx| {
                    e.replace_all(formatted, window, cx);
                    e.set_cursor_position(lsp_types::Position::new(line, column), window, cx);
                });
            })
            .ok();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `gpui_kit::*` brings its own `test` attribute; this is the standard one.
    #[::core::prelude::v1::test]
    fn edits_apply_in_any_order_with_utf16_positions() {
        let text = "fn  main(){\n    let s = \"ж\";let x=1;\n}\n";
        let edits = vec![
            json!({"range": {"start": {"line": 0, "character": 2}, "end": {"line": 0, "character": 4}}, "newText": " "}),
            json!({"range": {"start": {"line": 1, "character": 16}, "end": {"line": 1, "character": 16}}, "newText": "\r\n    "}),
            json!({"range": {"start": {"line": 0, "character": 10}, "end": {"line": 0, "character": 10}}, "newText": " "}),
        ];
        let out = apply_text_edits(text, &edits, Encoding::Utf16).unwrap();
        assert_eq!(out, "fn main() {\n    let s = \"ж\";\n    let x=1;\n}\n");
        assert!(!out.contains('\r'));
        // The cursor on "x" of "let x=1" stays on it.
        let at = text.find("x=1").unwrap();
        let (out, moved) = apply_text_edits_at(text, &edits, Encoding::Utf16, at).unwrap();
        assert!(out[moved..].starts_with("x=1"));
        // Inside a replaced range: same distance from its start.
        let whole = vec![
            json!({"range": {"start": {"line": 0, "character": 0}, "end": {"line": 3, "character": 0}}, "newText": "ab"}),
        ];
        let (_, moved) = apply_text_edits_at(text, &whole, Encoding::Utf16, 1).unwrap();
        assert_eq!(moved, 1);
    }
}
