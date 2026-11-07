//! Lazy directory listing for the file tree.
use anyhow::{Result, anyhow};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub ignored: bool,
}

/// Directory listing, directories first. `.git` is hidden, other ignored paths
/// remain visible (dimmed) because the user may need to look at them.
pub fn list_dir(project: &Path, dir: &Path) -> Result<Vec<Entry>> {
    let ignored = ignored_set(project, dir);
    let mut out = Vec::new();
    for item in std::fs::read_dir(dir)? {
        let item = item?;
        let name = item.file_name().to_string_lossy().to_string();
        if name == ".git" || name == ".DS_Store" {
            continue;
        }
        let path = item.path();
        let is_dir = std::fs::metadata(&path)
            .map(|m| m.is_dir())
            .unwrap_or(false);
        let is_ignored = ignored.contains(&name)
            || (is_dir && matches!(name.as_str(), "node_modules" | "target"));
        out.push(Entry {
            path,
            name,
            is_dir,
            ignored: is_ignored,
        });
    }
    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(out)
}

fn ignored_set(project: &Path, dir: &Path) -> std::collections::HashSet<String> {
    // Everything the walker would visit is not ignored: the complement is.
    let mut visible = std::collections::HashSet::new();
    let walker = ignore::WalkBuilder::new(dir)
        .max_depth(Some(1))
        .hidden(false)
        .require_git(false)
        .add_custom_ignore_filename(".kernignore")
        .parents(dir.starts_with(project))
        .build();
    for e in walker.flatten() {
        if e.depth() == 1 {
            visible.insert(e.file_name().to_string_lossy().to_string());
        }
    }
    let mut all = std::collections::HashSet::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if !visible.contains(&n) {
                all.insert(n);
            }
        }
    }
    all
}

/// Check a name typed for a new or renamed item. `/` (and `\` on Windows) separate
/// nested folders. Windows rules apply everywhere, so a project stays usable there.
pub fn validate_name(name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(anyhow!("Enter a name"));
    }
    if name.starts_with('/') || name.starts_with('\\') {
        return Err(anyhow!(
            "Use a name relative to the folder, not an absolute path"
        ));
    }
    for part in split_name(name) {
        check_component(part)?;
    }
    Ok(())
}

fn split_name(name: &str) -> impl Iterator<Item = &str> {
    name.split(|c| c == '/' || (cfg!(windows) && c == '\\'))
}

const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

fn check_component(part: &str) -> Result<()> {
    if part.is_empty() {
        return Err(anyhow!("The name has an empty folder part"));
    }
    if part == "." || part == ".." {
        return Err(anyhow!(
            "“{part}” cannot be used: the item must stay inside the project"
        ));
    }
    if let Some(c) = part
        .chars()
        .find(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '\\') || (*c as u32) < 0x20)
    {
        let shown = if (c as u32) < 0x20 {
            "control characters".to_string()
        } else {
            format!("“{c}”")
        };
        return Err(anyhow!(
            "Names cannot contain {shown} (not allowed on Windows)"
        ));
    }
    if part.ends_with('.') || part.ends_with(' ') {
        return Err(anyhow!(
            "Names cannot end with a dot or a space (not allowed on Windows)"
        ));
    }
    let stem = part.split('.').next().unwrap_or(part).trim_end();
    if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem)) {
        return Err(anyhow!("“{part}” is a reserved device name on Windows"));
    }
    if part.len() > 255 {
        return Err(anyhow!("The name is too long"));
    }
    Ok(())
}

/// The new path for `name` under `dir`, refusing anything that would land outside
/// `root`, including through a symbolic link.
fn target_in(root: &Path, dir: &Path, name: &str) -> Result<PathBuf> {
    validate_name(name)?;
    let mut path = dir.to_path_buf();
    for part in split_name(name.trim()) {
        path.push(part);
    }
    let root = root
        .canonicalize()
        .map_err(|e| anyhow!("cannot read the project folder: {e}"))?;
    // The deepest part that exists already decides where the item really goes.
    let mut existing = path.parent();
    while let Some(p) = existing {
        if p.exists() {
            break;
        }
        existing = p.parent();
    }
    let real = existing
        .ok_or_else(|| anyhow!("invalid location"))?
        .canonicalize()
        .map_err(|e| anyhow!("cannot read {}: {e}", dir.display()))?;
    if !real.starts_with(&root) {
        return Err(anyhow!("The item must stay inside the project folder"));
    }
    Ok(path)
}

fn exists_error(path: &Path) -> anyhow::Error {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let kind = if path.is_dir() { "A folder" } else { "A file" };
    anyhow!("{kind} named “{name}” already exists here")
}

/// Create an empty file inside `root`, refusing to touch an existing one.
pub fn create_file(root: &Path, dir: &Path, name: &str) -> Result<PathBuf> {
    let path = target_in(root, dir, name)?;
    if path.exists() {
        return Err(exists_error(&path));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow!("cannot create {}: {e}", parent.display()))?;
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(_) => Ok(path),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(exists_error(&path)),
        Err(e) => Err(anyhow!("cannot create {}: {e}", path.display())),
    }
}

/// Create a folder inside `root`; an existing one is an error, not reused.
pub fn create_dir(root: &Path, dir: &Path, name: &str) -> Result<PathBuf> {
    let path = target_in(root, dir, name)?;
    if path.exists() {
        return Err(exists_error(&path));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow!("cannot create {}: {e}", parent.display()))?;
    }
    match std::fs::create_dir(&path) {
        Ok(()) => Ok(path),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(exists_error(&path)),
        Err(e) => Err(anyhow!("cannot create {}: {e}", path.display())),
    }
}

/// Rename without overwriting a different existing file.
pub fn rename(path: &Path, new_name: &str) -> Result<PathBuf> {
    validate_name(new_name)?;
    let parent = path.parent().ok_or_else(|| anyhow!("no parent"))?;
    let target = parent.join(new_name.trim());
    let same_file_case_change =
        target.to_string_lossy().to_lowercase() == path.to_string_lossy().to_lowercase();
    if target.exists() && !same_file_case_change {
        return Err(anyhow!("{} already exists", target.display()));
    }
    if let Some(p) = target.parent() {
        std::fs::create_dir_all(p)?;
    }
    std::fs::rename(path, &target)?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_refuses_overwrite_and_traversal() {
        let d = tempfile::tempdir().unwrap();
        let p = create_file(d.path(), d.path(), "новый файл.ts").unwrap();
        std::fs::write(&p, "keep").unwrap();
        assert!(create_file(d.path(), d.path(), "новый файл.ts").is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "keep");
        assert!(create_file(d.path(), d.path(), "../evil").is_err());
        assert!(rename(&p, "a/../../b").is_err());
    }

    #[test]
    fn rename_refuses_to_clobber() {
        let d = tempfile::tempdir().unwrap();
        let a = create_file(d.path(), d.path(), "a").unwrap();
        let b = create_file(d.path(), d.path(), "b").unwrap();
        std::fs::write(&b, "b").unwrap();
        assert!(rename(&a, "b").is_err());
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "b");
        let c = rename(&a, "c").unwrap();
        assert!(c.exists() && !a.exists());
    }

    #[test]
    fn listing_marks_gitignored_and_hides_git() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join(".git")).unwrap();
        std::fs::create_dir(d.path().join("src")).unwrap();
        std::fs::create_dir(d.path().join("node_modules")).unwrap();
        std::fs::write(d.path().join(".gitignore"), "secret.txt\n").unwrap();
        std::fs::write(d.path().join("secret.txt"), "").unwrap();
        std::fs::write(d.path().join("a.ts"), "").unwrap();
        let list = list_dir(d.path(), d.path()).unwrap();
        let names: Vec<_> = list.iter().map(|e| e.name.as_str()).collect();
        assert!(!names.contains(&".git"));
        assert_eq!(names[0], "node_modules");
        assert!(
            list.iter()
                .find(|e| e.name == "secret.txt")
                .unwrap()
                .ignored
        );
        assert!(
            list.iter()
                .find(|e| e.name == "node_modules")
                .unwrap()
                .ignored
        );
        assert!(!list.iter().find(|e| e.name == "a.ts").unwrap().ignored);
    }

    #[test]
    fn names_follow_windows_rules() {
        for ok in [
            ".gitignore",
            "мой файл.md",
            "a b c.txt",
            "src/new.ts",
            ".env.example",
            "console.txt",
        ] {
            assert!(validate_name(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "  ",
            "/abs",
            "a/../b",
            "..",
            ".",
            "a//b",
            "con",
            "CON.txt",
            "nul",
            "Com1.log",
            "lpt9",
            "a:b",
            "a*b",
            "q?",
            "<x>",
            "pipe|",
            "quote\"",
            "dot.",
            "dir /x",
            "tab\tname",
        ] {
            assert!(validate_name(bad).is_err(), "{bad:?}");
        }
        // `\` separates folders on Windows; elsewhere it would make a file name
        // that Windows cannot open.
        assert_eq!(validate_name("back\\slash").is_ok(), cfg!(windows));
    }

    #[test]
    fn folders_nest_and_never_overwrite() {
        let d = tempfile::tempdir().unwrap();
        let f = create_dir(d.path(), d.path(), "Новая папка").unwrap();
        assert!(f.is_dir());
        let err = create_dir(d.path(), d.path(), "Новая папка").unwrap_err();
        assert!(err.to_string().contains("already exists"), "{err}");
        let nested = create_file(d.path(), &f, "a/b/.gitignore").unwrap();
        assert!(nested.is_file() && nested.starts_with(&f));
        // A file where a folder is asked for, and the other way round.
        assert!(create_dir(d.path(), &f, "a/b/.gitignore").is_err());
        assert!(create_file(d.path(), d.path(), "Новая папка").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_out_of_the_project_is_refused() {
        let outside = tempfile::tempdir().unwrap();
        let d = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), d.path().join("link")).unwrap();
        assert!(create_file(d.path(), &d.path().join("link"), "x.txt").is_err());
        assert!(create_dir(d.path(), d.path(), "link/sub").is_err());
        assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
        // A folder outside the root is refused as well.
        assert!(create_file(d.path(), outside.path(), "y.txt").is_err());
    }
}
