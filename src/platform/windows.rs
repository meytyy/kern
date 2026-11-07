//! Windows process helpers: hidden consoles, process trees, the default shell.
use std::collections::HashMap;
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::time::{Duration, Instant};

/// Used when neither a setting nor `pwsh`/`powershell` is available.
pub const DEFAULT_SHELL: &str = "cmd.exe";

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// A console program started from a GUI app opens its own window unless told not to.
pub fn no_console(cmd: &mut Command) {
    cmd.creation_flags(CREATE_NO_WINDOW);
}

/// Its own group and no console window. The tree is ended with `kill_tree`.
pub fn new_process_group(cmd: &mut Command) {
    cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
}

pub fn group_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: the handle is checked and closed; the exit code is a local out value.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return false;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(h, &mut code) != 0;
        CloseHandle(h);
        ok && code == STILL_ACTIVE as u32
    }
}

/// End the process and every process it started (`taskkill /T`).
pub fn kill_tree(pid: u32) {
    let mut cmd = Command::new("taskkill");
    cmd.args(["/PID", &pid.to_string(), "/T", "/F"]);
    no_console(&mut cmd);
    let _ = cmd.output();
}

/// Console programs have no gentle close from outside their console: the tree
/// gets the grace period to finish on its own, then it is ended.
pub fn terminate_group(pid: u32, grace: Duration) {
    let start = Instant::now();
    let mut asked = false;
    while start.elapsed() < grace {
        if !group_alive(pid) {
            return;
        }
        if !asked {
            let mut cmd = Command::new("taskkill");
            cmd.args(["/PID", &pid.to_string(), "/T"]);
            no_console(&mut cmd);
            let _ = cmd.output();
            asked = true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    kill_tree(pid);
}

/// `pwsh`, then Windows PowerShell, then `cmd.exe`, unless the setting names one.
pub fn user_shell(configured: Option<&str>) -> String {
    if let Some(s) = configured.filter(|s| !s.trim().is_empty()) {
        return s.to_string();
    }
    for candidate in ["pwsh", "powershell"] {
        if let Some(p) = super::tools::find(candidate) {
            return p.to_string_lossy().into_owned();
        }
    }
    std::env::var("ComSpec").unwrap_or_else(|_| DEFAULT_SHELL.to_string())
}

/// Arguments for an interactive session of this shell.
pub fn shell_args(shell: &str) -> Vec<String> {
    let name = std::path::Path::new(shell)
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match name.as_str() {
        "pwsh" | "powershell" => vec!["-NoLogo".into()],
        _ => vec![],
    }
}

/// Programs started from Explorer already get the user's environment.
pub fn capture_login_env(_shell: &str, _timeout: Duration) -> Option<HashMap<String, String>> {
    None
}

pub fn is_executable(path: &std::path::Path) -> bool {
    path.is_file()
}

/// File name variants Windows tries for a bare command name.
pub fn executable_names(tool: &str) -> Vec<String> {
    if std::path::Path::new(tool).extension().is_some() {
        return vec![tool.to_string()];
    }
    let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    exts.split(';')
        .filter(|e| !e.is_empty())
        .map(|e| format!("{tool}{}", e.to_lowercase()))
        .collect()
}

/// Inside a virtual environment.
pub const VENV_PYTHON: &str = "Scripts\\python.exe";
pub const DEFAULT_PYTHON: &str = "python";
