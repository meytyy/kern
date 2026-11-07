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

/// Shows `path` selected in File Explorer. `/select,` takes the path as one
/// argument, quoted, which `raw_arg` passes exactly as written.
pub fn reveal(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    Command::new("explorer.exe")
        .raw_arg(format!("/select,\"{}\"", simplify_path(path).display()))
        .spawn()
        .map(|_| ())
}

/// `std::fs::canonicalize` returns verbatim paths (`\\?\C:\…`, `\\?\UNC\…`).
/// The Shell (Recycle Bin, Explorer) refuses them, and file watchers report plain
/// paths that would never match them, so KERN keeps the plain form whenever it
/// names the same file: a drive or share path shorter than MAX_PATH.
pub fn simplify_path(path: &std::path::Path) -> std::path::PathBuf {
    let text = path.as_os_str().to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        if let Some(share) = rest.strip_prefix(r"UNC\") {
            if share.len() + 2 < 260 {
                return std::path::PathBuf::from(format!(r"\\{share}"));
            }
        } else {
            let b = rest.as_bytes();
            if b.len() >= 3
                && b[0].is_ascii_alphabetic()
                && b[1] == b':'
                && b[2] == b'\\'
                && rest.len() < 260
            {
                return std::path::PathBuf::from(rest);
            }
        }
    }
    path.to_path_buf()
}

/// What `SHFileOperationW` return codes mean. They are the Shell's own `DE_*`
/// values, not Win32 error codes (0x7C is DE_INVALIDFILES here, not
/// ERROR_INVALID_LEVEL).
fn shell_error(code: i32) -> String {
    let reason = match code {
        0x71 => "source and destination are the same file",
        0x7C => "the path is not valid for the Recycle Bin",
        0x78 | 0x86 => "access denied",
        0x79 => "the path is too long",
        0x80 => "the file is in use by another program",
        0x81 => "the file name is too long for the Recycle Bin",
        0x10000 | 0x402 => "the item could not be found",
        0x4C7 => "cancelled",
        _ => "the Recycle Bin refused the item",
    };
    format!("{reason} (Shell error {code:#x})")
}

/// Moves `path` to the Recycle Bin. With FOF_ALLOWUNDO the shell recycles; if a
/// disk has no Recycle Bin, FOF_WANTNUKEWARNING makes Windows ask before deleting
/// for good instead of doing it silently.
pub fn trash(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{
        FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT,
        FOF_WANTNUKEWARNING, SHFILEOPSTRUCTW, SHFileOperationW,
    };
    let full = simplify_path(&std::path::absolute(path)?);
    if full.as_os_str().to_string_lossy().starts_with(r"\\?\") {
        return Err(std::io::Error::other(
            "the path is too long for the Recycle Bin; nothing was deleted",
        ));
    }
    // A list of paths, each NUL-terminated, ending with an extra NUL.
    let mut from: Vec<u16> = full.as_os_str().encode_wide().collect();
    from.extend([0, 0]);
    let mut op: SHFILEOPSTRUCTW = unsafe { std::mem::zeroed() };
    op.wFunc = FO_DELETE;
    op.pFrom = from.as_ptr();
    op.fFlags =
        (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI | FOF_WANTNUKEWARNING)
            as u16;
    let code = unsafe { SHFileOperationW(&mut op) };
    if code != 0 {
        return Err(std::io::Error::other(shell_error(code)));
    }
    if op.fAnyOperationsAborted != 0 {
        return Err(std::io::Error::other("cancelled; nothing was deleted"));
    }
    if path.exists() {
        return Err(std::io::Error::other("the item is still there"));
    }
    Ok(())
}

/// Held for as long as KERN runs. The installer started by an in-app update
/// waits until it is released, so files are replaced only after KERN quit.
pub const RUNNING_MUTEX: &str = "KernAppRunning";

pub fn mark_running() {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let name: Vec<u16> = std::ffi::OsStr::new(RUNNING_MUTEX)
        .encode_wide()
        .chain([0])
        .collect();
    // Never closed: the system releases it when the process ends.
    unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
}

/// Starts the downloaded installer for an in-app update, silently, for the
/// installation KERN is running from. ShellExecute shows the standard UAC
/// prompt when the installation needs administrator rights; declining it is
/// an error and KERN keeps running.
pub fn launch_update(installer: &std::path::Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    let exe = std::env::current_exe()?;
    let dir = simplify_path(exe.parent().unwrap_or(std::path::Path::new(".")));
    let program_files = std::env::var_os("ProgramFiles").map(std::path::PathBuf::from);
    let scope = if program_files.is_some_and(|p| dir.starts_with(p)) {
        "/ALLUSERS"
    } else {
        "/CURRENTUSER"
    };
    let params = format!(
        "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /KERNUPDATE=1 {scope} /DIR=\"{}\"",
        dir.display()
    );
    let wide = |s: &std::ffi::OsStr| -> Vec<u16> { s.encode_wide().chain([0]).collect() };
    let file = wide(simplify_path(installer).as_os_str());
    let params = wide(std::ffi::OsStr::new(&params));
    let verb = wide(std::ffi::OsStr::new("open"));
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            params.as_ptr(),
            std::ptr::null(),
            1, // SW_SHOWNORMAL
        )
    } as isize;
    if result > 32 {
        Ok(())
    } else if result == 5 {
        Err(std::io::Error::other(
            "the update needs administrator permission, which was not given",
        ))
    } else {
        Err(std::io::Error::other(format!(
            "cannot start the installer (code {result})"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbatim_paths_become_plain() {
        let p = |s: &str| std::path::PathBuf::from(s);
        assert_eq!(
            simplify_path(&p(r"\\?\C:\Users\a b\проект")),
            p(r"C:\Users\a b\проект")
        );
        assert_eq!(
            simplify_path(&p(r"\\?\UNC\srv\share\x")),
            p(r"\\srv\share\x")
        );
        assert_eq!(simplify_path(&p(r"C:\plain")), p(r"C:\plain"));
        let long = format!(r"\\?\C:\{}", "a".repeat(300));
        assert_eq!(simplify_path(&p(&long)), p(&long));
    }

    /// The bug behind "Windows error 0x7c": a canonicalized (verbatim) path. The
    /// Shell refuses it; KERN now hands it the plain path, and the file really
    /// is in the Recycle Bin afterwards and can be restored from there.
    #[test]
    fn recycle_bin_takes_canonical_paths_with_spaces_and_cyrillic() {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::UI::Shell::{
            FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT,
            SHFILEOPSTRUCTW, SHFileOperationW,
        };
        let dir = tempfile::tempdir().unwrap();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let name = format!("kern корзина {stamp}.txt");
        let file = dir.path().join(&name);
        std::fs::write(&file, "restore me").unwrap();
        let verbatim = std::fs::canonicalize(&file).unwrap();
        assert!(verbatim.to_string_lossy().starts_with(r"\\?\"));

        // What the previous build did: the verbatim path straight to the Shell.
        let mut from: Vec<u16> = verbatim.as_os_str().encode_wide().collect();
        from.extend([0, 0]);
        let mut op: SHFILEOPSTRUCTW = unsafe { std::mem::zeroed() };
        op.wFunc = FO_DELETE;
        op.pFrom = from.as_ptr();
        op.fFlags = (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI) as u16;
        let code = unsafe { SHFileOperationW(&mut op) };
        println!("verbatim path: SHFileOperationW returned {code:#x}");
        assert_eq!(code, 0x7C, "DE_INVALIDFILES");
        assert!(file.exists(), "nothing deleted on error");

        // The fix, with the same path; a folder too.
        trash(&verbatim).unwrap();
        assert!(!file.exists());
        let folder = dir.path().join(format!("папка с пробелом {stamp}"));
        std::fs::create_dir_all(folder.join("inner")).unwrap();
        std::fs::write(folder.join("inner/a.txt"), "x").unwrap();
        trash(&std::fs::canonicalize(&folder).unwrap()).unwrap();
        assert!(!folder.exists());

        // It is in the Recycle Bin and comes back with Restore.
        let script = format!(
            "$bin = (New-Object -ComObject Shell.Application).NameSpace(10); \
             $item = $bin.Items() | Where-Object {{ $_.Name -eq '{name}' }} | Select-Object -First 1; \
             if (-not $item) {{ exit 3 }}; $item.InvokeVerb('undelete'); Start-Sleep 2"
        );
        let status = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .status()
            .unwrap();
        assert!(status.success(), "not found in the Recycle Bin: {status}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "restore me");
        std::fs::remove_file(&file).ok();
    }
}
