//! Local index of agent chats: agent, project, the official session id, a title
//! and the last activity. No transcripts or tokens are ever stored here: the
//! conversations themselves stay with the official clients.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionEntry {
    pub agent: String,
    pub project: PathBuf,
    pub id: String,
    pub title: String,
    /// The user renamed it: integrations must not overwrite the title.
    #[serde(default)]
    pub custom_title: bool,
    pub last_active: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct SessionIndex {
    pub entries: Vec<SessionEntry>,
    /// `agent|project` → session to reopen after a restart.
    pub last_open: BTreeMap<String, String>,
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// A title from the first line of a message.
pub fn title_from_message(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("New chat");
    let mut t: String = line.chars().take(60).collect();
    if line.chars().count() > 60 {
        t.push('…');
    }
    t
}

pub fn relative_time(at: i64, now: i64) -> String {
    let d = (now - at).max(0);
    match d {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m", d / 60),
        3600..=86399 => format!("{}h", d / 3600),
        86400..=2591999 => format!("{}d", d / 86400),
        _ => format!("{}mo", d / 2592000),
    }
}

fn last_key(agent: &str, project: &Path) -> String {
    format!("{agent}|{}", project.display())
}

impl SessionIndex {
    pub fn path() -> PathBuf {
        crate::core::settings::config_dir().join("agent-sessions.json")
    }

    pub fn load() -> Self {
        Self::load_from(&Self::path())
    }

    pub fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Err(e) = self.save_to(&Self::path()) {
            log::warn!("cannot save the session index: {e}");
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

    /// Insert or update. A title from an integration never replaces a user's.
    pub fn upsert(
        &mut self,
        agent: &str,
        project: &Path,
        id: &str,
        title: Option<&str>,
        last_active: Option<i64>,
    ) {
        match self
            .entries
            .iter_mut()
            .find(|e| e.agent == agent && e.id == id && e.project == project)
        {
            Some(e) => {
                if let Some(t) = title.filter(|t| !t.is_empty())
                    && !e.custom_title
                {
                    e.title = t.to_string();
                }
                if let Some(a) = last_active {
                    e.last_active = e.last_active.max(a);
                }
            }
            None => self.entries.push(SessionEntry {
                agent: agent.to_string(),
                project: project.to_path_buf(),
                id: id.to_string(),
                title: title
                    .filter(|t| !t.is_empty())
                    .unwrap_or("Untitled chat")
                    .to_string(),
                custom_title: false,
                last_active: last_active.unwrap_or_else(now),
            }),
        }
    }

    pub fn rename(&mut self, agent: &str, project: &Path, id: &str, title: &str) {
        if let Some(e) = self
            .entries
            .iter_mut()
            .find(|e| e.agent == agent && e.id == id && e.project == project)
        {
            e.title = title.to_string();
            e.custom_title = true;
        }
    }

    /// Chats of one agent in one project, most recent first, filtered by title.
    pub fn list(&self, agent: &str, project: &Path, query: &str) -> Vec<SessionEntry> {
        let q = query.to_lowercase();
        let mut out: Vec<SessionEntry> = self
            .entries
            .iter()
            .filter(|e| {
                e.agent == agent
                    && e.project == project
                    && (q.is_empty() || e.title.to_lowercase().contains(&q))
            })
            .cloned()
            .collect();
        out.sort_by_key(|e| std::cmp::Reverse(e.last_active));
        out
    }

    pub fn set_last_open(&mut self, agent: &str, project: &Path, id: &str) {
        self.last_open
            .insert(last_key(agent, project), id.to_string());
    }

    pub fn get_last_open(&self, agent: &str, project: &Path) -> Option<&String> {
        self.last_open.get(&last_key(agent, project))
    }

    pub fn clear_last_open(&mut self, agent: &str, project: &Path) {
        self.last_open.remove(&last_key(agent, project));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_are_per_agent_and_project_and_sorted() {
        let mut ix = SessionIndex::default();
        let (a, b) = (Path::new("/a"), Path::new("/b"));
        ix.upsert("codex", a, "1", Some("first"), Some(10));
        ix.upsert("codex", a, "2", Some("second"), Some(20));
        ix.upsert("codex", b, "3", Some("other"), Some(30));
        ix.upsert("claude", a, "4", Some("c"), Some(40));
        let l = ix.list("codex", a, "");
        assert_eq!(
            l.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            vec!["2", "1"]
        );
        assert_eq!(ix.list("codex", b, "").len(), 1);
        assert_eq!(ix.list("codex", a, "FIR").len(), 1);
    }

    #[test]
    fn user_title_survives_updates_and_time_only_moves_forward() {
        let mut ix = SessionIndex::default();
        let p = Path::new("/p");
        ix.upsert("codex", p, "1", Some("server"), Some(50));
        ix.rename("codex", p, "1", "мой чат");
        ix.upsert("codex", p, "1", Some("server again"), Some(40));
        let e = &ix.list("codex", p, "")[0];
        assert_eq!(e.title, "мой чат");
        assert_eq!(e.last_active, 50);
    }

    #[test]
    fn roundtrip_and_last_open() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("x/s.json");
        let mut ix = SessionIndex::default();
        ix.upsert("claude", Path::new("/p"), "u", None, Some(1));
        ix.set_last_open("claude", Path::new("/p"), "u");
        ix.save_to(&path).unwrap();
        let back = SessionIndex::load_from(&path);
        assert_eq!(back, ix);
        assert_eq!(back.get_last_open("claude", Path::new("/p")).unwrap(), "u");
        assert!(back.get_last_open("claude", Path::new("/q")).is_none());
        assert_eq!(
            SessionIndex::load_from(&d.path().join("none")),
            SessionIndex::default()
        );
    }

    #[test]
    fn titles_and_times() {
        assert_eq!(title_from_message("\n  fix it\nmore"), "fix it");
        assert!(title_from_message(&"я".repeat(99)).ends_with('…'));
        assert_eq!(relative_time(100, 130), "now");
        assert_eq!(relative_time(0, 7200), "2h");
    }
}
