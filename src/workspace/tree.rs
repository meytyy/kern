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

/// Reject names that would escape the directory.
pub fn validate_name(name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(anyhow!("name is empty"));
    }
    if name == "."
        || name == ".."
        || name.split('/').any(|c| c == ".." || c == ".")
        || name.starts_with('/')
    {
        return Err(anyhow!("invalid name"));
    }
    Ok(())
}

/// Create an empty file, refusing to touch an existing one.
pub fn create_file(dir: &Path, name: &str) -> Result<PathBuf> {
    validate_name(name)?;
    let path = dir.join(name.trim());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| anyhow!("cannot create {}: {e}", path.display()))?;
    Ok(path)
}

pub fn create_dir(dir: &Path, name: &str) -> Result<PathBuf> {
    validate_name(name)?;
    let path = dir.join(name.trim());
    if path.exists() {
        return Err(anyhow!("{} already exists", path.display()));
    }
    std::fs::create_dir_all(&path)?;
    Ok(path)
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
        let p = create_file(d.path(), "новый файл.ts").unwrap();
        std::fs::write(&p, "keep").unwrap();
        assert!(create_file(d.path(), "новый файл.ts").is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "keep");
        assert!(create_file(d.path(), "../evil").is_err());
        assert!(rename(&p, "a/../../b").is_err());
    }

    #[test]
    fn rename_refuses_to_clobber() {
        let d = tempfile::tempdir().unwrap();
        let a = create_file(d.path(), "a").unwrap();
        let b = create_file(d.path(), "b").unwrap();
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
}
