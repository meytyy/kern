//! The one table of keyboard shortcuts. Key bindings and every label shown for a
//! shortcut (menus, tooltips, the command palette, the Codex composer, hints)
//! come from here, so the two never disagree on either platform.

/// One shortcut, as GPUI keystrokes for macOS and for Windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Keys {
    pub mac: &'static str,
    pub win: &'static str,
}

const fn same(both: &'static str) -> Keys {
    Keys {
        mac: both,
        win: both,
    }
}

const fn k(mac: &'static str, win: &'static str) -> Keys {
    Keys { mac, win }
}

pub const SAVE: Keys = k("cmd-s", "ctrl-s");
pub const QUICK_OPEN: Keys = k("cmd-p", "ctrl-p");
pub const COMMAND_PALETTE: Keys = k("cmd-shift-p", "ctrl-shift-p");
pub const FIND_IN_PROJECT: Keys = k("cmd-shift-f", "ctrl-shift-f");
pub const SETTINGS: Keys = k("cmd-,", "ctrl-,");
pub const SEND: Keys = k("cmd-enter", "ctrl-enter");
pub const WORD_WRAP: Keys = same("alt-z");
pub const TOGGLE_SIDEBAR: Keys = k("cmd-b", "ctrl-b");
pub const TOGGLE_BOTTOM: Keys = k("cmd-j", "ctrl-j");
pub const TOGGLE_TERMINAL: Keys = same("ctrl-`");
/// Ctrl+Alt is AltGr on Windows keyboards, so no Windows shortcut uses it.
pub const TOGGLE_CODEX: Keys = k("cmd-alt-c", "ctrl-shift-a");
pub const OPEN_FOLDER: Keys = k("cmd-o", "ctrl-o");
pub const QUIT: Keys = k("cmd-q", "ctrl-q");
pub const RUN: Keys = k("cmd-r", "ctrl-r");
pub const STOP: Keys = k("cmd-.", "ctrl-.");
pub const EXTENSIONS: Keys = k("cmd-shift-x", "ctrl-shift-x");
pub const CLOSE_TAB: Keys = k("cmd-w", "ctrl-w");
pub const NEW_FILE: Keys = k("cmd-n", "ctrl-n");
pub const GO_TO_DEFINITION: Keys = same("f12");
pub const FORMAT_DOCUMENT: Keys = k("alt-shift-f", "shift-alt-f");
/// Taken by macOS for input sources unless changed; the palette has the command too.
pub const TRIGGER_SUGGEST: Keys = same("ctrl-space");

impl Keys {
    /// The keystroke for this build's platform.
    pub fn binding(self) -> &'static str {
        if cfg!(target_os = "macos") {
            self.mac
        } else {
            self.win
        }
    }

    /// The label for this build's platform: `⇧⌘P` on macOS, `Ctrl+Shift+P` on Windows.
    pub fn label(self) -> String {
        label(self.binding(), cfg!(target_os = "macos"))
    }
}

/// Formats a GPUI keystroke the way each platform writes shortcuts.
pub fn label(keystroke: &str, mac: bool) -> String {
    let parts: Vec<&str> = split(keystroke);
    let (mods, key) = parts.split_at(parts.len().saturating_sub(1));
    let key = key.first().copied().unwrap_or("");
    let has = |m: &str| mods.contains(&m);
    let key_text = match key {
        "enter" => if mac { "↩" } else { "Enter" }.to_string(),
        "tab" => "Tab".into(),
        "escape" => "Esc".into(),
        "space" => "Space".into(),
        "backspace" => if mac { "⌫" } else { "Backspace" }.to_string(),
        k if k.len() > 1 && k.starts_with('f') && k[1..].bytes().all(|b| b.is_ascii_digit()) => {
            k.to_uppercase()
        }
        k => k.to_uppercase(),
    };
    if mac {
        // Apple's order: Control, Option, Shift, Command.
        let mut s = String::new();
        for (m, glyph) in [("ctrl", "⌃"), ("alt", "⌥"), ("shift", "⇧"), ("cmd", "⌘")] {
            if has(m) {
                s.push_str(glyph);
            }
        }
        s + &key_text
    } else {
        let mut out: Vec<String> = vec![];
        // The order Windows apps write: Ctrl+Shift+Alt+key.
        for (m, name) in [
            ("ctrl", "Ctrl"),
            ("shift", "Shift"),
            ("alt", "Alt"),
            ("cmd", "Win"),
        ] {
            if has(m) {
                out.push(name.into());
            }
        }
        out.push(key_text);
        out.join("+")
    }
}

/// `ctrl--` and `cmd-shift--` end in the minus key itself.
fn split(keystroke: &str) -> Vec<&str> {
    if let Some(head) = keystroke.strip_suffix("--") {
        let mut v: Vec<&str> = if head.is_empty() {
            vec![]
        } else {
            head.split('-').collect()
        };
        v.push("-");
        v
    } else {
        keystroke.split('-').collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_requested_table() {
        let rows = [
            (SAVE, "⌘S", "Ctrl+S"),
            (QUICK_OPEN, "⌘P", "Ctrl+P"),
            (COMMAND_PALETTE, "⇧⌘P", "Ctrl+Shift+P"),
            (FIND_IN_PROJECT, "⇧⌘F", "Ctrl+Shift+F"),
            (SETTINGS, "⌘,", "Ctrl+,"),
            (SEND, "⌘↩", "Ctrl+Enter"),
            (WORD_WRAP, "⌥Z", "Alt+Z"),
            (TOGGLE_TERMINAL, "⌃`", "Ctrl+`"),
            (TOGGLE_CODEX, "⌥⌘C", "Ctrl+Shift+A"),
            (STOP, "⌘.", "Ctrl+."),
            (GO_TO_DEFINITION, "F12", "F12"),
            (FORMAT_DOCUMENT, "⌥⇧F", "Shift+Alt+F"),
            (TRIGGER_SUGGEST, "⌃Space", "Ctrl+Space"),
        ];
        for (keys, mac, win) in rows {
            assert_eq!(label(keys.mac, true), mac);
            assert_eq!(label(keys.win, false), win);
        }
        assert_eq!(label("ctrl--", false), "Ctrl+-");
    }

    #[test]
    fn windows_labels_and_bindings_never_use_mac_names() {
        let all = [
            SAVE,
            QUICK_OPEN,
            COMMAND_PALETTE,
            FIND_IN_PROJECT,
            SETTINGS,
            SEND,
            WORD_WRAP,
            TOGGLE_SIDEBAR,
            TOGGLE_BOTTOM,
            TOGGLE_TERMINAL,
            TOGGLE_CODEX,
            OPEN_FOLDER,
            QUIT,
            RUN,
            STOP,
            EXTENSIONS,
            CLOSE_TAB,
            NEW_FILE,
            GO_TO_DEFINITION,
            FORMAT_DOCUMENT,
            TRIGGER_SUGGEST,
        ];
        for keys in all {
            let l = label(keys.win, false);
            for bad in ["⌘", "⌥", "⌃", "⇧", "↩", "Cmd", "Option"] {
                assert!(!l.contains(bad), "{l}");
            }
            assert!(!keys.win.contains("cmd"), "{}", keys.win);
            // Ctrl+Alt is AltGr: it would type characters on many layouts.
            assert!(!(keys.win.contains("ctrl") && keys.win.contains("alt")));
        }
    }
}
