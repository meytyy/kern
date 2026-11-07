//! How Tab indents a given file: the file's own style first, settings as the fallback.
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileIndent {
    pub size: usize,
    pub hard_tabs: bool,
}

/// Formats whose tools expect tab characters, whatever the file holds now.
fn requires_tabs(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    matches!(
        name,
        "Makefile" | "makefile" | "GNUmakefile" | "go.mod" | "go.work"
    ) || matches!(ext, "mk" | "go")
}

/// Indentation for `text`. `default_size` applies when the file says nothing,
/// and is the display width of tab characters.
pub fn detect(path: &Path, text: &str, default_size: usize) -> FileIndent {
    let hard = FileIndent {
        size: default_size,
        hard_tabs: true,
    };
    if requires_tabs(path) {
        return hard;
    }
    let (mut tabs, mut spaces) = (0usize, 0usize);
    // How often each step between neighbouring indented lines occurs.
    let mut steps = [0usize; 9];
    let mut previous = 0usize;
    // Enough to read the style; huge generated files need no full scan.
    for line in text.lines().take(2000) {
        if line.trim().is_empty() {
            continue;
        }
        if line.starts_with('\t') {
            tabs += 1;
            continue;
        }
        let width = line.len() - line.trim_start_matches(' ').len();
        // A lone leading space is comment or block-star alignment, not indentation.
        if width >= 2 {
            spaces += 1;
        }
        let step = width.abs_diff(previous);
        if (2..=8).contains(&step) {
            steps[step] += 1;
        }
        previous = width;
    }
    if tabs > spaces {
        return hard;
    }
    let size = (2..=8)
        .filter(|s| steps[*s] > 0)
        .max_by_key(|s| (steps[*s], std::cmp::Reverse(*s)))
        .unwrap_or(default_size);
    FileIndent {
        size,
        hard_tabs: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(name: &str, text: &str) -> FileIndent {
        detect(Path::new(name), text, 2)
    }

    #[test]
    fn follows_the_files_own_style() {
        let four = run(
            "a.rs",
            "fn a() {\n    let x = 1;\n    if x {\n        y();\n    }\n}\n",
        );
        assert_eq!((four.size, four.hard_tabs), (4, false));
        let two = run("a.ts", "a {\n  b {\n    c\n  }\n}\n");
        assert_eq!((two.size, two.hard_tabs), (2, false));
        assert!(run("a.c", "int a() {\n\treturn 1;\n}\n").hard_tabs);
    }

    #[test]
    fn settings_apply_when_the_file_says_nothing() {
        assert_eq!(
            detect(Path::new("a.txt"), "one\ntwo\n", 3),
            FileIndent {
                size: 3,
                hard_tabs: false
            }
        );
        assert!(!run("new.rs", "").hard_tabs);
    }

    #[test]
    fn tab_only_formats_keep_tabs_even_when_empty() {
        for name in ["Makefile", "GNUmakefile", "rules.mk", "main.go", "go.mod"] {
            assert!(run(name, "").hard_tabs, "{name}");
        }
    }

    #[test]
    fn stray_alignment_does_not_decide() {
        // Doc-comment stars and a few mixed lines must not flip a tab-indented file.
        let text = "/**\n * doc\n */\nint a() {\n\tb();\n\tc();\n}\n";
        assert!(run("a.c", text).hard_tabs);
    }
}
