//! Unix process helpers: process groups, signals, login-shell environment.
use std::collections::HashMap;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const DEFAULT_SHELL: &str = "/bin/zsh";

/// Inside a virtual environment.
pub const VENV_PYTHON: &str = "bin/python";
pub const DEFAULT_PYTHON: &str = "python3";

/// Only Windows opens console windows for child processes.
pub fn no_console(_cmd: &mut Command) {}

pub fn kill_tree(pgid: u32) {
    signal_group(pgid, libc::SIGKILL);
}

/// Arguments for an interactive session of this shell: a login shell.
pub fn shell_args(_shell: &str) -> Vec<String> {
    vec!["-l".into()]
}

pub fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

pub fn executable_names(tool: &str) -> Vec<String> {
    vec![tool.to_string()]
}

/// Run the child as the leader of a new process group so the whole tree can be signalled.
pub fn new_process_group(cmd: &mut Command) {
    cmd.process_group(0);
}

pub fn signal_group(pgid: u32, signal: i32) -> bool {
    // SAFETY: plain libc call; a negative pid addresses the group.
    unsafe { libc::kill(-(pgid as i32), signal) == 0 }
}

pub fn group_alive(pgid: u32) -> bool {
    // SAFETY: signal 0 performs only an existence check.
    unsafe { libc::kill(-(pgid as i32), 0) == 0 }
}

/// SIGTERM the group, then SIGKILL whatever survives the grace period.
pub fn terminate_group(pgid: u32, grace: Duration) {
    signal_group(pgid, libc::SIGTERM);
    let start = Instant::now();
    while start.elapsed() < grace {
        if !group_alive(pgid) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    signal_group(pgid, libc::SIGKILL);
}

pub fn user_shell(configured: Option<&str>) -> String {
    configured
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .or_else(|| std::env::var("SHELL").ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| DEFAULT_SHELL.to_string())
}

const MARKER: &str = "__KERN_ENV_MARKER__";

/// Apps opened from Finder inherit a minimal launchd environment. Ask the user's
/// login shell for the real one; `None` when the shell misbehaves or times out.
pub fn capture_login_env(shell: &str, timeout: Duration) -> Option<HashMap<String, String>> {
    let script = format!("printf '{MARKER}'; /usr/bin/env -0; printf '{MARKER}'");
    let mut child = Command::new(shell)
        .args(["-l", "-i", "-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .ok()?;
    let pid = child.id();
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    let out = rx.recv_timeout(timeout);
    if out.is_err() {
        signal_group(pid, libc::SIGKILL);
    }
    let _ = child.wait();
    parse_env_output(&out.ok()?)
}

pub fn parse_env_output(bytes: &[u8]) -> Option<HashMap<String, String>> {
    let text = String::from_utf8_lossy(bytes);
    let start = text.find(MARKER)? + MARKER.len();
    let end = start + text[start..].find(MARKER)?;
    let mut env = HashMap::new();
    for entry in text[start..end].split('\0') {
        if let Some((k, v)) = entry.split_once('=')
            && !k.is_empty()
        {
            env.insert(k.to_string(), v.to_string());
        }
    }
    Some(env)
}

/// Shows `path` selected in Finder.
pub fn reveal(path: &std::path::Path) -> std::io::Result<()> {
    Command::new("/usr/bin/open")
        .arg("-R")
        .arg(path)
        .spawn()
        .map(|_| ())
}

/// Moves `path` to the Trash with `NSFileManager`, as Finder does. A disk
/// without a Trash is an error: nothing is deleted for good.
#[cfg(target_os = "macos")]
pub fn trash(path: &std::path::Path) -> std::io::Result<()> {
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject, Bool};
    use objc2_foundation::NSString;
    let full = std::path::absolute(path)?;
    unsafe {
        let class = |name: &std::ffi::CStr| {
            AnyClass::get(name).ok_or_else(|| std::io::Error::other("Foundation is missing"))
        };
        let text = NSString::from_str(&full.to_string_lossy());
        let url: Option<Retained<AnyObject>> = msg_send![class(c"NSURL")?, fileURLWithPath: &*text];
        let url = url.ok_or_else(|| std::io::Error::other("bad path"))?;
        let manager: Retained<AnyObject> = msg_send![class(c"NSFileManager")?, defaultManager];
        let mut error: *mut AnyObject = std::ptr::null_mut();
        let ok: Bool = msg_send![
            &*manager,
            trashItemAtURL: &*url,
            resultingItemURL: std::ptr::null_mut::<*mut AnyObject>(),
            error: &mut error
        ];
        if ok.as_bool() {
            return Ok(());
        }
        let reason = if error.is_null() {
            "the Trash is not available here".to_string()
        } else {
            let text: Retained<NSString> = msg_send![error, localizedDescription];
            text.to_string()
        };
        Err(std::io::Error::other(reason))
    }
}

#[cfg(not(target_os = "macos"))]
pub fn trash(_path: &std::path::Path) -> std::io::Result<()> {
    Err(std::io::Error::other("no system trash on this platform"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_env_with_noise_around_marker() {
        let raw = format!("motd noise{MARKER}A=1\0PATH=/a:/b\0MULTI=x\ny\0{MARKER}trailing");
        let env = parse_env_output(raw.as_bytes()).unwrap();
        assert_eq!(env["A"], "1");
        assert_eq!(env["PATH"], "/a:/b");
        assert_eq!(env["MULTI"], "x\ny");
    }

    #[test]
    fn captures_real_shell_env() {
        let env = capture_login_env("/bin/zsh", Duration::from_secs(20)).expect("env");
        assert!(env.contains_key("PATH"));
    }
}
