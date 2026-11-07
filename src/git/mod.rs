//! Local Git through the installed CLI. Nothing here needs a GitHub connection.
pub mod graph;
pub mod status;

use crate::platform::tools;
use anyhow::{Result, anyhow};
pub use status::{FileChange, FileState, RepoStatus};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn git(cwd: &Path) -> Command {
    let mut cmd = tools::command("git");
    // Reuse the official GitHub CLI login if present, after any configured helper.
    // Applies to github.com only; other remotes and plain local Git are unaffected.
    if let Some(gh) = tools::find("gh") {
        cmd.arg("-c").arg(format!(
            "credential.https://github.com.helper=!\"{}\" auth git-credential",
            gh.display()
        ));
    }
    cmd.current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .env("GIT_PAGER", "cat")
        .stdin(Stdio::null());
    cmd
}

fn run(cwd: &Path, args: &[&str]) -> Result<String> {
    let out = git(cwd)
        .args(args)
        .output()
        .map_err(|e| anyhow!("cannot run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let so = String::from_utf8_lossy(&out.stdout);
        let msg = if err.trim().is_empty() {
            so.trim()
        } else {
            err.trim()
        };
        Err(anyhow!("{}", clean_error(msg)))
    }
}

fn clean_error(msg: &str) -> String {
    msg.lines()
        .filter(|l| !l.trim().is_empty())
        .take(12)
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn is_repo(path: &Path) -> bool {
    run(path, &["rev-parse", "--is-inside-work-tree"])
        .map(|s| s.trim() == "true")
        .unwrap_or(false)
}

pub fn repo_root(path: &Path) -> Option<PathBuf> {
    run(path, &["rev-parse", "--show-toplevel"])
        .ok()
        .map(|s| PathBuf::from(s.trim_end_matches('\n')))
}

pub fn status(root: &Path) -> Result<RepoStatus> {
    let out = git(root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args([
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=all",
        ])
        .output()
        .map_err(|e| anyhow!("cannot run git: {e}"))?;
    if !out.status.success() {
        return Err(anyhow!(
            "{}",
            clean_error(&String::from_utf8_lossy(&out.stderr))
        ));
    }
    let mut st = status::parse_porcelain_v2(&out.stdout)?;
    let git_dir = run(root, &["rev-parse", "--git-dir"]).unwrap_or_default();
    let git_dir = root.join(git_dir.trim());
    st.merging = git_dir.join("MERGE_HEAD").exists();
    st.rebasing = git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists();
    Ok(st)
}

/// Unified diff of one file. `staged` selects index vs. HEAD; otherwise worktree vs. index.
pub fn diff_file(root: &Path, path: &str, staged: bool, untracked: bool) -> Result<String> {
    if untracked {
        // `git diff --no-index` exits with 1 when files differ.
        let out = git(root)
            .args(["diff", "--no-color", "--no-index", "--", "/dev/null", path])
            .output()
            .map_err(|e| anyhow!("cannot run git: {e}"))?;
        return Ok(String::from_utf8_lossy(&out.stdout).to_string());
    }
    let mut args = vec!["diff", "--no-color", "--no-ext-diff"];
    if staged {
        args.push("--cached");
    }
    args.extend(["--", path]);
    run(root, &args)
}

pub fn stage(root: &Path, path: &str) -> Result<()> {
    run(root, &["add", "--", path]).map(|_| ())
}

pub fn unstage(root: &Path, path: &str) -> Result<()> {
    let has_head = run(root, &["rev-parse", "--verify", "-q", "HEAD"]).is_ok();
    if has_head {
        run(root, &["restore", "--staged", "--", path]).map(|_| ())
    } else {
        run(root, &["rm", "--cached", "-q", "--", path]).map(|_| ())
    }
}

/// Commit what is staged. Returns the new short id.
pub fn commit(root: &Path, message: &str) -> Result<String> {
    if message.trim().is_empty() {
        return Err(anyhow!("commit message is empty"));
    }
    let mut child = git(root)
        .args(["commit", "-F", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow!("cannot run git: {e}"))?;
    {
        use std::io::Write;
        child.stdin.take().unwrap().write_all(message.as_bytes())?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        let so = String::from_utf8_lossy(&out.stdout);
        let se = String::from_utf8_lossy(&out.stderr);
        return Err(anyhow!(
            "{}",
            clean_error(if se.trim().is_empty() { &so } else { &se })
        ));
    }
    Ok(run(root, &["rev-parse", "--short", "HEAD"])?
        .trim()
        .to_string())
}

pub fn branches(root: &Path) -> Result<Vec<String>> {
    Ok(run(
        root,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
    )?
    .lines()
    .map(str::to_string)
    .collect())
}

/// Switch branches. Git itself refuses when local changes would be overwritten.
pub fn switch_branch(root: &Path, name: &str, create: bool) -> Result<()> {
    if name.starts_with('-') {
        return Err(anyhow!("invalid branch name"));
    }
    let args: Vec<&str> = if create {
        vec!["switch", "-c", name]
    } else {
        vec!["switch", name]
    };
    run(root, &args).map(|_| ())
}

/// Pull. Local changes are never discarded: Git aborts if they would be overwritten.
pub fn pull(root: &Path) -> Result<String> {
    run(root, &["pull", "--no-edit"])
}

pub fn fetch(root: &Path) -> Result<String> {
    run(root, &["fetch", "--all", "--prune", "--tags"])
}

/// Changes whenever a ref moves, so the graph knows when to reload.
pub fn refs_fingerprint(root: &Path) -> String {
    let refs =
        run(root, &["for-each-ref", "--format=%(objectname) %(refname)"]).unwrap_or_default();
    let head = run(root, &["rev-parse", "HEAD"]).unwrap_or_default();
    format!(
        "{:x}",
        crate::core::textfile::hash_bytes(format!("{head}{refs}").as_bytes())
    )
}

pub fn has_upstream(root: &Path) -> bool {
    run(
        root,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
    )
    .is_ok()
}

pub fn push(root: &Path) -> Result<String> {
    let out = if has_upstream(root) {
        git(root).args(["push"]).output()
    } else {
        git(root).args(["push", "-u", "origin", "HEAD"]).output()
    }
    .map_err(|e| anyhow!("cannot run git: {e}"))?;
    let text = String::from_utf8_lossy(&out.stderr).to_string();
    if out.status.success() {
        Ok(text)
    } else {
        Err(anyhow!("{}", clean_error(&text)))
    }
}

pub fn remote_url(root: &Path, name: &str) -> Option<String> {
    run(root, &["remote", "get-url", name])
        .ok()
        .map(|s| s.trim().to_string())
}

/// Clone by URL with progress lines. Does not depend on a GitHub connection.
pub fn clone(url: &str, dest: &Path, mut progress: impl FnMut(String)) -> Result<()> {
    if url.starts_with('-') {
        return Err(anyhow!("invalid repository URL"));
    }
    if dest.exists()
        && std::fs::read_dir(dest)
            .map(|mut d| d.next().is_some())
            .unwrap_or(true)
    {
        return Err(anyhow!("destination is not empty: {}", dest.display()));
    }
    let parent = dest
        .parent()
        .ok_or_else(|| anyhow!("invalid destination"))?;
    std::fs::create_dir_all(parent)?;
    let mut child = git(parent)
        .args(["clone", "--progress", "--", url])
        .arg(dest)
        .stderr(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .map_err(|e| anyhow!("cannot run git: {e}"))?;
    let mut tail = Vec::new();
    if let Some(stderr) = child.stderr.take() {
        read_progress(stderr, |line| {
            tail.push(line.clone());
            progress(line);
        });
    }
    let st = child.wait()?;
    if st.success() {
        Ok(())
    } else {
        Err(anyhow!("{}", clean_error(&tail.join("\n"))))
    }
}

fn read_progress(stream: impl Read, mut f: impl FnMut(String)) {
    // Progress uses \r between updates, so split on both.
    let mut reader = BufReader::new(stream);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let Ok(avail) = reader.fill_buf() else { break };
        if avail.is_empty() {
            break;
        }
        let n = avail
            .iter()
            .position(|b| *b == b'\r' || *b == b'\n')
            .map_or(avail.len(), |i| i + 1);
        buf.extend_from_slice(&avail[..n]);
        reader.consume(n);
        let line = String::from_utf8_lossy(&buf).trim().to_string();
        if !line.is_empty() {
            f(line);
        }
    }
}

/// Turn a remote URL into `owner/repo` for github.com remotes.
pub fn github_slug(remote: &str) -> Option<String> {
    let r = remote.trim().trim_end_matches('/').trim_end_matches(".git");
    let rest = r
        .strip_prefix("git@github.com:")
        .or_else(|| r.strip_prefix("ssh://git@github.com/"))
        .or_else(|| r.strip_prefix("https://github.com/"))
        .or_else(|| r.strip_prefix("http://github.com/"))
        .or_else(|| r.strip_prefix("git://github.com/"))
        .or_else(|| r.split_once("@github.com/").map(|(_, tail)| tail))?;
    let mut it = rest.split('/');
    let (owner, repo) = (it.next()?, it.next()?);
    (!owner.is_empty() && !repo.is_empty() && it.next().is_none())
        .then(|| format!("{owner}/{repo}"))
}

/// Browser URL of the "create pull request" page for a branch.
pub fn pull_request_url(remote: &str, branch: &str) -> Option<String> {
    let slug = github_slug(remote)?;
    let encoded = url::form_urlencoded::byte_serialize(branch.as_bytes())
        .collect::<String>()
        .replace("%2F", "/");
    Some(format!(
        "https://github.com/{slug}/compare/{encoded}?expand=1"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn repo() -> tempfile::TempDir {
        let d = tempfile::Builder::new()
            .prefix("kern проект ")
            .tempdir()
            .unwrap();
        sh(d.path(), &["init", "-q", "-b", "main"]);
        sh(d.path(), &["config", "user.email", "t@example.com"]);
        sh(d.path(), &["config", "user.name", "T"]);
        std::fs::write(d.path().join("a b.txt"), "one\n").unwrap();
        sh(d.path(), &["add", "."]);
        sh(d.path(), &["commit", "-qm", "init"]);
        d
    }

    #[test]
    fn status_stage_commit_flow_with_unicode_paths() {
        let d = repo();
        let root = d.path();
        std::fs::write(root.join("a b.txt"), "two\n").unwrap();
        std::fs::write(root.join("новый файл.ts"), "x\n").unwrap();
        let st = status(root).unwrap();
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!(st.files.len(), 2);
        let f = st.files.iter().find(|f| f.path == "a b.txt").unwrap();
        assert!(f.unstaged.is_some() && f.staged.is_none());
        let u = st.files.iter().find(|f| f.path == "новый файл.ts").unwrap();
        assert!(u.untracked);

        let diff = diff_file(root, "a b.txt", false, false).unwrap();
        assert!(diff.contains("-one") && diff.contains("+two"));
        let diff = diff_file(root, "новый файл.ts", false, true).unwrap();
        assert!(diff.contains("+x"));

        stage(root, "a b.txt").unwrap();
        let st = status(root).unwrap();
        assert!(
            st.files
                .iter()
                .find(|f| f.path == "a b.txt")
                .unwrap()
                .staged
                .is_some()
        );
        assert!(
            diff_file(root, "a b.txt", true, false)
                .unwrap()
                .contains("+two")
        );
        unstage(root, "a b.txt").unwrap();
        assert!(
            status(root)
                .unwrap()
                .files
                .iter()
                .find(|f| f.path == "a b.txt")
                .unwrap()
                .staged
                .is_none()
        );

        stage(root, "новый файл.ts").unwrap();
        commit(root, "добавить файл").unwrap();
        let st = status(root).unwrap();
        // Only the selected file is committed; the other edit is preserved.
        assert_eq!(st.files.len(), 1);
        assert_eq!(
            std::fs::read_to_string(root.join("a b.txt")).unwrap(),
            "two\n"
        );
        assert!(commit(root, "  ").is_err());
    }

    #[test]
    fn unstage_works_before_first_commit() {
        let d = tempfile::tempdir().unwrap();
        sh(d.path(), &["init", "-q"]);
        std::fs::write(d.path().join("f"), "x").unwrap();
        stage(d.path(), "f").unwrap();
        unstage(d.path(), "f").unwrap();
        assert!(status(d.path()).unwrap().files[0].untracked);
    }

    #[test]
    fn switch_refuses_to_lose_local_changes() {
        let d = repo();
        let root = d.path();
        switch_branch(root, "feature", true).unwrap();
        std::fs::write(root.join("a b.txt"), "feature\n").unwrap();
        stage(root, "a b.txt").unwrap();
        commit(root, "f").unwrap();
        std::fs::write(root.join("a b.txt"), "uncommitted\n").unwrap();
        assert!(switch_branch(root, "main", false).is_err());
        assert_eq!(
            std::fs::read_to_string(root.join("a b.txt")).unwrap(),
            "uncommitted\n"
        );
        assert_eq!(status(root).unwrap().branch.as_deref(), Some("feature"));
        assert_eq!(branches(root).unwrap().len(), 2);
    }

    #[test]
    fn clone_push_pull_with_local_remote_and_conflicts() {
        let bare = tempfile::tempdir().unwrap();
        sh(bare.path(), &["init", "-q", "--bare", "-b", "main"]);
        let url = bare.path().to_string_lossy().to_string();

        let d = repo();
        let root = d.path();
        sh(root, &["remote", "add", "origin", &url]);
        assert!(!has_upstream(root));
        push(root).unwrap();
        assert!(has_upstream(root));

        let parent = tempfile::tempdir().unwrap();
        let dest = parent.path().join("клон с пробелом");
        let mut lines = 0;
        clone(&url, &dest, |_| lines += 1).unwrap();
        assert!(lines > 0);
        sh(&dest, &["config", "user.email", "t@example.com"]);
        sh(&dest, &["config", "user.name", "T"]);
        assert_eq!(
            std::fs::read_to_string(dest.join("a b.txt")).unwrap(),
            "one\n"
        );
        assert!(
            clone(&url, &dest, |_| {}).is_err(),
            "refuses a non-empty destination"
        );

        // Upstream moves on.
        std::fs::write(root.join("a b.txt"), "upstream\n").unwrap();
        stage(root, "a b.txt").unwrap();
        commit(root, "up").unwrap();
        push(root).unwrap();

        // Uncommitted local edit on the same file survives a failed pull.
        std::fs::write(dest.join("a b.txt"), "local\n").unwrap();
        assert!(pull(&dest).is_err());
        assert_eq!(
            std::fs::read_to_string(dest.join("a b.txt")).unwrap(),
            "local\n"
        );

        // Committed divergence becomes a visible merge conflict.
        stage(&dest, "a b.txt").unwrap();
        commit(&dest, "local").unwrap();
        sh(&dest, &["config", "pull.rebase", "false"]);
        assert!(pull(&dest).is_err());
        let st = status(&dest).unwrap();
        assert!(st.merging);
        assert!(st.files.iter().any(|f| f.state == FileState::Conflicted));
    }

    #[test]
    fn clone_failure_gives_readable_error() {
        let parent = tempfile::tempdir().unwrap();
        let err = clone("/nonexistent/repo", &parent.path().join("x"), |_| {}).unwrap_err();
        assert!(
            err.to_string().contains("does not exist") || err.to_string().contains("not"),
            "{err}"
        );
    }

    #[test]
    fn github_urls() {
        for r in [
            "https://github.com/o/r.git",
            "git@github.com:o/r.git",
            "ssh://git@github.com/o/r",
            "https://github.com/o/r/",
            "https://user@github.com/o/r",
        ] {
            assert_eq!(github_slug(r).as_deref(), Some("o/r"), "{r}");
        }
        assert_eq!(github_slug("https://gitlab.com/o/r"), None);
        assert_eq!(
            pull_request_url("git@github.com:o/r.git", "feat/новая").unwrap(),
            "https://github.com/o/r/compare/feat/%D0%BD%D0%BE%D0%B2%D0%B0%D1%8F?expand=1"
        );
    }
}
