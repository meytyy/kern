//! Quick-open and project-wide content search. Both honor .gitignore and skip
//! `.git`, `node_modules` and `target`. Run these off the UI thread.
use regex::RegexBuilder;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

fn walker(root: &Path) -> ignore::WalkBuilder {
    let mut b = ignore::WalkBuilder::new(root);
    b.hidden(false)
        .require_git(false)
        .add_custom_ignore_filename(".kernignore")
        .filter_entry(|e| {
            let n = e.file_name().to_string_lossy();
            !(e.file_type().is_some_and(|t| t.is_dir())
                && matches!(n.as_ref(), ".git" | "node_modules" | "target"))
        });
    b
}

pub fn list_files(root: &Path, cancel: &AtomicBool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in walker(root).build().flatten() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if e.file_type().is_some_and(|t| t.is_file())
            && let Ok(rel) = e.path().strip_prefix(root)
        {
            out.push(rel.to_path_buf());
        }
    }
    out.sort();
    out
}

/// Subsequence fuzzy score; higher is better. Prefers matches in the file name,
/// at word boundaries and contiguous runs.
pub fn fuzzy_score(query: &str, path: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let lower: Vec<char> = path.to_lowercase().chars().collect();
    let chars: Vec<char> = path.chars().collect();
    if lower.len() != chars.len() {
        return None;
    }
    let name_start = chars.iter().rposition(|c| *c == '/').map_or(0, |i| i + 1);
    let mut score = 0i64;
    let mut pos = 0;
    let mut prev: Option<usize> = None;
    for q in query.to_lowercase().chars().filter(|c| !c.is_whitespace()) {
        let found = (pos..lower.len()).find(|i| lower[*i] == q)?;
        score += 1;
        if prev == Some(found.wrapping_sub(1)) {
            score += 8;
        }
        if found >= name_start {
            score += 4;
        }
        if found == name_start {
            score += 12;
        }
        if found > 0 && matches!(chars[found - 1], '/' | '_' | '-' | '.' | ' ') {
            score += 6;
        }
        if chars[found].is_uppercase() {
            score += 2;
        }
        prev = Some(found);
        pos = found + 1;
    }
    Some(score * 16 - lower.len() as i64)
}

pub fn fuzzy_filter(query: &str, files: &[PathBuf], limit: usize) -> Vec<PathBuf> {
    let mut scored: Vec<(i64, &PathBuf)> = files
        .iter()
        .filter_map(|p| fuzzy_score(query, &p.to_string_lossy()).map(|s| (s, p)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    scored
        .into_iter()
        .take(limit)
        .map(|(_, p)| p.clone())
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchMatch {
    pub path: PathBuf,
    /// 0-based.
    pub line: usize,
    /// Byte column of the match in the line.
    pub column: usize,
    pub len: usize,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct SearchOptions {
    pub case_sensitive: bool,
    pub regex: bool,
}

pub const MAX_RESULTS: usize = 2000;

/// Search every text file. Streams batches into `on_batch`; returns an error
/// message only for an invalid pattern.
pub fn search_project(
    root: &Path,
    query: &str,
    opts: &SearchOptions,
    cancel: Arc<AtomicBool>,
    mut on_batch: impl FnMut(Vec<SearchMatch>),
) -> Result<usize, String> {
    let pattern = if opts.regex {
        query.to_string()
    } else {
        regex::escape(query)
    };
    let re = RegexBuilder::new(&pattern)
        .case_insensitive(!opts.case_sensitive)
        .size_limit(1 << 24)
        .build()
        .map_err(|e| e.to_string())?;
    let mut total = 0;
    for e in walker(root).build().flatten() {
        if cancel.load(Ordering::Relaxed) || total >= MAX_RESULTS {
            break;
        }
        if !e.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if e.metadata()
            .map(|m| m.len() > 8 * 1024 * 1024)
            .unwrap_or(true)
        {
            continue;
        }
        let Ok(bytes) = std::fs::read(e.path()) else {
            continue;
        };
        if bytes[..bytes.len().min(8192)].contains(&0) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        let mut batch = Vec::new();
        for (line_ix, line) in text.lines().enumerate() {
            for m in re.find_iter(line) {
                let rel = e
                    .path()
                    .strip_prefix(root)
                    .unwrap_or(e.path())
                    .to_path_buf();
                let display: String = line.trim_end().chars().take(400).collect();
                batch.push(SearchMatch {
                    path: rel,
                    line: line_ix,
                    column: m.start(),
                    len: m.len(),
                    text: display,
                });
                total += 1;
                if total >= MAX_RESULTS {
                    break;
                }
            }
            if total >= MAX_RESULTS {
                break;
            }
        }
        if !batch.is_empty() {
            on_batch(batch);
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        std::fs::create_dir_all(root.join("src/components")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/x")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".gitignore"), "dist/\n*.log\n").unwrap();
        std::fs::create_dir_all(root.join("dist")).unwrap();
        std::fs::write(root.join("dist/out.js"), "needle").unwrap();
        std::fs::write(root.join("x.log"), "needle").unwrap();
        std::fs::write(root.join("node_modules/x/i.js"), "needle").unwrap();
        std::fs::write(root.join("target/t"), "needle").unwrap();
        std::fs::write(root.join(".git/config"), "needle").unwrap();
        std::fs::write(
            root.join("src/App.tsx"),
            "const a = 1;\nconst Needle = 'Привет needle';\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/components/Button.tsx"),
            "export const B = 1;\n",
        )
        .unwrap();
        std::fs::write(root.join("bin.dat"), b"needle\0binary").unwrap();
        d
    }

    #[test]
    fn exclusions_apply_to_listing_and_search() {
        let d = project();
        let cancel = AtomicBool::new(false);
        let files = list_files(d.path(), &cancel);
        let names: Vec<String> = files
            .iter()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect();
        assert!(names.contains(&"src/App.tsx".to_string()));
        for bad in [
            "dist/out.js",
            "x.log",
            "node_modules/x/i.js",
            "target/t",
            ".git/config",
        ] {
            assert!(!names.contains(&bad.to_string()), "{bad}");
        }
        let mut hits = vec![];
        search_project(
            d.path(),
            "needle",
            &SearchOptions::default(),
            Arc::new(AtomicBool::new(false)),
            |b| hits.extend(b),
        )
        .unwrap();
        assert_eq!(hits.len(), 2);
        assert!(
            hits.iter()
                .all(|h| h.path == Path::new("src/App.tsx") && h.line == 1)
        );
    }

    #[test]
    fn case_and_regex_options() {
        let d = project();
        let c = || Arc::new(AtomicBool::new(false));
        let mut hits = vec![];
        let opts = SearchOptions {
            case_sensitive: true,
            regex: false,
        };
        search_project(d.path(), "Needle", &opts, c(), |b| hits.extend(b)).unwrap();
        assert_eq!(hits.len(), 1);
        let opts = SearchOptions {
            case_sensitive: false,
            regex: true,
        };
        let mut hits = vec![];
        search_project(d.path(), "прив.т", &opts, c(), |b| hits.extend(b)).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(search_project(d.path(), "(", &opts, c(), |_| {}).is_err());
    }

    #[test]
    fn fuzzy_prefers_file_name_matches() {
        let files = vec![
            PathBuf::from("src/apps/page.tsx"),
            PathBuf::from("src/App.tsx"),
            PathBuf::from("zzz.txt"),
        ];
        let out = fuzzy_filter("app", &files, 10);
        assert_eq!(out[0], PathBuf::from("src/App.tsx"));
        assert!(!out.contains(&PathBuf::from("zzz.txt")));
    }
}
