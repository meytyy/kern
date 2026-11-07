//! Locating external tools, including when started from Finder.
use parking_lot::RwLock;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolStatus {
    Found(PathBuf),
    /// A file exists on the search path but cannot be executed (e.g. dangling symlink).
    Broken(PathBuf),
    Missing,
}

/// Set by a parent Claude Code session. They describe that session, not this app.
pub const INHERITED_AGENT_MARKERS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SSE_PORT",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
];

static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[derive(Default)]
struct State {
    env: HashMap<String, String>,
    overrides: BTreeMap<String, PathBuf>,
}

fn state() -> &'static RwLock<State> {
    static STATE: OnceLock<RwLock<State>> = OnceLock::new();
    STATE.get_or_init(|| {
        RwLock::new(State {
            env: std::env::vars().collect(),
            overrides: BTreeMap::new(),
        })
    })
}

/// Well-known locations added after the user's PATH.
#[cfg(windows)]
fn fallback_dirs() -> Vec<PathBuf> {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let mut dirs = vec![];
    if let Some(pf) = env("ProgramFiles") {
        for rel in ["nodejs", "Git\\cmd", "GitHub CLI", "PowerShell\\7"] {
            dirs.push(pf.join(rel));
        }
    }
    if let Some(appdata) = env("APPDATA") {
        dirs.push(appdata.join("npm"));
    }
    if let Some(local) = env("LOCALAPPDATA") {
        dirs.push(local.join("Microsoft\\WindowsApps"));
        dirs.push(local.join("Programs\\Python\\Launcher"));
    }
    if let Some(home) = dirs::home_dir() {
        for rel in [".local\\bin", ".cargo\\bin", ".bun\\bin", "scoop\\shims"] {
            dirs.push(home.join(rel));
        }
    }
    dirs
}

/// Well-known locations added after the user's PATH.
#[cfg(unix)]
fn fallback_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = [
        "/opt/homebrew/bin",
        "/opt/homebrew/sbin",
        "/usr/local/bin",
        "/usr/bin",
        "/bin",
        "/usr/sbin",
        "/sbin",
    ]
    .iter()
    .map(PathBuf::from)
    .collect();
    if let Some(home) = dirs::home_dir() {
        for rel in [
            ".local/bin",
            ".cargo/bin",
            ".bun/bin",
            ".volta/bin",
            ".npm-global/bin",
        ] {
            dirs.push(home.join(rel));
        }
    }
    dirs
}

/// Replace the process environment view with the one of the login shell.
/// Runs on a background thread at startup; keeps inherited values on failure.
pub fn initialize_environment(configured_shell: Option<&str>) {
    STARTED.store(true, std::sync::atomic::Ordering::SeqCst);
    let shell = super::user_shell(configured_shell);
    let captured = super::capture_login_env(&shell, Duration::from_secs(8));
    let mut st = state().write();
    if let Some(env) = captured {
        for (k, v) in env {
            // Do not inherit shell-session specifics from the capture subshell.
            if !matches!(k.as_str(), "_" | "SHLVL" | "PWD" | "OLDPWD")
                && !INHERITED_AGENT_MARKERS.contains(&k.as_str())
            {
                st.env.insert(k, v);
            }
        }
    }
    let key = path_key(&st.env);
    let mut path: Vec<PathBuf> = st
        .env
        .get(&key)
        .map(|p| std::env::split_paths(p).collect())
        .unwrap_or_default();
    for dir in fallback_dirs() {
        if !path.contains(&dir) {
            path.push(dir);
        }
    }
    // Development aid: tools outside of the user's PATH.
    if let Some(extra) = std::env::var_os("KERN_EXTRA_PATH") {
        path.insert(0, PathBuf::from(extra));
    }
    if let Ok(joined) = std::env::join_paths(path) {
        st.env.insert(key, joined.to_string_lossy().into_owned());
    }
    let (lock, cv) = ready();
    *lock.lock() = true;
    cv.notify_all();
}

fn ready() -> &'static (parking_lot::Mutex<bool>, parking_lot::Condvar) {
    static READY: OnceLock<(parking_lot::Mutex<bool>, parking_lot::Condvar)> = OnceLock::new();
    READY.get_or_init(Default::default)
}

/// Block (off the UI thread) until the login environment was captured.
pub fn wait_for_environment() {
    if !STARTED.load(std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let (lock, cv) = ready();
    let mut done = lock.lock();
    if !*done {
        cv.wait_for(&mut done, Duration::from_secs(10));
    }
}

pub fn set_overrides(overrides: BTreeMap<String, PathBuf>) {
    state().write().overrides = overrides;
}

/// Environment that child processes should see.
pub fn child_env() -> HashMap<String, String> {
    state().read().env.clone()
}

/// Windows spells it `Path`; environment names there are case-insensitive.
fn path_key(env: &HashMap<String, String>) -> String {
    env.keys()
        .find(|k| k.eq_ignore_ascii_case("PATH"))
        .cloned()
        .unwrap_or_else(|| "PATH".into())
}

pub fn path_var() -> String {
    let st = state().read();
    st.env.get(&path_key(&st.env)).cloned().unwrap_or_default()
}

fn is_executable(path: &Path) -> bool {
    super::is_executable(path)
}

pub fn locate(tool: &str) -> ToolStatus {
    let over = state().read().overrides.get(tool).cloned();
    let path = path_var();
    if let Some(path) = over {
        return if is_executable(&path) {
            ToolStatus::Found(path)
        } else {
            ToolStatus::Broken(path)
        };
    }
    locate_in(tool, std::env::split_paths(&path).chain(fallback_dirs()))
}

pub fn locate_in(tool: &str, dirs: impl Iterator<Item = PathBuf>) -> ToolStatus {
    let mut broken = None;
    for dir in dirs {
        if dir.as_os_str().is_empty() {
            continue;
        }
        for name in super::executable_names(tool) {
            let candidate = dir.join(name);
            if is_executable(&candidate) {
                return ToolStatus::Found(candidate);
            }
            if broken.is_none() && std::fs::symlink_metadata(&candidate).is_ok() {
                broken = Some(candidate);
            }
        }
    }
    broken.map_or(ToolStatus::Missing, ToolStatus::Broken)
}

pub fn find(tool: &str) -> Option<PathBuf> {
    match locate(tool) {
        ToolStatus::Found(p) => Some(p),
        _ => None,
    }
}

/// A command for an external tool with the resolved environment.
pub fn command(tool: &str) -> Command {
    wait_for_environment();
    let program = find(tool).unwrap_or_else(|| PathBuf::from(tool));
    let mut cmd = Command::new(program);
    cmd.env_clear().envs(child_env());
    super::no_console(&mut cmd);
    cmd
}

pub struct ToolInfo {
    pub name: &'static str,
    pub status: ToolStatus,
    pub install_hint: &'static str,
}

#[cfg(not(windows))]
pub const KNOWN_TOOLS: &[(&str, &str)] = &[
    ("git", "xcode-select --install"),
    ("node", "brew install node"),
    ("npm", "brew install node"),
    ("pnpm", "npm install -g pnpm"),
    ("yarn", "npm install -g yarn"),
    ("gh", "brew install gh"),
    ("codex", "brew reinstall --cask codex"),
    ("claude", "curl -fsSL https://claude.ai/install.sh | bash"),
    (
        "cargo",
        "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh",
    ),
    ("python3", "brew install python"),
    ("tsc", "npm install -g typescript"),
    (
        "typescript-language-server",
        "npm install -g typescript@5 typescript-language-server",
    ),
    ("pyright-langserver", "npm install -g pyright"),
    ("rust-analyzer", "rustup component add rust-analyzer"),
    ("sourcekit-lsp", "xcode-select --install"),
    ("clangd", "xcode-select --install"),
    ("cmake", "brew install cmake"),
];

#[cfg(windows)]
pub const KNOWN_TOOLS: &[(&str, &str)] = &[
    ("git", "winget install --id Git.Git -e"),
    ("node", "winget install --id OpenJS.NodeJS.LTS -e"),
    ("npm", "winget install --id OpenJS.NodeJS.LTS -e"),
    ("pnpm", "npm install -g pnpm"),
    ("yarn", "npm install -g yarn"),
    ("gh", "winget install --id GitHub.cli -e"),
    ("codex", "npm install -g @openai/codex"),
    ("claude", "irm https://claude.ai/install.ps1 | iex"),
    ("cargo", "winget install --id Rustlang.Rustup -e"),
    ("python", "winget install --id Python.Python.3.13 -e"),
    ("python3", "winget install --id Python.Python.3.13 -e"),
    ("tsc", "npm install -g typescript"),
    (
        "typescript-language-server",
        "npm install -g typescript@5 typescript-language-server",
    ),
    ("pyright-langserver", "npm install -g pyright"),
    ("rust-analyzer", "rustup component add rust-analyzer"),
    ("clangd", "winget install --id LLVM.LLVM -e"),
    ("cmake", "winget install --id Kitware.CMake -e"),
];

/// Result of actually running a tool. Finding a file is not enough: a rustup
/// proxy exists without the component, and macOS may refuse to run a binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    Ready { path: PathBuf, version: String },
    Broken { path: PathBuf, reason: String },
    Missing,
}

/// Tools that promise `--version`: a failure there means the tool cannot run.
const VERSION_FLAG_RELIABLE: &[&str] = &[
    "git",
    "node",
    "npm",
    "pnpm",
    "yarn",
    "gh",
    "codex",
    "claude",
    "cargo",
    "python3",
    "rust-analyzer",
    "cmake",
    "clangd",
    "tsc",
    "swift",
];

/// Run `tool --version` (blocking, off the UI thread) with a timeout.
pub fn probe(tool: &str) -> Probe {
    wait_for_environment();
    match locate(tool) {
        ToolStatus::Missing => Probe::Missing,
        ToolStatus::Broken(path) => Probe::Broken {
            path,
            reason: "broken link or not executable".into(),
        },
        ToolStatus::Found(path) => probe_path(tool, path),
    }
}

fn probe_path(tool: &str, path: PathBuf) -> Probe {
    use std::process::Stdio;
    let mut cmd = Command::new(&path);
    cmd.arg("--version")
        .env_clear()
        .envs(child_env())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    super::new_process_group(&mut cmd);
    let Ok(child) = cmd.spawn() else {
        return Probe::Broken {
            path,
            reason: "cannot be started".into(),
        };
    };
    let pid = child.id();
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = done.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(6));
        if !flag.load(std::sync::atomic::Ordering::SeqCst) {
            super::kill_tree(pid);
        }
    });
    let output = child.wait_with_output();
    done.store(true, std::sync::atomic::Ordering::SeqCst);
    let Ok(out) = output else {
        return Probe::Broken {
            path,
            reason: "cannot be started".into(),
        };
    };
    let first = |b: &[u8]| {
        String::from_utf8_lossy(b)
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim()
            .to_string()
    };
    if out.status.success() {
        return Probe::Ready {
            path,
            version: first(&out.stdout),
        };
    }
    if VERSION_FLAG_RELIABLE.contains(&tool) {
        let reason = first(&out.stderr);
        return Probe::Broken {
            path,
            reason: if reason.is_empty() {
                format!("exited with {}", out.status)
            } else {
                reason
            },
        };
    }
    // Language servers often have no such flag: found, version unknown.
    Probe::Ready {
        path,
        version: String::new(),
    }
}

pub fn tool_report() -> Vec<ToolInfo> {
    KNOWN_TOOLS
        .iter()
        .map(|(name, hint)| ToolInfo {
            name,
            status: locate(name),
            install_hint: hint,
        })
        .collect()
}

pub fn install_hint(tool: &str) -> &'static str {
    KNOWN_TOOLS
        .iter()
        .find(|(n, _)| *n == tool)
        .map_or("", |(_, h)| h)
}

// These tests drive real Unix processes; Windows is checked on Windows itself.
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn probe_runs_the_tool_for_real() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let ok = d.path().join("ok");
        std::fs::write(&ok, "#!/bin/sh\necho tool 1.2.3\n").unwrap();
        let bad = d.path().join("bad");
        std::fs::write(
            &bad,
            "#!/bin/sh\necho 'error: Unknown binary' 1>&2\nexit 1\n",
        )
        .unwrap();
        for f in [&ok, &bad] {
            std::fs::set_permissions(f, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert!(
            matches!(probe_path("x", ok), Probe::Ready { version, .. } if version == "tool 1.2.3")
        );
        // A rustup proxy without its component looks like this.
        assert!(
            matches!(probe_path("rust-analyzer", bad.clone()), Probe::Broken { reason, .. } if reason.contains("Unknown binary"))
        );
        // A server with no --version flag is still only "found".
        assert!(
            matches!(probe_path("some-server", bad), Probe::Ready { version, .. } if version.is_empty())
        );
    }

    #[test]
    fn dangling_symlink_is_reported_as_broken() {
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/nonexistent/thing", dir.path().join("codex")).unwrap();
        assert!(matches!(
            locate_in("codex", std::iter::once(dir.path().to_path_buf())),
            ToolStatus::Broken(_)
        ));
        assert_eq!(
            locate_in("nothing", std::iter::once(dir.path().to_path_buf())),
            ToolStatus::Missing
        );
    }

    #[test]
    fn broken_entry_does_not_hide_later_valid_one() {
        use std::os::unix::fs::PermissionsExt;
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/nonexistent", a.path().join("t")).unwrap();
        let good = b.path().join("t");
        std::fs::write(&good, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&good, std::fs::Permissions::from_mode(0o755)).unwrap();
        let dirs = vec![a.path().to_path_buf(), b.path().to_path_buf()];
        assert_eq!(locate_in("t", dirs.into_iter()), ToolStatus::Found(good));
    }
}
