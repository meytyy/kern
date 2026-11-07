//! Detection of what can be run in a project. Detection only reads files:
//! nothing from a downloaded project is executed until the user presses Run.
use crate::lang;
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunConfig {
    /// Stable id, used to restore the selection.
    pub id: String,
    pub label: String,
    /// Tool name (`npm`, `cargo`) or absolute path of an interpreter.
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

impl RunConfig {
    pub fn command_line(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .map(|s| {
                shlex::try_quote(s)
                    .map(|q| q.to_string())
                    .unwrap_or_else(|_| s.to_string())
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageManagerChoice {
    Detected(&'static str),
    Ambiguous(Vec<&'static str>),
    /// No hints at all: npm by convention.
    Default,
}

const MANAGERS: &[&str] = &["pnpm", "yarn", "npm", "bun"];

pub fn detect_package_manager(dir: &Path) -> PackageManagerChoice {
    // The `packageManager` field is an explicit statement of intent.
    if let Ok(text) = std::fs::read_to_string(dir.join("package.json"))
        && let Ok(json) = serde_json::from_str::<Value>(&text)
        && let Some(field) = json.get("packageManager").and_then(Value::as_str)
    {
        let name = field.split('@').next().unwrap_or("");
        if let Some(m) = MANAGERS.iter().find(|m| **m == name) {
            return PackageManagerChoice::Detected(m);
        }
    }
    let mut found = vec![];
    if dir.join("pnpm-lock.yaml").exists() {
        found.push("pnpm");
    }
    if dir.join("yarn.lock").exists() {
        found.push("yarn");
    }
    if dir.join("package-lock.json").exists() || dir.join("npm-shrinkwrap.json").exists() {
        found.push("npm");
    }
    if dir.join("bun.lockb").exists() || dir.join("bun.lock").exists() {
        found.push("bun");
    }
    match found.len() {
        0 => PackageManagerChoice::Default,
        1 => PackageManagerChoice::Detected(found[0]),
        _ => PackageManagerChoice::Ambiguous(found),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Detection {
    pub configs: Vec<RunConfig>,
    /// Candidates when the package manager cannot be decided automatically.
    pub ambiguous_managers: Vec<&'static str>,
    pub package_manager: Option<&'static str>,
}

pub fn detect(
    project: &Path,
    chosen_manager: Option<&str>,
    chosen_python: Option<&Path>,
) -> Detection {
    let mut d = Detection::default();
    detect_js(project, chosen_manager, &mut d);
    detect_python(project, chosen_python, &mut d);
    detect_cargo(project, &mut d);
    detect_swift_cmake(project, &mut d);
    d
}

fn detect_js(project: &Path, chosen: Option<&str>, d: &mut Detection) {
    let Ok(text) = std::fs::read_to_string(project.join("package.json")) else {
        return;
    };
    let Ok(json) = serde_json::from_str::<Value>(&text) else {
        return;
    };
    let detection = detect_package_manager(project);
    let manager: &'static str = match (
        chosen.and_then(|c| MANAGERS.iter().find(|m| **m == c).copied()),
        &detection,
    ) {
        (Some(m), _) => m,
        (None, PackageManagerChoice::Detected(m)) => m,
        (None, PackageManagerChoice::Ambiguous(list)) => {
            d.ambiguous_managers = list.clone();
            list[0]
        }
        (None, PackageManagerChoice::Default) => "npm",
    };
    d.package_manager = Some(manager);
    let Some(scripts) = json.get("scripts").and_then(Value::as_object) else {
        return;
    };
    // Order: dev-like scripts first, then the file's order (serde_json keeps sorted keys).
    let mut names: Vec<&String> = scripts.keys().collect();
    names.sort_by_key(|n| match n.as_str() {
        "dev" => 0,
        "start" => 1,
        "serve" => 2,
        "build" => 3,
        "test" => 4,
        _ => 5,
    });
    for name in names {
        d.configs.push(RunConfig {
            id: format!("js:{name}"),
            label: format!("{manager} {name}"),
            program: manager.to_string(),
            args: vec!["run".into(), name.clone()],
            cwd: project.to_path_buf(),
        });
    }
}

fn detect_python(project: &Path, chosen: Option<&Path>, d: &mut Detection) {
    let has_project = [
        "pyproject.toml",
        "requirements.txt",
        "setup.py",
        ".venv",
        "venv",
    ]
    .iter()
    .any(|f| project.join(f).exists());
    let entries: Vec<&str> = ["main.py", "app.py", "manage.py", "run.py"]
        .into_iter()
        .filter(|f| project.join(f).is_file())
        .collect();
    if !has_project && entries.is_empty() {
        return;
    }
    let interpreter = lang::python_interpreter(project, chosen);
    let (program, tag) = match &interpreter {
        Some(p) => (
            p.to_string_lossy().to_string(),
            p.strip_prefix(project)
                .map(|r| {
                    r.parent()
                        .and_then(Path::parent)
                        .map(|x| x.display().to_string())
                        .unwrap_or_default()
                })
                .unwrap_or_else(|_| p.display().to_string()),
        ),
        None => (
            crate::platform::DEFAULT_PYTHON.to_string(),
            crate::platform::DEFAULT_PYTHON.to_string(),
        ),
    };
    for e in entries {
        let mut args = vec![e.to_string()];
        if e == "manage.py" {
            args.push("runserver".into());
        }
        d.configs.push(RunConfig {
            id: format!("py:{e}"),
            label: format!("Python {e} ({tag})"),
            program: program.clone(),
            args,
            cwd: project.to_path_buf(),
        });
    }
}

/// Run configuration for one Python file (the file open in the editor).
/// Run configuration for a shell script open in the editor.
pub fn shell_file_config(project: &Path, file: &Path) -> RunConfig {
    let rel = file
        .strip_prefix(project)
        .unwrap_or(file)
        .to_string_lossy()
        .to_string();
    RunConfig {
        id: format!("shfile:{rel}"),
        label: format!("sh {rel}"),
        // On Windows, Git for Windows provides `bash` for shell scripts.
        program: if cfg!(windows) { "bash" } else { "/bin/sh" }.into(),
        args: vec![rel],
        cwd: project.to_path_buf(),
    }
}

fn simple(id: String, label: &str, program: &str, args: &[&str], cwd: &Path) -> RunConfig {
    RunConfig {
        id,
        label: label.to_string(),
        program: program.to_string(),
        args: args.iter().map(|a| a.to_string()).collect(),
        cwd: cwd.to_path_buf(),
    }
}

fn detect_swift_cmake(project: &Path, d: &mut Detection) {
    if project.join("Package.swift").is_file() {
        for (id, label, args) in [
            ("run", "swift run", &["run"]),
            ("build", "swift build", &["build"]),
            ("test", "swift test", &["test"]),
        ] {
            d.configs
                .push(simple(format!("swift:{id}"), label, "swift", args, project));
        }
    }
    if project.join("CMakeLists.txt").is_file() {
        d.configs.push(simple(
            "cmake:configure".into(),
            "cmake configure",
            "cmake",
            &["-S", ".", "-B", "build"],
            project,
        ));
        d.configs.push(simple(
            "cmake:build".into(),
            "cmake build",
            "cmake",
            &["--build", "build"],
            project,
        ));
    }
}

pub fn python_file_config(project: &Path, file: &Path, chosen: Option<&Path>) -> RunConfig {
    let program = lang::python_interpreter(project, chosen)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| crate::platform::DEFAULT_PYTHON.into());
    let rel = file
        .strip_prefix(project)
        .unwrap_or(file)
        .to_string_lossy()
        .to_string();
    RunConfig {
        id: format!("pyfile:{rel}"),
        label: format!("Python {rel}"),
        program,
        args: vec![rel],
        cwd: project.to_path_buf(),
    }
}

fn detect_cargo(project: &Path, d: &mut Detection) {
    if !project.join("Cargo.toml").is_file() {
        return;
    }
    for (id, label, args) in [
        ("run", "cargo run", vec!["run"]),
        ("test", "cargo test", vec!["test"]),
        ("build", "cargo build", vec!["build"]),
        ("check", "cargo check", vec!["check"]),
    ] {
        d.configs.push(RunConfig {
            id: format!("cargo:{id}"),
            label: label.to_string(),
            program: "cargo".into(),
            args: args.into_iter().map(String::from).collect(),
            cwd: project.to_path_buf(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(files: &[(&str, &str)]) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for (f, c) in files {
            let p = d.path().join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, c).unwrap();
        }
        d
    }

    #[test]
    fn package_manager_detection() {
        let d = dir(&[("package.json", "{}")]);
        assert_eq!(
            detect_package_manager(d.path()),
            PackageManagerChoice::Default
        );
        let d = dir(&[("package.json", "{}"), ("pnpm-lock.yaml", "")]);
        assert_eq!(
            detect_package_manager(d.path()),
            PackageManagerChoice::Detected("pnpm")
        );
        let d = dir(&[
            ("package.json", "{}"),
            ("pnpm-lock.yaml", ""),
            ("yarn.lock", ""),
        ]);
        assert_eq!(
            detect_package_manager(d.path()),
            PackageManagerChoice::Ambiguous(vec!["pnpm", "yarn"])
        );
        let d = dir(&[
            ("package.json", r#"{"packageManager":"yarn@4.1.0"}"#),
            ("pnpm-lock.yaml", ""),
            ("package-lock.json", ""),
        ]);
        assert_eq!(
            detect_package_manager(d.path()),
            PackageManagerChoice::Detected("yarn")
        );
    }

    #[test]
    fn js_scripts_become_configs_with_dev_first() {
        let d = dir(&[
            (
                "package.json",
                r#"{"scripts":{"a":"x","build":"b","dev":"vite"}}"#,
            ),
            ("yarn.lock", ""),
        ]);
        let det = detect(d.path(), None, None);
        assert_eq!(det.configs[0].label, "yarn dev");
        assert_eq!(det.configs[0].args, vec!["run", "dev"]);
        assert_eq!(det.configs.len(), 3);
    }

    #[test]
    fn ambiguous_managers_are_reported_until_chosen() {
        let d = dir(&[
            ("package.json", r#"{"scripts":{"dev":"vite"}}"#),
            ("yarn.lock", ""),
            ("package-lock.json", ""),
        ]);
        let det = detect(d.path(), None, None);
        assert_eq!(det.ambiguous_managers, vec!["yarn", "npm"]);
        let det = detect(d.path(), Some("npm"), None);
        assert!(det.ambiguous_managers.is_empty());
        assert_eq!(det.configs[0].program, "npm");
    }

    #[cfg(unix)]
    #[test]
    fn python_uses_venv() {
        let d = dir(&[("main.py", ""), (".venv/bin/python", "")]);
        let det = detect(d.path(), None, None);
        assert_eq!(
            det.configs[0].program,
            d.path().join(".venv/bin/python").to_string_lossy()
        );
        assert!(
            det.configs[0].label.contains(".venv"),
            "{}",
            det.configs[0].label
        );
        let d = dir(&[("main.py", "")]);
        assert_eq!(detect(d.path(), None, None).configs[0].program, "python3");
    }

    #[test]
    fn swift_and_cmake_projects() {
        let d = dir(&[("Package.swift", ""), ("CMakeLists.txt", "")]);
        let det = detect(d.path(), None, None);
        let ids: Vec<_> = det.configs.iter().map(|c| c.id.as_str()).collect();
        assert!(
            ids.contains(&"swift:run") && ids.contains(&"cmake:build"),
            "{ids:?}"
        );
    }

    #[test]
    fn cargo_and_broken_package_json() {
        let d = dir(&[("Cargo.toml", ""), ("package.json", "{broken")]);
        let det = detect(d.path(), None, None);
        assert!(det.configs.iter().all(|c| c.program == "cargo"));
        assert_eq!(det.configs[0].id, "cargo:run");
    }
}
