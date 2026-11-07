//! Language registry. Each language states separately what KERN can do with it:
//! file type detection, syntax highlighting, language server, and running.
//! "Supported" here says nothing about this Mac: whether a server is actually
//! present is a separate, runtime question (`platform::tools`).
pub mod lexers;

use crate::core::settings::{CustomLsp, Settings};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunSupport {
    None,
    PackageJson,
    Python,
    Cargo,
    Swift,
    CMake,
    Shell,
}

impl RunSupport {
    pub fn label(self) -> &'static str {
        match self {
            RunSupport::None => "—",
            RunSupport::PackageJson => "package.json scripts",
            RunSupport::Python => "python (selected interpreter)",
            RunSupport::Cargo => "cargo",
            RunSupport::Swift => "swift (Package.swift)",
            RunSupport::CMake => "cmake",
            RunSupport::Shell => "current script",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinServer {
    TypeScript,
    Python,
    Rust,
    Swift,
    Clang,
    Bash,
    Html,
    Css,
    Json,
    Yaml,
    Docker,
    CMake,
    CSharp,
}

/// Commands and install hint of a server. Selection logic for the three
/// project-aware servers lives in `server_for`.
pub struct ServerInfo {
    pub id: &'static str,
    pub commands: &'static [&'static str],
    pub args: &'static [&'static str],
    pub root_markers: &'static [&'static str],
    pub install_hint: &'static str,
}

impl BuiltinServer {
    pub fn info(self) -> ServerInfo {
        let s = |id, commands, args, root_markers, install_hint| ServerInfo {
            id,
            commands,
            args,
            root_markers,
            install_hint,
        };
        match self {
            BuiltinServer::TypeScript => s(
                "typescript",
                &["tsc", "typescript-language-server"],
                &["--stdio"],
                &["tsconfig.json", "jsconfig.json", "package.json"],
                "npm install -D typescript",
            ),
            BuiltinServer::Python => s(
                "pyright",
                &["pyright-langserver", "basedpyright-langserver"],
                &["--stdio"],
                &[
                    "pyproject.toml",
                    "pyrightconfig.json",
                    "setup.py",
                    "requirements.txt",
                ],
                "npm install -g pyright",
            ),
            BuiltinServer::Rust => s(
                "rust-analyzer",
                &["rust-analyzer"],
                &[],
                &["Cargo.toml"],
                "rustup component add rust-analyzer",
            ),
            BuiltinServer::Swift => s(
                "sourcekit-lsp",
                &["sourcekit-lsp"],
                &[],
                &["Package.swift", ".git"],
                "xcode-select --install",
            ),
            BuiltinServer::Clang => s(
                "clangd",
                &["clangd"],
                &[],
                &[
                    "compile_commands.json",
                    "compile_flags.txt",
                    "CMakeLists.txt",
                    ".git",
                ],
                "xcode-select --install",
            ),
            BuiltinServer::Bash => s(
                "bash-language-server",
                &["bash-language-server"],
                &["start"],
                &[".git"],
                "npm install -g bash-language-server",
            ),
            BuiltinServer::Html => s(
                "vscode-html-language-server",
                &["vscode-html-language-server"],
                &["--stdio"],
                &["package.json", ".git"],
                "npm install -g vscode-langservers-extracted",
            ),
            BuiltinServer::Css => s(
                "vscode-css-language-server",
                &["vscode-css-language-server"],
                &["--stdio"],
                &["package.json", ".git"],
                "npm install -g vscode-langservers-extracted",
            ),
            BuiltinServer::Json => s(
                "vscode-json-language-server",
                &["vscode-json-language-server"],
                &["--stdio"],
                &["package.json", ".git"],
                "npm install -g vscode-langservers-extracted",
            ),
            BuiltinServer::Yaml => s(
                "yaml-language-server",
                &["yaml-language-server"],
                &["--stdio"],
                &[".git"],
                "npm install -g yaml-language-server",
            ),
            BuiltinServer::Docker => s(
                "docker-langserver",
                &["docker-langserver"],
                &["--stdio"],
                &[".git"],
                "npm install -g dockerfile-language-server-nodejs",
            ),
            BuiltinServer::CSharp => s(
                "csharp-ls",
                &["csharp-ls"],
                &[],
                &[".git"],
                "dotnet tool install --global csharp-ls (needs the .NET SDK)",
            ),
            BuiltinServer::CMake => s(
                "cmake-language-server",
                &["cmake-language-server"],
                &[],
                &["CMakeLists.txt", ".git"],
                "pip install cmake-language-server",
            ),
        }
    }
}

/// How the text of a language gets its colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Highlight {
    TreeSitter,
    /// KERN's own line-based lexer (`lexers.rs`), for formats without a grammar.
    Lexer,
    None,
}

#[derive(Debug, Clone, Copy)]
pub struct Language {
    /// Stable id: settings refer to it.
    pub id: &'static str,
    pub display: &'static str,
    /// Name passed to the editor: a Tree-sitter grammar or a lexer.
    pub grammar: &'static str,
    /// LSP `languageId`.
    pub lsp_id: &'static str,
    pub extensions: &'static [&'static str],
    pub filenames: &'static [&'static str],
    /// File names starting with one of these, like `Dockerfile.dev`.
    pub prefixes: &'static [&'static str],
    pub highlight: Highlight,
    /// A note for the support matrix about partial coverage.
    pub note: &'static str,
    pub server: Option<BuiltinServer>,
    pub run: RunSupport,
}

#[allow(clippy::too_many_arguments)]
const fn ts(
    id: &'static str,
    display: &'static str,
    grammar: &'static str,
    lsp_id: &'static str,
    extensions: &'static [&'static str],
    filenames: &'static [&'static str],
    server: Option<BuiltinServer>,
    run: RunSupport,
) -> Language {
    Language {
        id,
        display,
        grammar,
        lsp_id,
        extensions,
        filenames,
        prefixes: &[],
        highlight: Highlight::TreeSitter,
        note: "",
        server,
        run,
    }
}

use BuiltinServer as S;
use RunSupport as R;

pub const LANGUAGES: &[Language] = &[
    ts(
        "javascript",
        "JavaScript",
        "javascript",
        "javascript",
        &["js", "mjs", "cjs"],
        &[],
        Some(S::TypeScript),
        R::PackageJson,
    ),
    ts(
        "jsx",
        "JSX",
        "javascript",
        "javascriptreact",
        &["jsx"],
        &[],
        Some(S::TypeScript),
        R::PackageJson,
    ),
    ts(
        "typescript",
        "TypeScript",
        "typescript",
        "typescript",
        &["ts", "mts", "cts"],
        &[],
        Some(S::TypeScript),
        R::PackageJson,
    ),
    ts(
        "tsx",
        "TSX",
        "tsx",
        "typescriptreact",
        &["tsx"],
        &[],
        Some(S::TypeScript),
        R::PackageJson,
    ),
    ts(
        "python",
        "Python",
        "python",
        "python",
        &["py", "pyi"],
        &[],
        Some(S::Python),
        R::Python,
    ),
    ts(
        "rust",
        "Rust",
        "rust",
        "rust",
        &["rs"],
        &[],
        Some(S::Rust),
        R::Cargo,
    ),
    ts(
        "swift",
        "Swift",
        "swift",
        "swift",
        &["swift"],
        &[],
        Some(S::Swift),
        R::Swift,
    ),
    ts(
        "c",
        "C",
        "c",
        "c",
        &["c", "h"],
        &[],
        Some(S::Clang),
        R::CMake,
    ),
    ts(
        "cpp",
        "C++",
        "cpp",
        "cpp",
        &["cpp", "cc", "cxx", "c++", "hpp", "hh", "hxx", "mm"],
        &[],
        Some(S::Clang),
        R::CMake,
    ),
    ts(
        "csharp",
        "C#",
        "csharp",
        "csharp",
        &["cs", "csx"],
        &[],
        Some(S::CSharp),
        R::None,
    ),
    ts(
        "cmake",
        "CMake",
        "cmake",
        "cmake",
        &["cmake"],
        &["CMakeLists.txt"],
        Some(S::CMake),
        R::CMake,
    ),
    ts(
        "shell",
        "Shell",
        "bash",
        "shellscript",
        &["sh", "bash", "zsh"],
        &[
            ".bashrc",
            ".zshrc",
            ".profile",
            ".bash_profile",
            ".zprofile",
        ],
        Some(S::Bash),
        R::Shell,
    ),
    ts(
        "html",
        "HTML",
        "html",
        "html",
        &["html", "htm"],
        &[],
        Some(S::Html),
        R::None,
    ),
    ts(
        "css",
        "CSS",
        "css",
        "css",
        &["css", "scss"],
        &[],
        Some(S::Css),
        R::None,
    ),
    ts(
        "json",
        "JSON",
        "json",
        "json",
        &["json", "jsonc"],
        &[],
        Some(S::Json),
        R::None,
    ),
    ts(
        "yaml",
        "YAML",
        "yaml",
        "yaml",
        &["yaml", "yml"],
        &[],
        Some(S::Yaml),
        R::None,
    ),
    ts(
        "toml",
        "TOML",
        "toml",
        "toml",
        &["toml"],
        &["Cargo.lock"],
        None,
        R::None,
    ),
    ts(
        "markdown",
        "Markdown",
        "markdown",
        "markdown",
        &["md", "markdown", "mdx"],
        &[],
        None,
        R::None,
    ),
    Language {
        id: "inno",
        display: "Inno Setup",
        grammar: "inno",
        lsp_id: "innosetup",
        extensions: &["iss", "isl"],
        filenames: &[],
        prefixes: &[],
        highlight: Highlight::Lexer,
        note: "[Code] section: basic Pascal keywords, strings and comments only",
        server: None,
        run: R::None,
    },
    Language {
        id: "dockerfile",
        display: "Dockerfile",
        grammar: "dockerfile",
        lsp_id: "dockerfile",
        extensions: &["dockerfile"],
        filenames: &["Dockerfile", "Containerfile"],
        prefixes: &["Dockerfile."],
        highlight: Highlight::Lexer,
        note: "",
        server: Some(S::Docker),
        run: R::None,
    },
    Language {
        id: "dotenv",
        display: "Env File",
        grammar: "dotenv",
        lsp_id: "dotenv",
        extensions: &["env"],
        filenames: &[".env"],
        prefixes: &[".env."],
        highlight: Highlight::Lexer,
        note: "",
        server: None,
        run: R::None,
    },
    Language {
        id: "ignore",
        display: "Ignore File",
        grammar: "ignore",
        lsp_id: "ignore",
        extensions: &["gitignore", "dockerignore"],
        filenames: &[
            ".gitignore",
            ".dockerignore",
            ".npmignore",
            ".eslintignore",
            ".prettierignore",
            ".kernignore",
        ],
        prefixes: &[],
        highlight: Highlight::Lexer,
        note: "",
        server: None,
        run: R::None,
    },
];

pub const PLAIN: Language = Language {
    id: "text",
    display: "Plain Text",
    grammar: "text",
    lsp_id: "plaintext",
    extensions: &[],
    filenames: &[],
    prefixes: &[],
    highlight: Highlight::None,
    note: "",
    server: None,
    run: RunSupport::None,
};

pub fn by_id(id: &str) -> Option<Language> {
    if id == PLAIN.id {
        return Some(PLAIN);
    }
    LANGUAGES.iter().find(|l| l.id == id).copied()
}

/// Built-in detection from the file name alone.
pub fn detect(path: &Path) -> Language {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if let Some(l) = LANGUAGES.iter().find(|l| l.filenames.contains(&name)) {
        return *l;
    }
    if let Some(l) = LANGUAGES
        .iter()
        .find(|l| l.prefixes.iter().any(|p| name.starts_with(p)))
    {
        return *l;
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    LANGUAGES
        .iter()
        .find(|l| l.extensions.contains(&ext.as_str()))
        .copied()
        .unwrap_or(PLAIN)
}

/// The association key for a file: `*.ext`, or the exact name without extension.
pub fn association_key(path: &Path) -> String {
    match path.extension().and_then(|e| e.to_str()).filter(|_| {
        !path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .starts_with('.')
    }) {
        Some(ext) => format!("*.{}", ext.to_lowercase()),
        None => path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string(),
    }
}

/// Detection with user choices: the per-file override of this project, then
/// associations by exact name, then by extension, then the built-in rules.
pub fn resolve(path: &Path, project: Option<&Path>, settings: &Settings) -> Language {
    if let Some(root) = project {
        let rel = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();
        if let Some(id) = settings.session(root).language_overrides.get(&rel)
            && let Some(l) = by_id(id)
        {
            return l;
        }
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if let Some(l) = settings
        .file_associations
        .get(name)
        .and_then(|id| by_id(id))
    {
        return l;
    }
    if let Some(ext) = path.extension().and_then(|e| e.to_str())
        && let Some(l) = settings
            .file_associations
            .get(&format!("*.{}", ext.to_lowercase()))
            .and_then(|id| by_id(id))
    {
        return l;
    }
    detect(path)
}

/// How to launch one language server.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerConfig {
    pub id: String,
    /// Absolute path or tool name; `candidates` are tried in order.
    pub candidates: Vec<String>,
    pub args: Vec<String>,
    pub root: PathBuf,
    pub init_options: serde_json::Value,
    pub settings: serde_json::Value,
    pub install_hint: String,
}

/// Walk up from `file` up to `project` looking for marker files.
pub fn find_root(file: &Path, project: &Path, markers: &[&str]) -> Option<PathBuf> {
    let mut dir = file.parent();
    while let Some(d) = dir {
        if markers.iter().any(|m| d.join(m).exists()) {
            return Some(d.to_path_buf());
        }
        if d == project {
            break;
        }
        dir = d.parent();
    }
    None
}

/// The Cargo workspace root: the outermost manifest declaring `[workspace]`,
/// else the nearest manifest. May be above the opened folder.
pub fn cargo_root(file: &Path) -> Option<PathBuf> {
    let mut nearest = None;
    let mut workspace = None;
    let mut dir = file.parent();
    while let Some(d) = dir {
        let manifest = d.join("Cargo.toml");
        if manifest.is_file() {
            if nearest.is_none() {
                nearest = Some(d.to_path_buf());
            }
            if std::fs::read_to_string(&manifest)
                .is_ok_and(|t| t.lines().any(|l| l.trim_start().starts_with("[workspace")))
            {
                workspace = Some(d.to_path_buf());
            }
        }
        dir = d.parent();
    }
    workspace.or(nearest)
}

/// Python interpreter: explicit choice, then `.venv`, then `venv`, then `python3`.
pub fn python_interpreter(project: &Path, chosen: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = chosen {
        return Some(p.to_path_buf());
    }
    for dir in [".venv", "venv", "env"] {
        let p = project.join(dir).join(crate::platform::VENV_PYTHON);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

fn custom_for(path: &Path, custom: &[CustomLsp]) -> Option<CustomLsp> {
    let ext = path.extension().and_then(|e| e.to_str())?.to_lowercase();
    custom
        .iter()
        .find(|c| {
            c.extensions
                .iter()
                .any(|e| e.trim_start_matches('.').eq_ignore_ascii_case(&ext))
        })
        .cloned()
}

/// Major version of a `typescript` package directory.
pub fn typescript_major(package_dir: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(package_dir.join("package.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    if json.get("name")?.as_str()? != "typescript" {
        return None;
    }
    json.get("version")?
        .as_str()?
        .split('.')
        .next()?
        .parse()
        .ok()
}

/// The `typescript` package an installed `tsc` command belongs to.
fn package_of_tool(tool: &Path) -> Option<PathBuf> {
    let real = std::fs::canonicalize(tool).ok()?;
    real.ancestors()
        .skip(1)
        .find(|d| typescript_major(d).is_some())
        .map(Path::to_path_buf)
}

/// TypeScript 7+ is a native compiler with a built-in language server and no
/// `tsserver.js`; earlier versions go through `typescript-language-server`.
/// The project's own TypeScript always wins, because it is what the project
/// compiles with and what loads tsconfig plugins such as Next.js'.
fn typescript_server(path: &Path, project: &Path) -> ServerConfig {
    let root = find_root(
        path,
        project,
        &["tsconfig.json", "jsconfig.json", "package.json"],
    )
    .unwrap_or_else(|| project.to_path_buf());
    let native = |tsc: String, root: PathBuf| ServerConfig {
        id: "typescript-native".into(),
        candidates: vec![tsc],
        args: vec!["--lsp".into(), "--stdio".into()],
        root,
        init_options: serde_json::Value::Null,
        settings: serde_json::Value::Null,
        install_hint: "npm install -g typescript".into(),
    };
    let mut init = serde_json::json!({"hostInfo": "KERN", "preferences": {"includeCompletionsForModuleExports": true}});
    let mut candidates = vec![];
    // Look in the nearest root and the project folder (monorepos hoist packages).
    for base in [root.as_path(), project] {
        let package = base.join("node_modules/typescript");
        let Some(major) = typescript_major(&package) else {
            continue;
        };
        if major >= 7 {
            return native(
                base.join("node_modules/.bin/tsc")
                    .to_string_lossy()
                    .to_string(),
                root.clone(),
            );
        }
        init["tsserver"] = serde_json::json!({"path": package.join("lib")});
        let local = base.join("node_modules/.bin/typescript-language-server");
        if local.exists() {
            candidates.push(local.to_string_lossy().to_string());
        }
        break;
    }
    if init.get("tsserver").is_none() {
        // No project TypeScript: use a global install, whichever generation it is.
        if let Some(tsc) = crate::platform::tools::find("tsc")
            && let Some(package) = package_of_tool(&tsc)
        {
            match typescript_major(&package) {
                Some(m) if m >= 7 => return native(tsc.to_string_lossy().to_string(), root),
                Some(_) => init["tsserver"] = serde_json::json!({"path": package.join("lib")}),
                None => {}
            }
        }
    }
    candidates.push("typescript-language-server".to_string());
    ServerConfig {
        id: "typescript-language-server".into(),
        candidates,
        args: vec!["--stdio".into()],
        root,
        init_options: init,
        settings: serde_json::Value::Null,
        install_hint: "npm install -g typescript@5 typescript-language-server".into(),
    }
}

/// Which server handles this file. User-defined servers take precedence.
pub fn server_for(path: &Path, project: &Path, settings: &Settings) -> Option<ServerConfig> {
    server_for_language(
        path,
        project,
        settings,
        resolve(path, Some(project), settings),
    )
}

pub fn server_for_language(
    path: &Path,
    project: &Path,
    settings: &Settings,
    language: Language,
) -> Option<ServerConfig> {
    if let Some(c) = custom_for(path, &settings.custom_lsp) {
        let markers: Vec<&str> = c.root_markers.iter().map(String::as_str).collect();
        let root = find_root(path, project, &markers).unwrap_or_else(|| project.to_path_buf());
        return Some(ServerConfig {
            id: format!("custom:{}", c.name),
            candidates: vec![c.command.clone()],
            args: c.args.clone(),
            root,
            init_options: serde_json::Value::Null,
            settings: serde_json::Value::Null,
            install_hint: format!("Check the command for \"{}\" in Settings", c.name),
        });
    }
    let session = settings.session(project);
    Some(match language.server? {
        BuiltinServer::TypeScript => typescript_server(path, project),
        BuiltinServer::Python => {
            let root = find_root(
                path,
                project,
                &[
                    "pyproject.toml",
                    "pyrightconfig.json",
                    "setup.py",
                    "requirements.txt",
                ],
            )
            .unwrap_or_else(|| project.to_path_buf());
            let interpreter = python_interpreter(&root, session.python.as_deref())
                .or_else(|| python_interpreter(project, None));
            let mut settings_json = serde_json::json!({"python": {"analysis": {"autoSearchPaths": true, "useLibraryCodeForTypes": true}}});
            if let Some(i) = &interpreter {
                settings_json["python"]["pythonPath"] = serde_json::json!(i);
            }
            ServerConfig {
                id: "pyright".into(),
                candidates: vec![
                    "pyright-langserver".into(),
                    "basedpyright-langserver".into(),
                ],
                args: vec!["--stdio".into()],
                root,
                init_options: serde_json::Value::Null,
                settings: settings_json,
                install_hint: "npm install -g pyright".into(),
            }
        }
        BuiltinServer::Rust => ServerConfig {
            id: "rust-analyzer".into(),
            candidates: vec!["rust-analyzer".into()],
            args: vec![],
            root: cargo_root(path).unwrap_or_else(|| project.to_path_buf()),
            init_options: serde_json::Value::Null,
            settings: serde_json::Value::Null,
            install_hint: "rustup component add rust-analyzer".into(),
        },
        other => {
            let info = other.info();
            let root = find_root(path, project, info.root_markers)
                .unwrap_or_else(|| project.to_path_buf());
            ServerConfig {
                id: info.id.to_string(),
                candidates: info.commands.iter().map(|c| c.to_string()).collect(),
                args: info.args.iter().map(|c| c.to_string()).collect(),
                root,
                init_options: serde_json::Value::Null,
                settings: serde_json::Value::Null,
                install_hint: info.install_hint.to_string(),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_and_capabilities() {
        assert_eq!(detect(Path::new("App.tsx")).id, "tsx");
        assert_eq!(detect(Path::new("App.tsx")).lsp_id, "typescriptreact");
        assert_eq!(detect(Path::new("a.JSX")).lsp_id, "javascriptreact");
        assert_eq!(detect(Path::new("Cargo.lock")).id, "toml");
        let unknown = detect(Path::new("weird.zzz"));
        assert_eq!(unknown.id, "text");
        assert_eq!(unknown.highlight, Highlight::None);
        assert!(unknown.server.is_none());
        assert_eq!(detect(Path::new("x.css")).highlight, Highlight::TreeSitter);
        for (name, id) in [
            ("setup.iss", "inno"),
            ("a.swift", "swift"),
            ("a.c", "c"),
            ("a.h", "c"),
            ("a.cpp", "cpp"),
            ("a.hpp", "cpp"),
            ("Program.cs", "csharp"),
            ("Script.CSX", "csharp"),
            ("CMakeLists.txt", "cmake"),
            ("x.cmake", "cmake"),
            ("build.sh", "shell"),
            (".zshrc", "shell"),
            ("build.mjs", "javascript"),
            ("Dockerfile", "dockerfile"),
            ("Dockerfile.dev", "dockerfile"),
            ("api.dockerfile", "dockerfile"),
            (".env", "dotenv"),
            (".env.example", "dotenv"),
            (".gitignore", "ignore"),
            (".dockerignore", "ignore"),
        ] {
            assert_eq!(detect(Path::new(name)).id, id, "{name}");
        }
        assert_eq!(detect(Path::new("x.py")).run, RunSupport::Python);
    }

    #[test]
    fn cargo_workspace_root_wins() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("crates/a/src")).unwrap();
        std::fs::write(
            d.path().join("Cargo.toml"),
            "[workspace]\nmembers=[\"crates/*\"]\n",
        )
        .unwrap();
        std::fs::write(
            d.path().join("crates/a/Cargo.toml"),
            "[package]\nname=\"a\"\n",
        )
        .unwrap();
        let file = d.path().join("crates/a/src/lib.rs");
        assert_eq!(cargo_root(&file).unwrap(), d.path());
    }

    #[cfg(unix)]
    #[test]
    fn python_prefers_chosen_then_venv() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(python_interpreter(d.path(), None), None);
        std::fs::create_dir_all(d.path().join(".venv/bin")).unwrap();
        std::fs::write(d.path().join(".venv/bin/python"), "").unwrap();
        assert_eq!(
            python_interpreter(d.path(), None).unwrap(),
            d.path().join(".venv/bin/python")
        );
        assert_eq!(
            python_interpreter(d.path(), Some(Path::new("/x/py"))).unwrap(),
            PathBuf::from("/x/py")
        );
        let mut s = Settings::default();
        let cfg = server_for(&d.path().join("a.py"), d.path(), &s).unwrap();
        assert_eq!(
            cfg.settings["python"]["pythonPath"],
            serde_json::json!(d.path().join(".venv/bin/python"))
        );
        s.custom_lsp.push(CustomLsp {
            name: "my".into(),
            command: "my-ls".into(),
            args: vec!["--x".into()],
            extensions: vec![".py".into()],
            root_markers: vec![],
        });
        assert_eq!(
            server_for(&d.path().join("a.py"), d.path(), &s).unwrap().id,
            "custom:my"
        );
    }

    #[test]
    fn typescript_uses_project_tsserver() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("web/src")).unwrap();
        std::fs::create_dir_all(d.path().join("web/node_modules/typescript/lib")).unwrap();
        std::fs::write(d.path().join("web/tsconfig.json"), "{}").unwrap();
        std::fs::write(
            d.path().join("web/node_modules/typescript/package.json"),
            r#"{"name":"typescript","version":"5.9.3"}"#,
        )
        .unwrap();
        let cfg = server_for(
            &d.path().join("web/src/App.tsx"),
            d.path(),
            &Settings::default(),
        )
        .unwrap();
        assert_eq!(cfg.root, d.path().join("web"));
        assert!(
            cfg.init_options["tsserver"]["path"]
                .as_str()
                .unwrap()
                .ends_with("typescript/lib")
        );
        assert!(server_for(Path::new("/x/y.toml"), d.path(), &Settings::default()).is_none());
    }

    #[test]
    fn user_associations_and_overrides_win() {
        let mut s = Settings::default();
        let root = Path::new("/p");
        assert_eq!(resolve(Path::new("/p/a.conf"), Some(root), &s).id, "text");
        s.file_associations.insert("*.conf".into(), "toml".into());
        assert_eq!(resolve(Path::new("/p/a.conf"), Some(root), &s).id, "toml");
        s.file_associations
            .insert("special.conf".into(), "yaml".into());
        assert_eq!(
            resolve(Path::new("/p/special.conf"), Some(root), &s).id,
            "yaml"
        );
        s.session_mut(root)
            .language_overrides
            .insert("a.conf".into(), "json".into());
        assert_eq!(resolve(Path::new("/p/a.conf"), Some(root), &s).id, "json");
        assert_eq!(association_key(Path::new("x/a.CONF")), "*.conf");
        assert_eq!(association_key(Path::new("x/Justfile")), "Justfile");
        assert_eq!(association_key(Path::new("x/.env")), ".env");
    }

    #[test]
    fn typescript_7_uses_its_native_language_server() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("node_modules/typescript")).unwrap();
        std::fs::write(
            d.path().join("node_modules/typescript/package.json"),
            r#"{"name":"typescript","version":"7.0.2"}"#,
        )
        .unwrap();
        std::fs::write(d.path().join("package.json"), "{}").unwrap();
        let cfg = server_for(&d.path().join("App.tsx"), d.path(), &Settings::default()).unwrap();
        assert_eq!(cfg.id, "typescript-native");
        assert_eq!(cfg.args, vec!["--lsp", "--stdio"]);
        assert!(cfg.candidates[0].ends_with("node_modules/.bin/tsc"));
    }
}
