//! Filesystem change notifications, debounced and delivered off the UI thread.
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct ProjectWatcher {
    _watcher: RecommendedWatcher,
}

impl ProjectWatcher {
    /// `on_change` receives batches of changed paths outside ignored build dirs.
    pub fn start(
        root: &Path,
        on_change: impl Fn(Vec<PathBuf>) + Send + 'static,
    ) -> notify::Result<Self> {
        let (tx, rx) = std::sync::mpsc::channel::<PathBuf>();
        let mut watcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if let Ok(ev) = res {
                    for p in ev.paths {
                        let _ = tx.send(p);
                    }
                }
            })?;
        watcher.watch(root, RecursiveMode::Recursive)?;
        std::thread::Builder::new()
            .name("kern-watch".into())
            .spawn(move || {
                while let Ok(first) = rx.recv() {
                    let mut set = HashSet::new();
                    set.insert(first);
                    while let Ok(p) = rx.recv_timeout(Duration::from_millis(150)) {
                        set.insert(p);
                        if set.len() > 4096 {
                            break;
                        }
                    }
                    let list: Vec<PathBuf> = set.into_iter().filter(|p| !is_noise(p)).collect();
                    if !list.is_empty() {
                        on_change(list);
                    }
                }
            })
            .ok();
        Ok(Self { _watcher: watcher })
    }
}

/// `.git` internals that matter for status are kept; object churn is not.
fn is_noise(p: &Path) -> bool {
    let mut comps = p
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string());
    let s: Vec<String> = comps.by_ref().collect();
    if s.iter().any(|c| c == "node_modules" || c == "target") {
        return true;
    }
    if let Some(ix) = s.iter().position(|c| c == ".git") {
        let rest = &s[ix + 1..];
        return !(rest
            .first()
            .is_some_and(|c| c == "HEAD" || c == "index" || c == "refs" || c == "MERGE_HEAD"));
    }
    s.last()
        .is_some_and(|c| c.contains(".kern-") && c.ends_with(".tmp"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_filter() {
        assert!(is_noise(Path::new("/p/node_modules/x/y.js")));
        assert!(is_noise(Path::new("/p/.git/objects/ab/cd")));
        assert!(is_noise(Path::new("/p/.a.ts.kern-12.tmp")));
        assert!(!is_noise(Path::new("/p/.git/HEAD")));
        assert!(!is_noise(Path::new("/p/.git/index")));
        assert!(!is_noise(Path::new("/p/src/a.ts")));
    }

    #[test]
    fn delivers_real_changes() {
        let d = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(d.path()).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let _w = ProjectWatcher::start(&root, move |p| {
            let _ = tx.send(p);
        })
        .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(root.join("a.txt"), "x").unwrap();
        // FSEvents may first report earlier activity on the directory itself.
        loop {
            let got = rx.recv_timeout(Duration::from_secs(10)).expect("event");
            if got.iter().any(|p| p.ends_with("a.txt")) {
                break;
            }
        }
    }
}
