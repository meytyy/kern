//! Commit history and graph lanes. Everything here only reads the repository:
//! no checkout, no index or branch change.
use super::{git, run};
use anyhow::{Result, anyhow};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    Head,
    Branch,
    Remote,
    Tag,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefLabel {
    pub name: String,
    pub kind: RefKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    pub date: String,
    pub subject: String,
    pub refs: Vec<RefLabel>,
    pub is_head: bool,
}

impl Commit {
    pub fn short(&self) -> &str {
        &self.hash[..self.hash.len().min(8)]
    }
}

const FORMAT: &str = "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%ad%x1f%D%x1f%s%x1e";

pub fn parse_log(text: &str) -> Vec<Commit> {
    text.split('\u{1e}')
        .map(|r| r.trim_start_matches(['\n', '\r']))
        .filter(|r| !r.is_empty())
        .filter_map(|record| {
            let f: Vec<&str> = record.split('\u{1f}').collect();
            if f.len() < 7 {
                return None;
            }
            let (refs, is_head) = parse_decorations(f[5]);
            Some(Commit {
                hash: f[0].to_string(),
                parents: f[1].split_whitespace().map(str::to_string).collect(),
                author: f[2].to_string(),
                email: f[3].to_string(),
                date: f[4].to_string(),
                refs,
                is_head,
                subject: f[6].trim_end().to_string(),
            })
        })
        .collect()
}

fn parse_decorations(d: &str) -> (Vec<RefLabel>, bool) {
    let mut out = vec![];
    let mut head = false;
    for part in d.split(", ").map(str::trim).filter(|p| !p.is_empty()) {
        if part == "HEAD" {
            head = true;
            out.push(RefLabel {
                name: "HEAD".into(),
                kind: RefKind::Head,
            });
        } else if let Some(b) = part.strip_prefix("HEAD -> ") {
            head = true;
            out.push(RefLabel {
                name: b.to_string(),
                kind: RefKind::Head,
            });
        } else if let Some(t) = part.strip_prefix("tag: ") {
            out.push(RefLabel {
                name: t.to_string(),
                kind: RefKind::Tag,
            });
        } else if part.contains('/') {
            out.push(RefLabel {
                name: part.to_string(),
                kind: RefKind::Remote,
            });
        } else {
            out.push(RefLabel {
                name: part.to_string(),
                kind: RefKind::Branch,
            });
        }
    }
    (out, head)
}

/// One page of history. An empty repository is an empty page, not an error.
pub fn load_page(
    root: &Path,
    all_branches: bool,
    skip: usize,
    limit: usize,
) -> Result<Vec<Commit>> {
    let mut cmd = git(root);
    cmd.env("GIT_OPTIONAL_LOCKS", "0").args([
        "log",
        "--topo-order",
        "--decorate=short",
        "--date=format:%Y-%m-%d %H:%M",
        FORMAT,
    ]);
    cmd.arg(format!("--skip={skip}"))
        .arg(format!("--max-count={limit}"));
    if all_branches {
        cmd.arg("--all");
    }
    let out = cmd.output().map_err(|e| anyhow!("cannot run git: {e}"))?;
    if out.status.success() {
        return Ok(parse_log(&String::from_utf8_lossy(&out.stdout)));
    }
    let err = String::from_utf8_lossy(&out.stderr);
    if err.contains("does not have any commits")
        || err.contains("bad default revision")
        || err.contains("unknown revision")
    {
        return Ok(vec![]);
    }
    Err(anyhow!("{}", err.trim()))
}

/// The full commit message.
pub fn message(root: &Path, hash: &str) -> Result<String> {
    Ok(run(root, &["show", "--no-patch", "--format=%B", hash])?
        .trim_end()
        .to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitFile {
    pub path: String,
    pub status: char,
}

/// Files changed in `hash` relative to `parent`; `None` means a root commit.
pub fn changed_files(root: &Path, hash: &str, parent: Option<&str>) -> Result<Vec<CommitFile>> {
    let mut args = vec!["diff-tree", "-r", "-z", "--name-status", "--no-renames"];
    match parent {
        Some(p) => args.extend([p, hash]),
        None => args.extend(["--root", "--no-commit-id", hash]),
    }
    let text = run(root, &args)?;
    let mut parts = text.split('\0').filter(|s| !s.is_empty());
    let mut files = vec![];
    while let (Some(st), Some(path)) = (parts.next(), parts.next()) {
        files.push(CommitFile {
            path: path.to_string(),
            status: st.chars().next().unwrap_or('M'),
        });
    }
    Ok(files)
}

/// Patch of one file in a commit, relative to the given parent.
pub fn file_patch(root: &Path, hash: &str, parent: Option<&str>, path: &str) -> Result<String> {
    match parent {
        Some(p) => run(
            root,
            &["diff", "--no-color", "--no-ext-diff", p, hash, "--", path],
        ),
        None => run(root, &["show", "--no-color", "--format=", hash, "--", path]),
    }
}

/// How one commit row is drawn. Columns are lane numbers; colors are lane colors.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GraphRow {
    pub col: usize,
    pub color: usize,
    /// A line arrives from above in the commit's own column.
    pub top: bool,
    /// A line leaves downward in the commit's own column.
    pub bottom: bool,
    /// Lanes that pass by without touching this commit: (col, color).
    pub passing: Vec<(usize, usize)>,
    /// Other lanes that end in this commit (branch points seen from below): (col, color).
    pub merge_in: Vec<(usize, usize)>,
    /// New lanes for extra parents of a merge: (col, color).
    pub fork_out: Vec<(usize, usize)>,
}

/// Lanes carried across pages, so that paging continues the same drawing.
#[derive(Debug, Clone, Default)]
pub struct Lanes {
    /// For each column, the commit it is waiting for, and its color.
    lanes: Vec<Option<(String, usize)>>,
    next_color: usize,
}

impl Lanes {
    fn free_slot(&mut self) -> usize {
        match self.lanes.iter().position(Option::is_none) {
            Some(i) => i,
            None => {
                self.lanes.push(None);
                self.lanes.len() - 1
            }
        }
    }

    fn new_color(&mut self) -> usize {
        self.next_color += 1;
        self.next_color - 1
    }

    pub fn advance(&mut self, commit: &Commit) -> GraphRow {
        let mut row = GraphRow::default();
        let waiting: Vec<usize> = self
            .lanes
            .iter()
            .enumerate()
            .filter(|(_, l)| l.as_ref().is_some_and(|(h, _)| *h == commit.hash))
            .map(|(i, _)| i)
            .collect();
        let (col, color, top) = match waiting.first() {
            Some(&c) => (c, self.lanes[c].as_ref().map_or(0, |l| l.1), true),
            None => {
                let c = self.free_slot();
                (c, self.new_color(), false)
            }
        };
        row.col = col;
        row.color = color;
        row.top = top;
        for &w in waiting.iter().skip(1) {
            let lane_color = self.lanes[w].as_ref().map_or(color, |l| l.1);
            row.merge_in.push((w, lane_color));
            self.lanes[w] = None;
        }
        self.lanes[col] = None;
        for (i, l) in self.lanes.iter().enumerate() {
            if let Some((_, c)) = l {
                row.passing.push((i, *c));
            }
        }
        // The first parent continues this lane unless another lane already waits for it.
        let mut parents = commit.parents.iter();
        if let Some(first) = parents.next() {
            match self
                .lanes
                .iter()
                .position(|l| l.as_ref().is_some_and(|(h, _)| h == first))
            {
                Some(existing) => {
                    let c = self.lanes[existing].as_ref().map_or(color, |l| l.1);
                    row.fork_out.push((existing, c));
                }
                None => {
                    self.lanes[col] = Some((first.clone(), color));
                    row.bottom = true;
                }
            }
        }
        for parent in parents {
            match self
                .lanes
                .iter()
                .position(|l| l.as_ref().is_some_and(|(h, _)| h == parent))
            {
                Some(existing) => {
                    let c = self.lanes[existing].as_ref().map_or(color, |l| l.1);
                    row.fork_out.push((existing, c));
                }
                None => {
                    let slot = self.free_slot();
                    let c = self.new_color();
                    self.lanes[slot] = Some((parent.clone(), c));
                    row.fork_out.push((slot, c));
                }
            }
        }
        while self.lanes.last().is_some_and(Option::is_none) {
            self.lanes.pop();
        }
        row
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn commit(hash: &str, parents: &[&str]) -> Commit {
        Commit {
            hash: hash.into(),
            parents: parents.iter().map(|p| p.to_string()).collect(),
            author: String::new(),
            email: String::new(),
            date: String::new(),
            subject: String::new(),
            refs: vec![],
            is_head: false,
        }
    }

    #[test]
    fn parses_log_records_and_decorations() {
        let raw = "aaa\u{1f}bbb ccc\u{1f}Я\u{1f}a@b\u{1f}2026-01-02 03:04\u{1f}HEAD -> main, tag: v1, origin/main, dev\u{1f}Слияние | веток\u{1e}\nbbb\u{1f}\u{1f}B\u{1f}b@b\u{1f}2026\u{1f}\u{1f}init\u{1e}\n";
        let c = parse_log(raw);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].parents, vec!["bbb", "ccc"]);
        assert!(c[0].is_head);
        assert_eq!(c[0].subject, "Слияние | веток");
        let kinds: Vec<_> = c[0]
            .refs
            .iter()
            .map(|r| (r.name.as_str(), r.kind))
            .collect();
        assert_eq!(
            kinds,
            vec![
                ("main", RefKind::Head),
                ("v1", RefKind::Tag),
                ("origin/main", RefKind::Remote),
                ("dev", RefKind::Branch)
            ]
        );
        assert!(c[1].parents.is_empty() && !c[1].is_head);
    }

    #[test]
    fn linear_history_stays_in_one_lane() {
        let mut l = Lanes::default();
        let a = l.advance(&commit("a", &["b"]));
        let b = l.advance(&commit("b", &["c"]));
        let c = l.advance(&commit("c", &[]));
        assert_eq!((a.col, a.top, a.bottom), (0, false, true));
        assert_eq!((b.col, b.top, b.bottom), (0, true, true));
        assert_eq!((c.col, c.top, c.bottom), (0, true, false));
        assert_eq!(a.color, c.color);
    }

    #[test]
    fn merge_and_branch_point() {
        //  m (merge of a and x)
        //  |\
        //  a x
        //  |/
        //  b
        let mut l = Lanes::default();
        let m = l.advance(&commit("m", &["a", "x"]));
        assert_eq!(m.col, 0);
        assert_eq!(m.fork_out.len(), 1);
        let (fork_col, fork_color) = m.fork_out[0];
        assert_eq!(fork_col, 1);
        assert_ne!(fork_color, m.color);

        let a = l.advance(&commit("a", &["b"]));
        assert_eq!(a.col, 0);
        assert_eq!(a.passing, vec![(1, fork_color)]);

        let x = l.advance(&commit("x", &["b"]));
        assert_eq!(x.col, 1);
        // Lane 0 already waits for b: x joins it instead of opening a new lane.
        assert_eq!(x.fork_out, vec![(0, m.color)]);
        assert!(!x.bottom);

        let b = l.advance(&commit("b", &[]));
        assert_eq!((b.col, b.top), (0, true));
        assert!(b.passing.is_empty());
    }

    #[test]
    fn parallel_tips_get_separate_lanes_and_paging_continues() {
        let mut l = Lanes::default();
        let t1 = l.advance(&commit("t1", &["b"]));
        let t2 = l.advance(&commit("t2", &["c"]));
        assert_eq!((t1.col, t2.col), (0, 1));
        assert_ne!(t1.color, t2.color);
        // A later page keeps using the same state.
        let mut page2 = l.clone();
        let b = page2.advance(&commit("b", &["d"]));
        assert_eq!(b.col, 0);
        assert_eq!(b.color, t1.color);
    }

    fn sh(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_AUTHOR_DATE", "2026-01-01T10:00:00")
            .env("GIT_COMMITTER_DATE", "2026-01-01T10:00:00")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn write_commit(dir: &Path, file: &str, text: &str, msg: &str) {
        std::fs::write(dir.join(file), text).unwrap();
        sh(dir, &["add", file]);
        sh(dir, &["commit", "-qm", msg]);
    }

    #[test]
    fn real_repository_with_fork_merge_and_tag_read_only() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        sh(root, &["init", "-q", "-b", "main"]);
        sh(root, &["config", "user.email", "t@e"]);
        sh(root, &["config", "user.name", "T"]);
        write_commit(root, "a.txt", "1\n", "первый");
        sh(root, &["switch", "-qc", "dev"]);
        write_commit(root, "b.txt", "dev\n", "on dev");
        sh(root, &["switch", "-q", "main"]);
        write_commit(root, "a.txt", "2\n", "on main");
        sh(root, &["merge", "-q", "--no-ff", "dev", "-m", "merge dev"]);
        sh(root, &["tag", "v1"]);
        // Staged and unstaged work must survive looking at history.
        std::fs::write(root.join("a.txt"), "staged\n").unwrap();
        sh(root, &["add", "a.txt"]);
        std::fs::write(root.join("a.txt"), "unstaged\n").unwrap();
        let before = Command::new("git")
            .current_dir(root)
            .args(["status", "--porcelain", "--branch"])
            .output()
            .unwrap()
            .stdout;

        let log = load_page(root, false, 0, 100).unwrap();
        assert_eq!(log.len(), 4);
        assert_eq!(log[0].subject, "merge dev");
        assert_eq!(log[0].parents.len(), 2);
        assert!(log[0].is_head);
        assert!(
            log[0]
                .refs
                .iter()
                .any(|r| r.name == "v1" && r.kind == RefKind::Tag)
        );

        // Paging.
        assert_eq!(load_page(root, false, 1, 2).unwrap().len(), 2);
        assert_eq!(load_page(root, false, 4, 2).unwrap().len(), 0);

        // Layout draws a real fork.
        let mut lanes = Lanes::default();
        let rows: Vec<_> = log.iter().map(|c| lanes.advance(c)).collect();
        assert!(rows[0].fork_out.iter().any(|(c, _)| *c == 1), "{rows:?}");

        // A merge diff depends on the parent chosen.
        let merge = &log[0];
        let vs_first = changed_files(root, &merge.hash, Some(&merge.parents[0])).unwrap();
        assert_eq!(
            vs_first,
            vec![CommitFile {
                path: "b.txt".into(),
                status: 'A'
            }]
        );
        let vs_second = changed_files(root, &merge.hash, Some(&merge.parents[1])).unwrap();
        assert_eq!(
            vs_second,
            vec![CommitFile {
                path: "a.txt".into(),
                status: 'M'
            }]
        );
        assert!(
            file_patch(root, &merge.hash, Some(&merge.parents[0]), "b.txt")
                .unwrap()
                .contains("+dev")
        );

        // Root commit.
        let first = log.last().unwrap();
        assert!(first.parents.is_empty());
        assert_eq!(
            changed_files(root, &first.hash, None).unwrap()[0].path,
            "a.txt"
        );
        assert!(
            file_patch(root, &first.hash, None, "a.txt")
                .unwrap()
                .contains("+1")
        );
        assert_eq!(message(root, &first.hash).unwrap(), "первый");

        // Nothing changed in the working tree, index or branch.
        let after = Command::new("git")
            .current_dir(root)
            .args(["status", "--porcelain", "--branch"])
            .output()
            .unwrap()
            .stdout;
        assert_eq!(before, after);
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "unstaged\n"
        );

        // All branches vs current.
        sh(root, &["branch", "side", &log[2].hash]);
        sh(root, &["switch", "-q", "side"]);
        sh(root, &["stash", "-q"]);
        write_commit(root, "c.txt", "x\n", "on side");
        sh(root, &["switch", "-q", "main"]);
        sh(root, &["stash", "pop", "-q"]);
        let current = load_page(root, false, 0, 100).unwrap();
        let all = load_page(root, true, 0, 100).unwrap();
        assert!(all.len() > current.len());
    }

    #[test]
    fn empty_repository_is_an_empty_page() {
        let d = tempfile::tempdir().unwrap();
        sh(d.path(), &["init", "-q"]);
        assert!(load_page(d.path(), false, 0, 10).unwrap().is_empty());
    }
}
