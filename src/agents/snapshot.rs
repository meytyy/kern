//! Working-tree snapshots that make Review Diff trustworthy.
//!
//! Snapshots live in a private Git directory under KERN's data folder, so the
//! project's own repository, index and history are never touched, and projects
//! without Git work too. A snapshot is a tree object of all non-ignored files.
use crate::platform::tools;
use anyhow::{Context as _, Result, anyhow};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone)]
pub struct SnapshotStore {
    git_dir: PathBuf,
    project: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    /// A, M, D, T.
    pub status: char,
}

/// What KERN knows about one file changed during an agent turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attribution {
    /// Reported by the agent, untouched by the user: safe to revert.
    Agent,
    /// Overlaps with user edits; needs a human decision.
    Conflict(String),
    /// Changed during the turn but cannot be tied to the agent reliably
    /// (e.g. via a shell command, or from another program): diff only.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewedFile {
    pub path: String,
    pub status: char,
    pub attribution: Attribution,
}

#[derive(Debug, Clone, Default)]
pub struct TurnObservations {
    /// Paths reported through `fileChange` items.
    pub agent_paths: BTreeSet<String>,
    /// Paths with unsaved editor changes at the start or end of the turn.
    pub dirty_paths: BTreeSet<String>,
    /// Paths the user saved from the editor while the turn was running.
    pub user_saved_paths: BTreeSet<String>,
}

pub fn classify(changed: &[ChangedFile], obs: &TurnObservations) -> Vec<ReviewedFile> {
    changed
        .iter()
        .map(|c| ReviewedFile {
            path: c.path.clone(),
            status: c.status,
            attribution: if obs.dirty_paths.contains(&c.path) {
                Attribution::Conflict("the editor has unsaved changes to this file".into())
            } else if obs.user_saved_paths.contains(&c.path) {
                Attribution::Conflict("you saved this file while the agent was working".into())
            } else if obs.agent_paths.contains(&c.path) {
                Attribution::Agent
            } else {
                Attribution::Unknown
            },
        })
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
pub enum RevertOutcome {
    Reverted,
    /// The file changed after the turn ended; nothing was written.
    ChangedSince,
}

const EXCLUDES: &str =
    "node_modules/\ntarget/\n.venv/\nvenv/\n__pycache__/\n.next/\ndist/\nbuild/\n.DS_Store\n";

impl SnapshotStore {
    pub fn open(project: &Path) -> Result<Self> {
        let key = crate::core::textfile::hash_bytes(project.to_string_lossy().as_bytes());
        let dir = crate::core::settings::config_dir()
            .join("snapshots")
            .join(format!("{key:016x}"));
        Self::open_at(project, &dir)
    }

    pub fn open_at(project: &Path, git_dir: &Path) -> Result<Self> {
        let store = Self {
            git_dir: git_dir.to_path_buf(),
            project: project.to_path_buf(),
        };
        if !git_dir.join("HEAD").exists() {
            std::fs::create_dir_all(git_dir)?;
            let init = tools::command("git")
                .args(["init", "-q", "--bare"])
                .arg(git_dir)
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .stdin(Stdio::null())
                .output()
                .context("cannot run git")?;
            if !init.status.success() {
                return Err(anyhow!("{}", String::from_utf8_lossy(&init.stderr).trim()));
            }
            store.git(&["config", "core.bare", "false"])?;
            store.git(&["config", "core.autocrlf", "false"])?;
            store.git(&["config", "core.safecrlf", "false"])?;
            store.git(&["config", "core.quotepath", "false"])?;
            store.git(&["config", "gc.auto", "0"])?;
            let info = git_dir.join("info");
            std::fs::create_dir_all(&info)?;
            std::fs::write(info.join("exclude"), EXCLUDES)?;
        }
        Ok(store)
    }

    fn cmd(&self) -> Command {
        let mut c = tools::command("git");
        c.env("GIT_DIR", &self.git_dir)
            .env("GIT_WORK_TREE", &self.project)
            .env("LC_ALL", "C")
            .current_dir(&self.project)
            .stdin(Stdio::null());
        // A user's hooks or templates have no business here.
        c.env_remove("GIT_INDEX_FILE");
        c
    }

    fn git(&self, args: &[&str]) -> Result<Vec<u8>> {
        let out = self.cmd().args(args).output().context("cannot run git")?;
        if out.status.success() {
            Ok(out.stdout)
        } else {
            Err(anyhow!("{}", String::from_utf8_lossy(&out.stderr).trim()))
        }
    }

    /// Record the working tree. Returns the tree id.
    pub fn snapshot(&self) -> Result<String> {
        // A turn's closing snapshot can meet the next message's opening one, and
        // both agents share this store: only one may hold git's index at a time.
        static ONE_AT_A_TIME: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
        let _guard = ONE_AT_A_TIME.lock();
        // Under the lock, a lock file here can only be left over from a crash.
        let _ = std::fs::remove_file(self.git_dir.join("index.lock"));
        self.git(&["add", "-A", "--ignore-errors", "--", "."])?;
        Ok(String::from_utf8_lossy(&self.git(&["write-tree"])?)
            .trim()
            .to_string())
    }

    pub fn changed_files(&self, before: &str, after: &str) -> Result<Vec<ChangedFile>> {
        let out = self.git(&[
            "diff-tree",
            "-r",
            "-z",
            "--no-renames",
            "--name-status",
            before,
            after,
        ])?;
        let text = String::from_utf8_lossy(&out).to_string();
        let mut parts = text.split('\0').filter(|s| !s.is_empty());
        let mut files = vec![];
        while let (Some(st), Some(path)) = (parts.next(), parts.next()) {
            files.push(ChangedFile {
                path: path.to_string(),
                status: st.chars().next().unwrap_or('M'),
            });
        }
        Ok(files)
    }

    /// Unified diff between two snapshots, optionally for one path.
    pub fn patch(&self, before: &str, after: &str, path: Option<&str>) -> Result<String> {
        let mut args = vec![
            "diff-tree",
            "-r",
            "-p",
            "--no-renames",
            "--no-color",
            before,
            after,
        ];
        if let Some(p) = path {
            args.extend(["--", p]);
        }
        Ok(String::from_utf8_lossy(&self.git(&args)?).to_string())
    }

    pub fn file_at(&self, tree: &str, path: &str) -> Result<Option<Vec<u8>>> {
        let spec = format!("{tree}:{path}");
        let out = self
            .cmd()
            .args(["cat-file", "blob", &spec])
            .output()
            .context("cannot run git")?;
        Ok(out.status.success().then_some(out.stdout))
    }

    /// Restore one file to its state before the turn, but only if it is still
    /// exactly as the agent left it. Otherwise the user's newer work would be lost.
    pub fn revert_file(&self, before: &str, after: &str, path: &str) -> Result<RevertOutcome> {
        if path.split('/').any(|c| c == "..") {
            return Err(anyhow!("invalid path"));
        }
        let full = self.project.join(path);
        let current = std::fs::read(&full).ok();
        if current != self.file_at(after, path)? {
            return Ok(RevertOutcome::ChangedSince);
        }
        match self.file_at(before, path)? {
            Some(bytes) => {
                if let Some(p) = full.parent() {
                    std::fs::create_dir_all(p)?;
                }
                crate::core::document::atomic_write(&full, &bytes)?;
            }
            None => std::fs::remove_file(&full)?,
        }
        Ok(RevertOutcome::Reverted)
    }
}

/// Paths in `fileChange` items may be absolute or relative to the project.
pub fn relative_agent_path(path: &str, project: &Path) -> String {
    let p = Path::new(path);
    p.strip_prefix(project)
        .map(|r| r.to_string_lossy().to_string())
        .unwrap_or_else(|_| {
            // macOS resolves /tmp and /var through /private.
            let canon = std::fs::canonicalize(project).unwrap_or_else(|_| project.to_path_buf());
            p.strip_prefix(&canon)
                .map(|r| r.to_string_lossy().to_string())
                .unwrap_or_else(|_| path.to_string())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_snapshots_do_not_collide() {
        let d = tempfile::tempdir().unwrap();
        let (project, store_dir) = (d.path().join("p"), d.path().join("s"));
        std::fs::create_dir_all(&project).unwrap();
        for i in 0..50 {
            std::fs::write(project.join(format!("f{i}.txt")), "x").unwrap();
        }
        let store = SnapshotStore::open_at(&project, &store_dir).unwrap();
        // What a crash leaves behind must not block the next snapshot.
        std::fs::write(store_dir.join("index.lock"), "").unwrap();
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let s = store.clone();
                std::thread::spawn(move || s.snapshot())
            })
            .collect();
        let trees: Vec<String> = handles
            .into_iter()
            .map(|h| h.join().unwrap().unwrap())
            .collect();
        assert!(trees.iter().all(|t| *t == trees[0]));
    }

    fn setup() -> (tempfile::TempDir, tempfile::TempDir, SnapshotStore) {
        let project = tempfile::Builder::new()
            .prefix("проект ")
            .tempdir()
            .unwrap();
        let store_dir = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("a.txt"), "one\n").unwrap();
        std::fs::write(project.path().join("b файл.txt"), "b\n").unwrap();
        let store = SnapshotStore::open_at(project.path(), &store_dir.path().join("s")).unwrap();
        (project, store_dir, store)
    }

    #[test]
    fn detects_add_modify_delete_without_touching_project_git() {
        let (project, _s, store) = setup();
        let root = project.path();
        Command::new("git")
            .current_dir(root)
            .args(["init", "-q"])
            .output()
            .unwrap();
        Command::new("git")
            .current_dir(root)
            .args(["add", "a.txt"])
            .output()
            .unwrap();
        let status_before = Command::new("git")
            .current_dir(root)
            .args(["status", "--porcelain"])
            .output()
            .unwrap()
            .stdout;

        let before = store.snapshot().unwrap();
        std::fs::write(root.join("a.txt"), "two\n").unwrap();
        std::fs::write(root.join("c.txt"), "new\n").unwrap();
        std::fs::remove_file(root.join("b файл.txt")).unwrap();
        let after = store.snapshot().unwrap();

        let mut changed = store.changed_files(&before, &after).unwrap();
        changed.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(
            changed
                .iter()
                .map(|c| (c.path.as_str(), c.status))
                .collect::<Vec<_>>(),
            vec![("a.txt", 'M'), ("b файл.txt", 'D'), ("c.txt", 'A')]
        );
        assert!(
            store
                .patch(&before, &after, Some("a.txt"))
                .unwrap()
                .contains("+two")
        );

        // Project's index unchanged apart from the worktree edits.
        let status_after = Command::new("git")
            .current_dir(root)
            .args(["status", "--porcelain"])
            .output()
            .unwrap()
            .stdout;
        assert_ne!(status_before, status_after);
        assert!(
            String::from_utf8_lossy(&status_after).contains("AM a.txt"),
            "{}",
            String::from_utf8_lossy(&status_after)
        );
    }

    #[test]
    fn revert_restores_modified_added_and_deleted_files() {
        let (project, _s, store) = setup();
        let root = project.path();
        let before = store.snapshot().unwrap();
        std::fs::write(root.join("a.txt"), "agent\n").unwrap();
        std::fs::write(root.join("new.txt"), "agent\n").unwrap();
        std::fs::remove_file(root.join("b файл.txt")).unwrap();
        let after = store.snapshot().unwrap();

        assert_eq!(
            store.revert_file(&before, &after, "a.txt").unwrap(),
            RevertOutcome::Reverted
        );
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "one\n"
        );
        assert_eq!(
            store.revert_file(&before, &after, "new.txt").unwrap(),
            RevertOutcome::Reverted
        );
        assert!(!root.join("new.txt").exists());
        assert_eq!(
            store.revert_file(&before, &after, "b файл.txt").unwrap(),
            RevertOutcome::Reverted
        );
        assert_eq!(
            std::fs::read_to_string(root.join("b файл.txt")).unwrap(),
            "b\n"
        );
    }

    #[test]
    fn revert_never_overwrites_later_user_edits() {
        let (project, _s, store) = setup();
        let root = project.path();
        let before = store.snapshot().unwrap();
        std::fs::write(root.join("a.txt"), "agent\n").unwrap();
        let after = store.snapshot().unwrap();
        std::fs::write(root.join("a.txt"), "agent\nuser edit\n").unwrap();
        assert_eq!(
            store.revert_file(&before, &after, "a.txt").unwrap(),
            RevertOutcome::ChangedSince
        );
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "agent\nuser edit\n"
        );
        // Deleted by the user after the turn: also refused.
        std::fs::remove_file(root.join("a.txt")).unwrap();
        assert_eq!(
            store.revert_file(&before, &after, "a.txt").unwrap(),
            RevertOutcome::ChangedSince
        );
        assert!(!root.join("a.txt").exists());
        assert!(store.revert_file(&before, &after, "../x").is_err());
    }

    #[test]
    fn respects_ignores_and_exact_bytes() {
        let (project, _s, store) = setup();
        let root = project.path();
        std::fs::write(root.join(".gitignore"), ".env\n").unwrap();
        std::fs::write(root.join(".env"), "SECRET=1\n").unwrap();
        std::fs::create_dir_all(root.join("node_modules/x")).unwrap();
        std::fs::write(root.join("node_modules/x/i.js"), "x").unwrap();
        std::fs::write(root.join("crlf.txt"), "a\r\nb\r\n").unwrap();
        let tree = store.snapshot().unwrap();
        assert_eq!(store.file_at(&tree, ".env").unwrap(), None);
        assert_eq!(store.file_at(&tree, "node_modules/x/i.js").unwrap(), None);
        assert_eq!(
            store.file_at(&tree, "crlf.txt").unwrap().unwrap(),
            b"a\r\nb\r\n"
        );
    }

    #[test]
    fn attribution_rules() {
        let changed = vec![
            ChangedFile {
                path: "agent.ts".into(),
                status: 'M',
            },
            ChangedFile {
                path: "dirty.ts".into(),
                status: 'M',
            },
            ChangedFile {
                path: "saved.ts".into(),
                status: 'M',
            },
            ChangedFile {
                path: "shell.ts".into(),
                status: 'M',
            },
        ];
        let mut obs = TurnObservations::default();
        for p in ["agent.ts", "dirty.ts", "saved.ts"] {
            obs.agent_paths.insert(p.into());
        }
        obs.dirty_paths.insert("dirty.ts".into());
        obs.user_saved_paths.insert("saved.ts".into());
        let r = classify(&changed, &obs);
        assert_eq!(r[0].attribution, Attribution::Agent);
        assert!(matches!(r[1].attribution, Attribution::Conflict(_)));
        assert!(matches!(r[2].attribution, Attribution::Conflict(_)));
        assert_eq!(r[3].attribution, Attribution::Unknown);
    }

    #[test]
    fn agent_paths_relative() {
        assert_eq!(
            relative_agent_path("/p/src/a.ts", Path::new("/p")),
            "src/a.ts"
        );
        assert_eq!(relative_agent_path("src/a.ts", Path::new("/p")), "src/a.ts");
    }
}
