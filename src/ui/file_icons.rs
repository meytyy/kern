//! The one registry of file and folder icons, used by the Explorer and editor
//! tabs. Exact names and patterns win over extensions, so `vite.config.ts` is Vite
//! and not TypeScript. Tints come from the palette, so both themes stay readable.
use super::theme::Palette;
use gpui_kit::Hsla;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tint {
    Folder,
    Blue,
    Cyan,
    Green,
    Yellow,
    Orange,
    Red,
    Pink,
    Purple,
    Gray,
}

impl Tint {
    pub fn color(self, p: Palette) -> Hsla {
        match self {
            Tint::Folder => p.tint_folder,
            Tint::Blue => p.tint_blue,
            Tint::Cyan => p.tint_cyan,
            Tint::Green => p.tint_green,
            Tint::Yellow => p.tint_yellow,
            Tint::Orange => p.tint_orange,
            Tint::Red => p.tint_red,
            Tint::Pink => p.tint_pink,
            Tint::Purple => p.tint_purple,
            Tint::Gray => p.muted,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileIcon {
    pub svg: &'static str,
    pub tint: Tint,
}

const fn ic(svg: &'static str, tint: Tint) -> FileIcon {
    FileIcon { svg, tint }
}

pub fn for_folder(name: &str, expanded: bool) -> FileIcon {
    let lower = name.to_lowercase();
    macro_rules! pick {
        ($closed:literal, $open:literal, $tint:expr) => {
            ic(if expanded { $open } else { $closed }, $tint)
        };
    }
    match lower.as_str() {
        "src" | "source" | "sources" => {
            pick!("ft-folder-closed-src", "ft-folder-open-src", Tint::Blue)
        }
        "public" | "static" | "www" => pick!(
            "ft-folder-closed-public",
            "ft-folder-open-public",
            Tint::Cyan
        ),
        "assets" | "asset" | "resources" => pick!(
            "ft-folder-closed-assets",
            "ft-folder-open-assets",
            Tint::Pink
        ),
        "tests" | "test" | "__tests__" | "spec" | "e2e" => {
            pick!(
                "ft-folder-closed-tests",
                "ft-folder-open-tests",
                Tint::Green
            )
        }
        "server" | "backend" | "api" => pick!(
            "ft-folder-closed-server",
            "ft-folder-open-server",
            Tint::Orange
        ),
        "dist" | "build" | "out" | "target" => {
            pick!("ft-folder-closed-dist", "ft-folder-open-dist", Tint::Yellow)
        }
        "node_modules" => pick!("ft-folder-closed-node", "ft-folder-open-node", Tint::Red),
        _ => pick!("ft-folder-closed", "ft-folder-open", Tint::Folder),
    }
}

/// Exact names and patterns, checked before the extension.
fn by_name(lower: &str) -> Option<FileIcon> {
    let pattern = |prefix: &str, suffixes: &[&str]| {
        lower.starts_with(prefix) && suffixes.iter().any(|s| lower.ends_with(s))
    };
    Some(match lower {
        ".gitignore" | ".gitattributes" | ".gitmodules" | ".gitkeep" | ".mailmap" => {
            ic("ft-git", Tint::Orange)
        }
        "readme" | "readme.md" | "readme.txt" | "readme.rst" | "changelog.md"
        | "contributing.md" => ic("ft-readme", Tint::Blue),
        "license" | "license.md" | "license.txt" | "licence" | "copying" => {
            ic("ft-license", Tint::Yellow)
        }
        "package.json" | "cargo.toml" | "pyproject.toml" | "gemfile" | "go.mod"
        | "composer.json" | "package.swift" => ic("ft-package", Tint::Red),
        "package-lock.json"
        | "npm-shrinkwrap.json"
        | "pnpm-lock.yaml"
        | "yarn.lock"
        | "bun.lock"
        | "bun.lockb"
        | "cargo.lock"
        | "poetry.lock"
        | "gemfile.lock"
        | "go.sum"
        | "composer.lock"
        | "uv.lock" => ic("ft-package-lock", Tint::Red),
        "tsconfig.json" => ic("ft-tsconfig", Tint::Blue),
        "jsconfig.json" => ic("ft-jsconfig", Tint::Yellow),
        "dockerfile"
        | ".dockerignore"
        | "docker-compose.yml"
        | "docker-compose.yaml"
        | "compose.yml"
        | "compose.yaml" => ic("ft-docker", Tint::Blue),
        "makefile" | "gnumakefile" | "justfile" => ic("ft-make", Tint::Orange),
        "cmakelists.txt" => ic("ft-cmake", Tint::Green),
        ".env" => ic("ft-key", Tint::Yellow),
        ".editorconfig" | ".prettierrc" | ".eslintrc" | ".npmrc" | ".nvmrc" | ".babelrc"
        | ".swiftlint.yml" | ".clang-format" | "rustfmt.toml" | "clippy.toml" => {
            ic("ft-config", Tint::Gray)
        }
        _ if pattern(
            "vite.config.",
            &[".ts", ".js", ".mjs", ".cjs", ".mts", ".cts"],
        ) || pattern("vitest.config.", &[".ts", ".js", ".mjs", ".mts"]) =>
        {
            ic("ft-vite", Tint::Purple)
        }
        _ if pattern("tsconfig.", &[".json"]) || pattern("tsconfig-", &[".json"]) => {
            ic("ft-tsconfig", Tint::Blue)
        }
        _ if pattern("dockerfile.", &[""]) || lower.ends_with(".dockerfile") => {
            ic("ft-docker", Tint::Blue)
        }
        _ if pattern("docker-compose", &[".yml", ".yaml"]) => ic("ft-docker", Tint::Blue),
        // Templates hold names, not values: settings, and not a key.
        _ if pattern(".env.", &[".example", ".sample", ".template", ".dist"]) => {
            ic("ft-env", Tint::Green)
        }
        _ if lower.starts_with(".env.") || lower.ends_with(".env") => ic("ft-key", Tint::Yellow),
        _ if lower.ends_with(".d.ts") => ic("ft-dts", Tint::Blue),
        _ if pattern(
            "",
            &[
                ".test.ts",
                ".test.tsx",
                ".test.js",
                ".test.jsx",
                ".spec.ts",
                ".spec.tsx",
                ".spec.js",
                ".spec.jsx",
            ],
        ) =>
        {
            ic("ft-test", Tint::Green)
        }
        _ if pattern(
            "",
            &[
                ".config.js",
                ".config.ts",
                ".config.mjs",
                ".config.cjs",
                "rc.json",
                "rc.js",
                "rc.yml",
                "rc.yaml",
            ],
        ) =>
        {
            ic("ft-config", Tint::Gray)
        }
        _ => return None,
    })
}

pub fn for_file(name: &str) -> FileIcon {
    let lower = name.to_lowercase();
    if let Some(found) = by_name(&lower) {
        return found;
    }
    let ext = lower.rsplit_once('.').map_or("", |(_, e)| e);
    match ext {
        "js" | "mjs" | "cjs" => ic("ft-js", Tint::Yellow),
        "ts" | "mts" | "cts" => ic("ft-ts", Tint::Blue),
        "jsx" => ic("ft-react", Tint::Cyan),
        "tsx" => ic("ft-react", Tint::Blue),
        "py" | "pyi" | "pyw" => ic("ft-py", Tint::Green),
        "rs" => ic("ft-rust", Tint::Orange),
        "swift" => ic("ft-swift", Tint::Orange),
        "c" => ic("ft-c", Tint::Blue),
        "cc" | "cpp" | "cxx" | "c++" | "m" | "mm" => ic("ft-cpp", Tint::Purple),
        "h" | "hh" | "hpp" | "hxx" => ic("ft-h", Tint::Purple),
        "go" => ic("ft-go", Tint::Cyan),
        "java" => ic("ft-java", Tint::Red),
        "kt" | "kts" => ic("ft-kotlin", Tint::Purple),
        "rb" => ic("ft-ruby", Tint::Red),
        "php" => ic("ft-php", Tint::Purple),
        "lua" => ic("ft-lua", Tint::Blue),
        "html" | "htm" | "xhtml" | "xml" | "vue" | "svelte" => ic("ft-html", Tint::Orange),
        "css" | "scss" | "sass" | "less" => ic("ft-css", Tint::Pink),
        "json" | "jsonc" | "json5" | "webmanifest" => ic("ft-json", Tint::Yellow),
        "yaml" | "yml" => ic("ft-yaml", Tint::Purple),
        "toml" => ic("ft-toml", Tint::Orange),
        "ini" | "cfg" | "conf" | "properties" | "plist" => ic("ft-config", Tint::Gray),
        "md" | "mdx" | "markdown" => ic("ft-md", Tint::Blue),
        "txt" | "rtf" | "log" | "rst" => ic("ft-text", Tint::Gray),
        "sh" | "bash" | "zsh" | "fish" | "command" => ic("ft-shell", Tint::Green),
        "iss" | "isl" => ic("ft-inno", Tint::Cyan),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" | "icns" | "bmp" | "tiff"
        | "avif" | "heic" => ic("ft-image", Tint::Pink),
        "mp4" | "mov" | "webm" | "mkv" | "avi" | "m4v" => ic("ft-video", Tint::Purple),
        "sql" | "db" | "sqlite" | "sqlite3" => ic("ft-database", Tint::Cyan),
        "pem" | "key" | "p12" | "pfx" | "crt" | "cer" => ic("ft-key", Tint::Yellow),
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "7z" | "rar" | "dmg" => {
            ic("ft-archive", Tint::Gray)
        }
        "lock" => ic("ft-package-lock", Tint::Gray),
        _ => ic("ft-file", Tint::Gray),
    }
}

/// Keyboard movement in a list of visible tree rows. Pure, so that it can be tested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeKey {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeMove {
    Select(usize),
    Expand(usize),
    Collapse(usize),
    Nothing,
}

/// `rows`: (depth, is_dir, expanded) of each visible row.
pub fn tree_move(rows: &[(usize, bool, bool)], selected: Option<usize>, key: TreeKey) -> TreeMove {
    if rows.is_empty() {
        return TreeMove::Nothing;
    }
    let Some(ix) = selected else {
        return TreeMove::Select(0);
    };
    let (depth, is_dir, expanded) = rows[ix];
    match key {
        TreeKey::Up if ix > 0 => TreeMove::Select(ix - 1),
        TreeKey::Down if ix + 1 < rows.len() => TreeMove::Select(ix + 1),
        TreeKey::Right if is_dir && !expanded => TreeMove::Expand(ix),
        TreeKey::Right if is_dir && ix + 1 < rows.len() && rows[ix + 1].0 > depth => {
            TreeMove::Select(ix + 1)
        }
        TreeKey::Left if is_dir && expanded => TreeMove::Collapse(ix),
        TreeKey::Left => rows[..ix]
            .iter()
            .rposition(|r| r.0 < depth)
            .map_or(TreeMove::Nothing, TreeMove::Select),
        _ => TreeMove::Nothing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svg(name: &str) -> &'static str {
        for_file(name).svg
    }

    #[test]
    fn names_win_over_extensions() {
        assert_eq!(svg("vite.config.ts"), "ft-vite");
        assert_eq!(svg("vite.config.mjs"), "ft-vite");
        assert_eq!(svg("tsconfig.json"), "ft-tsconfig");
        assert_eq!(svg("tsconfig.node.json"), "ft-tsconfig");
        assert_eq!(svg("package.json"), "ft-package");
        assert_eq!(svg("package-lock.json"), "ft-package-lock");
        assert_eq!(svg("pnpm-lock.yaml"), "ft-package-lock");
        assert_eq!(svg("yarn.lock"), "ft-package-lock");
        assert_eq!(svg("Cargo.lock"), "ft-package-lock");
        assert_eq!(svg(".gitignore"), "ft-git");
        assert_eq!(svg(".env.example"), "ft-env");
        assert_eq!(svg(".env"), "ft-key");
        assert_eq!(svg(".env.local"), "ft-key");
        assert_eq!(svg("README.md"), "ft-readme");
        assert_eq!(svg("Dockerfile"), "ft-docker");
        assert_eq!(svg("docker-compose.local.yml"), "ft-docker");
        assert_eq!(svg("CMakeLists.txt"), "ft-cmake");
        assert_eq!(svg("types.d.ts"), "ft-dts");
        assert_eq!(svg("a.test.tsx"), "ft-test");
    }

    #[test]
    fn extensions_cover_the_required_set() {
        for (name, want) in [
            ("a.js", "ft-js"),
            ("a.mjs", "ft-js"),
            ("a.ts", "ft-ts"),
            ("a.jsx", "ft-react"),
            ("a.tsx", "ft-react"),
            ("a.py", "ft-py"),
            ("a.rs", "ft-rust"),
            ("a.swift", "ft-swift"),
            ("a.c", "ft-c"),
            ("a.cpp", "ft-cpp"),
            ("a.h", "ft-h"),
            ("a.html", "ft-html"),
            ("a.css", "ft-css"),
            ("a.json", "ft-json"),
            ("a.yaml", "ft-yaml"),
            ("a.toml", "ft-toml"),
            ("a.md", "ft-md"),
            ("a.sh", "ft-shell"),
            ("a.iss", "ft-inno"),
            ("A.PNG", "ft-image"),
            ("a.svg", "ft-image"),
            ("a.mov", "ft-video"),
            ("a.xyz", "ft-file"),
            ("noextension", "ft-file"),
        ] {
            assert_eq!(svg(name), want, "{name}");
        }
    }

    #[test]
    fn folders_have_variants_and_two_states() {
        assert_eq!(for_folder("frontend", false).svg, "ft-folder-closed");
        assert_eq!(for_folder("frontend", true).svg, "ft-folder-open");
        for (name, closed, open) in [
            ("src", "ft-folder-closed-src", "ft-folder-open-src"),
            ("public", "ft-folder-closed-public", "ft-folder-open-public"),
            ("assets", "ft-folder-closed-assets", "ft-folder-open-assets"),
            ("tests", "ft-folder-closed-tests", "ft-folder-open-tests"),
            ("server", "ft-folder-closed-server", "ft-folder-open-server"),
            ("dist", "ft-folder-closed-dist", "ft-folder-open-dist"),
            (
                "node_modules",
                "ft-folder-closed-node",
                "ft-folder-open-node",
            ),
        ] {
            assert_eq!(for_folder(name, false).svg, closed);
            assert_eq!(for_folder(name, true).svg, open);
        }
    }

    #[test]
    fn every_icon_in_the_registry_is_embedded() {
        let mut names = vec![];
        for n in [
            "vite.config.ts",
            "tsconfig.json",
            "jsconfig.json",
            "package.json",
            "yarn.lock",
            ".gitignore",
            ".env",
            ".env.example",
            "README.md",
            "LICENSE",
            "Dockerfile",
            "Makefile",
            "CMakeLists.txt",
            ".editorconfig",
            "a.d.ts",
            "a.test.ts",
            "a.js",
            "a.ts",
            "a.jsx",
            "a.py",
            "a.rs",
            "a.swift",
            "a.c",
            "a.cpp",
            "a.h",
            "a.go",
            "a.java",
            "a.kt",
            "a.rb",
            "a.php",
            "a.lua",
            "a.html",
            "a.css",
            "a.json",
            "a.yaml",
            "a.toml",
            "a.ini",
            "a.md",
            "a.txt",
            "a.sh",
            "a.iss",
            "a.png",
            "a.mp4",
            "a.sql",
            "a.pem",
            "a.zip",
            "a.unknown",
        ] {
            names.push(for_file(n).svg);
        }
        for n in [
            "x",
            "src",
            "public",
            "assets",
            "tests",
            "server",
            "dist",
            "node_modules",
        ] {
            names.push(for_folder(n, false).svg);
            names.push(for_folder(n, true).svg);
        }
        for n in names {
            let path = format!("icons/{n}.svg");
            let bytes = crate::ui::file_icon_assets::embedded(&path)
                .unwrap_or_else(|| panic!("{n} is not embedded"));
            assert!(std::str::from_utf8(bytes).unwrap().starts_with("<svg"));
        }
    }

    #[test]
    fn tree_keys_move_like_a_tree() {
        // root / a (open) / a1 / b (closed) / c.txt
        let rows = [
            (0, true, true),
            (1, true, true),
            (2, false, false),
            (1, true, false),
            (1, false, false),
        ];
        use TreeKey::*;
        assert_eq!(tree_move(&rows, None, Down), TreeMove::Select(0));
        assert_eq!(tree_move(&rows, Some(0), Up), TreeMove::Nothing);
        assert_eq!(tree_move(&rows, Some(0), Down), TreeMove::Select(1));
        assert_eq!(tree_move(&rows, Some(4), Down), TreeMove::Nothing);
        assert_eq!(tree_move(&rows, Some(1), Right), TreeMove::Select(2));
        assert_eq!(tree_move(&rows, Some(3), Right), TreeMove::Expand(3));
        assert_eq!(tree_move(&rows, Some(1), Left), TreeMove::Collapse(1));
        assert_eq!(tree_move(&rows, Some(2), Left), TreeMove::Select(1));
        assert_eq!(tree_move(&rows, Some(3), Left), TreeMove::Select(0));
        assert_eq!(tree_move(&rows, Some(4), Right), TreeMove::Nothing);
        assert_eq!(tree_move(&[], None, Up), TreeMove::Nothing);
    }
}
