//! Persistent settings and session state, stored as JSON in the platform config dir.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The monospaced font every platform has: Menlo on macOS, Consolas on Windows.
pub const MONO_FONT: &str = if cfg!(windows) { "Consolas" } else { "Menlo" };

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    /// Graphite. Earlier builds called the dark theme Navy: that setting maps here.
    #[default]
    #[serde(alias = "navy", alias = "graphite")]
    Dark,
    /// Cream.
    #[serde(alias = "cream")]
    Light,
    /// Follow the macOS appearance.
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct EditorSettings {
    pub font_family: String,
    pub font_size: f32,
    pub line_numbers: bool,
    pub word_wrap: bool,
    pub tab_size: usize,
    pub indent_guides: bool,
    pub active_line: bool,
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            font_family: MONO_FONT.into(),
            font_size: 13.0,
            line_numbers: true,
            word_wrap: false,
            tab_size: 2,
            indent_guides: true,
            active_line: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TerminalSettings {
    pub font_size: f32,
    pub scrollback: usize,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            font_size: 13.0,
            scrollback: 10_000,
        }
    }
}

/// Options of the native Claude chat. The API key is not here: it is in the Keychain.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ClaudeChatSettings {
    /// Empty: the SDK's default model.
    pub model: String,
    /// Empty: the model's default effort.
    pub effort: String,
    pub permission_mode: String,
}

/// The model chosen for Codex. Empty values leave Codex's own defaults in force.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct CodexChatSettings {
    pub model: String,
    pub effort: String,
}

impl Default for ClaudeChatSettings {
    fn default() -> Self {
        Self {
            model: String::new(),
            effort: String::new(),
            permission_mode: "default".into(),
        }
    }
}

/// State of one built-in extension. External CLIs, sign-ins and chat history are
/// never affected by these flags.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct ExtensionState {
    pub installed: bool,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    pub sidebar_visible: bool,
    pub sidebar_width: f32,
    pub bottom_visible: bool,
    pub bottom_height: f32,
    pub codex_visible: bool,
    pub codex_width: f32,
    pub window_width: f32,
    pub window_height: f32,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            sidebar_visible: true,
            sidebar_width: 260.0,
            bottom_visible: true,
            bottom_height: 240.0,
            codex_visible: false,
            codex_width: 360.0,
            window_width: 1320.0,
            window_height: 840.0,
        }
    }
}

/// A user-defined language server.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct CustomLsp {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub extensions: Vec<String>,
    pub root_markers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProjectSession {
    pub open_files: Vec<PathBuf>,
    pub active_file: Option<PathBuf>,
    pub run_config: Option<String>,
    /// Selected Python interpreter for this project.
    pub python: Option<PathBuf>,
    /// User-selected JS package manager, if detection was ambiguous.
    pub package_manager: Option<String>,
    /// Language chosen by hand for a file of this project, by relative path.
    pub language_overrides: BTreeMap<String, String>,
    /// Bottom panel of this project. `None`: never set, so hidden.
    pub bottom_visible: Option<bool>,
    pub bottom_height: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeChoice,
    pub layout: Layout,
    pub recent_projects: Vec<PathBuf>,
    /// Folders where language servers may run. Language servers can execute
    /// project code (build scripts, tsserver plugins, interpreters), so this
    /// is an explicit per-folder decision.
    pub trusted_projects: Vec<PathBuf>,
    pub sessions: BTreeMap<String, ProjectSession>,
    /// Explicit paths for external tools, by tool name (`git`, `node`, `codex`, ...).
    pub tool_paths: BTreeMap<String, PathBuf>,
    /// Shell for new terminals. Falls back to `$SHELL`, then `/bin/zsh`.
    pub shell: Option<String>,
    pub custom_lsp: Vec<CustomLsp>,
    pub editor: EditorSettings,
    pub terminal: TerminalSettings,
    /// `*.ext` or an exact file name → language id.
    pub file_associations: BTreeMap<String, String>,
    /// `None` only in a settings file from before extensions existed.
    pub extensions: Option<BTreeMap<String, ExtensionState>>,
    pub graph_all_branches: bool,
    pub graph_height: f32,
    pub graph_hidden: bool,
    pub claude_chat: ClaudeChatSettings,
    pub codex_chat: CodexChatSettings,
}

impl Settings {
    pub fn path() -> PathBuf {
        config_dir().join("settings.json")
    }

    pub fn load() -> Self {
        Self::load_from(&Self::path())
    }

    /// A broken settings file is preserved next to the original instead of being lost.
    pub fn load_from(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        match serde_json::from_str::<Self>(&text) {
            Ok(mut s) => {
                // Menlo was the default everywhere, but Windows has no Menlo and
                // substituted a proportional font: code and the terminal misaligned.
                if cfg!(windows) && s.editor.font_family == "Menlo" {
                    s.editor.font_family = MONO_FONT.into();
                }
                // Upgrading: integrations that already worked stay available.
                if s.extensions.is_none() {
                    let on = ExtensionState {
                        installed: true,
                        enabled: true,
                    };
                    s.extensions = Some(
                        ["codex", "claude"]
                            .iter()
                            .map(|id| (id.to_string(), on))
                            .collect(),
                    );
                }
                s
            }
            Err(e) => {
                log::warn!("settings file is invalid: {e}");
                let _ = std::fs::copy(path, path.with_extension("json.broken"));
                Self::default()
            }
        }
    }

    pub fn save(&self) {
        if let Err(e) = self.save_to(&Self::path()) {
            log::warn!("cannot save settings: {e}");
        }
    }

    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        if !path.exists() {
            std::fs::write(path, b"")?;
        }
        crate::core::document::atomic_write(path, serde_json::to_string_pretty(self)?.as_bytes())
    }

    pub fn extension(&self, id: &str) -> ExtensionState {
        self.extensions
            .as_ref()
            .and_then(|m| m.get(id))
            .copied()
            .unwrap_or_default()
    }

    pub fn extension_enabled(&self, id: &str) -> bool {
        let e = self.extension(id);
        e.installed && e.enabled
    }

    pub fn set_extension(&mut self, id: &str, state: ExtensionState) {
        self.extensions
            .get_or_insert_with(BTreeMap::new)
            .insert(id.to_string(), state);
    }

    pub fn add_recent(&mut self, project: &Path) {
        self.recent_projects.retain(|p| p != project);
        self.recent_projects.insert(0, project.to_path_buf());
        self.recent_projects.truncate(15);
    }

    pub fn session(&self, project: &Path) -> ProjectSession {
        self.sessions
            .get(&project.to_string_lossy().to_string())
            .cloned()
            .unwrap_or_default()
    }

    pub fn session_mut(&mut self, project: &Path) -> &mut ProjectSession {
        self.sessions
            .entry(project.to_string_lossy().to_string())
            .or_default()
    }
}

pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("KERN_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("KERN")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navy_and_cream_from_earlier_builds_load_as_graphite_and_cream() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        for (stored, want) in [
            ("navy", ThemeChoice::Dark),
            ("graphite", ThemeChoice::Dark),
            ("dark", ThemeChoice::Dark),
            ("cream", ThemeChoice::Light),
            ("light", ThemeChoice::Light),
            ("system", ThemeChoice::System),
        ] {
            std::fs::write(
                &path,
                format!("{{\"theme\": \"{stored}\", \"editor\": {{\"font_size\": 15}}}}"),
            )
            .unwrap();
            let s = Settings::load_from(&path);
            assert_eq!(s.theme, want, "{stored}");
            // The rest of the file survives the migration.
            assert_eq!(s.editor.font_size, 15.0);
        }
    }

    #[test]
    fn roundtrip_and_partial_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s/settings.json");
        let mut s = Settings {
            theme: ThemeChoice::Light,
            ..Default::default()
        };
        s.layout.sidebar_width = 333.0;
        s.add_recent(Path::new("/tmp/Мой проект"));
        s.save_to(&path).unwrap();
        let back = Settings::load_from(&path);
        assert_eq!(back.theme, ThemeChoice::Light);
        assert_eq!(back.layout.sidebar_width, 333.0);
        assert_eq!(back.recent_projects, vec![PathBuf::from("/tmp/Мой проект")]);

        std::fs::write(&path, r#"{"layout":{"codex_visible":true}}"#).unwrap();
        let back = Settings::load_from(&path);
        assert!(back.layout.codex_visible);
        assert_eq!(back.layout.sidebar_width, 260.0);
    }

    #[test]
    fn upgrade_keeps_integrations_and_defaults_stay_off_for_new_installs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"theme":"light"}"#).unwrap();
        let old = Settings::load_from(&path);
        assert!(old.extension_enabled("codex") && old.extension_enabled("claude"));
        assert!(!old.editor.word_wrap);
        assert_eq!(old.editor.font_size, 13.0);
        let fresh = Settings::load_from(&dir.path().join("none.json"));
        assert!(!fresh.extension_enabled("codex"));
        // A saved choice survives a round trip.
        let mut s = fresh;
        s.editor.word_wrap = true;
        s.theme = ThemeChoice::System;
        s.set_extension(
            "codex",
            ExtensionState {
                installed: true,
                enabled: false,
            },
        );
        s.save_to(&path).unwrap();
        let back = Settings::load_from(&path);
        assert!(back.editor.word_wrap);
        assert_eq!(back.theme, ThemeChoice::System);
        assert!(back.extension("codex").installed && !back.extension_enabled("codex"));
    }

    #[test]
    fn broken_file_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{oops").unwrap();
        let s = Settings::load_from(&path);
        assert_eq!(s.theme, ThemeChoice::Dark);
        assert!(path.with_extension("json.broken").exists());
    }
}
