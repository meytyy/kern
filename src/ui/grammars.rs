//! Grammar registration beyond the component library's defaults.
use crate::lang::lexers::{self, Token};
use gpui_kit::component::highlighter::{GrammarConfig, LanguageRegistry};
use gpui_kit::component::input::{
    EditorState, FoldRange, InputEdit, InputHighlighter, InputHighlighterFactory, Rope,
};
use gpui_kit::*;
use std::ops::Range;
use std::rc::Rc;

/// The TSX grammar shares its base rules with JavaScript. The stock registration
/// loads only TypeScript's additions, so keywords and strings stay uncolored.
pub fn register() {
    let highlights = format!(
        "{}\n{}\n{}",
        tree_sitter_javascript::HIGHLIGHT_QUERY,
        tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
        tree_sitter_typescript::HIGHLIGHTS_QUERY
    );
    let registry = LanguageRegistry::singleton();
    registry.register(
        "tsx",
        &GrammarConfig::new(
            "tsx",
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            vec![],
            &highlights,
            tree_sitter_javascript::INJECTIONS_QUERY,
            tree_sitter_typescript::LOCALS_QUERY,
        ),
    );
    // The stock Swift registration has an empty query: the parser runs, no colors.
    registry.register(
        "swift",
        &GrammarConfig::new(
            "swift",
            tree_sitter_swift::LANGUAGE.into(),
            vec![],
            tree_sitter_swift::HIGHLIGHTS_QUERY,
            "",
            tree_sitter_swift::LOCALS_QUERY,
        ),
    );
    // Same empty query for CMake in the stock registration.
    registry.register(
        "cmake",
        &GrammarConfig::new(
            "cmake",
            tree_sitter_cmake::LANGUAGE.into(),
            vec![],
            tree_sitter_cmake::HIGHLIGHTS_QUERY,
            "",
            "",
        ),
    );
    // C++ queries only add to the C ones, as TSX does to JavaScript.
    let cpp = format!(
        "{}\n{}",
        tree_sitter_c::HIGHLIGHT_QUERY,
        tree_sitter_cpp::HIGHLIGHT_QUERY
    );
    registry.register(
        "cpp",
        &GrammarConfig::new(
            "cpp",
            tree_sitter_cpp::LANGUAGE.into(),
            vec![],
            &cpp,
            "",
            "",
        ),
    );
    let js = format!(
        "{}\n{}",
        tree_sitter_javascript::HIGHLIGHT_QUERY,
        tree_sitter_javascript::JSX_HIGHLIGHT_QUERY
    );
    registry.register(
        "javascript",
        &GrammarConfig::new(
            "javascript",
            tree_sitter_javascript::LANGUAGE.into(),
            vec![],
            &js,
            tree_sitter_javascript::INJECTIONS_QUERY,
            tree_sitter_javascript::LOCALS_QUERY,
        ),
    );
}

/// Editor highlighter for KERN's own lexers.
pub struct LexerHighlighter {
    language: SharedString,
    tokens: Vec<Token>,
}

impl InputHighlighter for LexerHighlighter {
    fn language(&self) -> SharedString {
        self.language.clone()
    }

    fn update(
        &mut self,
        _: Option<InputEdit>,
        text: &Rope,
        _: bool,
        _: &mut Window,
        _: &mut Context<EditorState>,
    ) {
        self.tokens = lexers::tokenize(&self.language, &text.to_string());
    }

    fn styles(
        &self,
        range: &Range<usize>,
        resolver: &dyn gpui_kit::base::input::HighlightStyleResolver,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        style_runs(&self.tokens, range, |kind| {
            // In these formats keys and constants are the structure of the file,
            // so they get the stronger theme colors.
            let kind = match kind {
                "property" => "attribute",
                "variable" => "constant",
                other => other,
            };
            resolver.style(kind).unwrap_or_default()
        })
    }

    fn fold_ranges(&self, _: &Rope) -> Vec<FoldRange> {
        vec![]
    }
}

/// Ordered, non-overlapping runs covering `range`, as the editor requires.
fn style_runs(
    tokens: &[Token],
    range: &Range<usize>,
    style: impl Fn(&str) -> HighlightStyle,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut out = vec![];
    let mut pos = range.start;
    for t in tokens {
        if t.range.end <= pos {
            continue;
        }
        if t.range.start >= range.end {
            break;
        }
        let start = t.range.start.max(pos);
        if start > pos {
            out.push((pos..start, HighlightStyle::default()));
        }
        let end = t.range.end.min(range.end);
        out.push((start..end, style(t.kind)));
        pos = end;
    }
    if pos < range.end {
        out.push((pos..range.end, HighlightStyle::default()));
    }
    out
}

pub fn lexer_factory() -> InputHighlighterFactory {
    Rc::new(|language| {
        Some(Box::new(LexerHighlighter {
            language: language.to_string().into(),
            tokens: vec![],
        }) as Box<dyn InputHighlighter>)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::{Highlight, LANGUAGES};
    use gpui_kit::component::highlighter::{HighlightTheme, SyntaxHighlighter};

    /// A grammar in the dependency tree proves nothing: a query that fails to
    /// compile leaves the editor silently uncolored. Parse real samples and
    /// require several different colors from the theme.
    fn distinct_colors(grammar: &str, sample: &str) -> usize {
        register();
        let rope = Rope::from(sample);
        let mut h = SyntaxHighlighter::new(grammar);
        h.update(None, &rope, None);
        let theme = HighlightTheme::default_dark();
        let styles = h.styles(&(0..rope.len()), theme.as_ref());
        let mut colors: Vec<String> = styles
            .iter()
            .filter_map(|(_, s)| s.color.map(|c| format!("{c:?}")))
            .collect();
        colors.sort();
        colors.dedup();
        colors.len()
    }

    #[::core::prelude::v1::test]
    fn every_tree_sitter_language_really_highlights() {
        let samples: &[(&str, &str)] = &[
            (
                "javascript",
                "// c\nimport a from 'x';\nfunction f(n) { return <div id=\"a\">{n + 1}</div>; }\n",
            ),
            (
                "typescript",
                "// c\ninterface P { a: number }\nexport function f(p: P): string { return 'x' + 1; }\n",
            ),
            (
                "tsx",
                "// c\nimport a from 'x';\ntype P = { a: number };\nexport function F(p: P) { return <div id=\"a\">{p.a + 1}</div>; }\n",
            ),
            (
                "python",
                "# c\nimport os\ndef f(a: int) -> str:\n    return 'x' + str(1)\n",
            ),
            (
                "rust",
                "// c\nfn f(a: u32) -> String { let s = \"x\"; format!(\"{s}{}\", a + 1) }\n",
            ),
            (
                "swift",
                "// c\nimport Foundation\nstruct P { let a: Int }\nfunc f(_ p: P) -> String { return \"x\\(p.a + 1)\" }\n",
            ),
            (
                "c",
                "// c\n#include <stdio.h>\nint f(int a) { return printf(\"x\") + 1; }\n",
            ),
            (
                "cpp",
                "// c\n#include <string>\nclass P { public: int a; };\nstd::string f(P p) { return \"x\"; }\n",
            ),
            (
                "cmake",
                "# c\nproject(mirra VERSION 1.0)\nadd_executable(a main.cpp)\nif(WIN32)\n  set(X \"y\")\nendif()\n",
            ),
            (
                "bash",
                "# c\nset -e\nf() { echo \"x $1\"; }\nif [ -n \"$A\" ]; then f 1; fi\n",
            ),
            ("html", "<!-- c -->\n<div class=\"a\"><p>text</p></div>\n"),
            ("css", "/* c */\n.a > b { color: red; margin: 1px; }\n"),
            ("json", "{\"a\": 1, \"b\": \"x\", \"c\": true}\n"),
            ("yaml", "# c\na: 1\nb: 'x'\nc: true\n"),
            ("toml", "# c\n[a]\nb = 1\nc = \"x\"\n"),
            ("markdown", "# Title\n\ntext\n\n- item\n\n```\ncode\n```\n"),
        ];
        let mut failed = vec![];
        for l in LANGUAGES
            .iter()
            .filter(|l| l.highlight == Highlight::TreeSitter)
        {
            let sample = samples
                .iter()
                .find(|(g, _)| *g == l.grammar)
                .unwrap_or_else(|| panic!("no sample for {}", l.grammar))
                .1;
            let n = distinct_colors(l.grammar, sample);
            eprintln!("{}: {n} colors", l.id);
            if n < 2 {
                failed.push(l.id);
            }
        }
        assert!(failed.is_empty(), "not really highlighted: {failed:?}");
    }

    #[::core::prelude::v1::test]
    fn tsx_distinguishes_keywords_strings_tags_and_attributes() {
        assert!(
            distinct_colors(
                "tsx",
                "import a from 'x';\nexport function F() { return <div id=\"a\">{1}</div>; }\n"
            ) >= 5
        );
    }

    #[::core::prelude::v1::test]
    fn lexer_style_runs_cover_the_range_without_overlap() {
        let src = "; c\n[Setup]\nAppName=KERN\n";
        let tokens = lexers::tokenize("inno", src);
        let runs = style_runs(&tokens, &(0..src.len()), |_| HighlightStyle::default());
        assert_eq!(runs.first().unwrap().0.start, 0);
        assert_eq!(runs.last().unwrap().0.end, src.len());
        for w in runs.windows(2) {
            assert_eq!(w[0].0.end, w[1].0.start);
        }
        let inner = style_runs(&tokens, &(5..9), |_| HighlightStyle::default());
        assert_eq!(inner.first().unwrap().0.start, 5);
        assert_eq!(inner.last().unwrap().0.end, 9);
    }
}
