//! Parsing of process output: ANSI stripping, links, error locations.
use once_cell::sync::Lazy;
use regex::Regex;
use std::path::{Path, PathBuf};

static ANSI: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[@-Z\\^_]")
        .unwrap()
});
static URL: Lazy<Regex> = Lazy::new(|| Regex::new(r#"https?://[^\s"'<>`\x1b)\]]+"#).unwrap());
static LOCATION: Lazy<Regex> = Lazy::new(|| {
    Regex::new(concat!(
        r#"(?:File "(?P<py>[^"]+)", line (?P<pyl>\d+))"#,
        r"|(?P<ts>[\w./@\-\p{L} ]*?\.[A-Za-z0-9]{1,5})\((?P<tsl>\d+),(?P<tsc>\d+)\)",
        r"|(?P<p>(?:file://)?(?:\.{0,2}/)?[\w.@\-\p{L}+\[\]]+(?:/[\w.@\-\p{L}+\[\] ]+)*\.[A-Za-z0-9]{1,5}):(?P<l>\d+)(?::(?P<c>\d+))?",
    ))
    .unwrap()
});

pub fn strip_ansi(s: &str) -> String {
    let t = ANSI.replace_all(s, "");
    t.chars()
        .filter(|c| *c == '\t' || !c.is_control())
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// Byte range in the line.
    pub start: usize,
    pub end: usize,
    pub url: String,
}

pub fn find_urls(line: &str) -> Vec<Link> {
    URL.find_iter(line)
        .map(|m| {
            let trimmed = m.as_str().trim_end_matches(['.', ',', ';', ':']);
            Link {
                start: m.start(),
                end: m.start() + trimmed.len(),
                url: trimmed.to_string(),
            }
        })
        .collect()
}

pub fn is_localhost(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let host = rest.split(['/', ':']).next().unwrap_or("");
    matches!(host, "localhost" | "127.0.0.1" | "0.0.0.0" | "[")
        || rest.starts_with("[::1]")
        || host.ends_with(".localhost")
}

/// Browsers cannot always open the bind-all address.
pub fn browser_url(url: &str) -> String {
    url.replacen("://0.0.0.0", "://localhost", 1)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorLocation {
    pub start: usize,
    pub end: usize,
    pub path: String,
    /// 1-based.
    pub line: u32,
    /// 1-based, when known.
    pub column: Option<u32>,
}

/// File locations mentioned in a line of output: `src/a.ts:3:5`, `a.ts(3,5)`,
/// Python tracebacks, stack frames, Rust `-->` arrows.
pub fn find_locations(line: &str) -> Vec<ErrorLocation> {
    let mut out = vec![];
    // URLs contain host:port patterns that look like locations: blank them out,
    // keeping byte offsets, so a location later on the line is still found.
    let mut masked = line.to_string();
    for u in URL.find_iter(line) {
        masked.replace_range(u.range(), &"\u{1}".repeat(u.len()));
    }
    for cap in LOCATION.captures_iter(&masked) {
        let whole = cap.get(0).unwrap();
        let (path, line_no, col) = if let Some(p) = cap.name("py") {
            (p.as_str().to_string(), &cap["pyl"], None)
        } else if let Some(p) = cap.name("ts") {
            (
                p.as_str().trim().to_string(),
                &cap["tsl"],
                cap.name("tsc").map(|m| m.as_str()),
            )
        } else {
            let p = cap["p"].trim_start_matches("file://").to_string();
            (p, &cap["l"], cap.name("c").map(|m| m.as_str()))
        };
        let Ok(line_no) = line_no.parse() else {
            continue;
        };
        out.push(ErrorLocation {
            start: whole.start(),
            end: whole.end(),
            path,
            line: line_no,
            column: col.and_then(|c| c.parse().ok()),
        });
    }
    out
}

/// Resolve a reported path against the working directory; `None` if no such file.
pub fn resolve_location(path: &str, cwd: &Path) -> Option<PathBuf> {
    let p = Path::new(path);
    let full = if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    };
    full.is_file().then_some(full)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Meta,
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputLine {
    pub stream: Stream,
    pub text: String,
}

/// Accumulates chunks into lines. Chunks may split lines and UTF-8 is already
/// decoded by the reader; carriage returns overwrite the current line.
#[derive(Debug, Default)]
pub struct OutputLog {
    pub lines: Vec<OutputLine>,
    open: [bool; 2],
    pub dropped: usize,
}

const MAX_LINES: usize = 50_000;

impl OutputLog {
    pub fn push_meta(&mut self, text: impl Into<String>) {
        self.open = [false, false];
        self.lines.push(OutputLine {
            stream: Stream::Meta,
            text: text.into(),
        });
    }

    pub fn push_chunk(&mut self, stream: Stream, chunk: &str) {
        let ix = match stream {
            Stream::Stdout => 0,
            Stream::Stderr => 1,
            Stream::Meta => {
                self.push_meta(chunk);
                return;
            }
        };
        let mut parts = chunk.split('\n').peekable();
        while let Some(part) = parts.next() {
            let at_end = parts.peek().is_none();
            let cleaned = strip_ansi(part.rsplit('\r').find(|s| !s.is_empty()).unwrap_or(""));
            if self.open[ix] {
                if let Some(last) = self.lines.iter_mut().rev().find(|l| l.stream == stream) {
                    if part.contains('\r') {
                        last.text = cleaned;
                    } else {
                        last.text.push_str(&cleaned);
                    }
                }
            } else if !(at_end && part.is_empty()) {
                self.lines.push(OutputLine {
                    stream,
                    text: cleaned,
                });
            }
            self.open[ix] = at_end && !part.is_empty();
            if !at_end && self.open[ix] {
                self.open[ix] = false;
            }
        }
        if self.lines.len() > MAX_LINES {
            let n = self.lines.len() - MAX_LINES;
            self.lines.drain(..n);
            self.dropped += n;
        }
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_colors_and_control() {
        assert_eq!(
            strip_ansi("\x1b[32m➜\x1b[39m  \x1b[1mLocal\x1b[22m: x"),
            "➜  Local: x"
        );
        assert_eq!(strip_ansi("\x1b]8;;http://a\x07link\x1b]8;;\x07"), "link");
        assert_eq!(strip_ansi("\x1b[2K\x1b[1Gdone"), "done");
    }

    #[test]
    fn finds_vite_localhost_url() {
        let line = "  ➜  Local:   http://localhost:5173/";
        let links = find_urls(line);
        assert_eq!(links[0].url, "http://localhost:5173/");
        assert!(is_localhost(&links[0].url));
        assert_eq!(
            &line[links[0].start..links[0].end],
            "http://localhost:5173/"
        );
        assert!(!is_localhost("https://github.com/x"));
        assert!(is_localhost("http://127.0.0.1:3000"));
        assert!(is_localhost("http://[::1]:3000/"));
        assert_eq!(
            browser_url("http://0.0.0.0:8000/"),
            "http://localhost:8000/"
        );
        assert_eq!(
            find_urls("see http://x.dev/a.").first().unwrap().url,
            "http://x.dev/a"
        );
    }

    #[test]
    fn locations_from_common_tools() {
        let cases: &[(&str, &str, u32, Option<u32>)] = &[
            ("src/App.tsx:12:5: error TS2322", "src/App.tsx", 12, Some(5)),
            (
                "src/App.tsx(12,5): error TS2322: nope",
                "src/App.tsx",
                12,
                Some(5),
            ),
            ("  --> src/main.rs:3:9", "src/main.rs", 3, Some(9)),
            (
                "  File \"/x/app.py\", line 7, in <module>",
                "/x/app.py",
                7,
                None,
            ),
            (
                "    at foo (/abs/my dir/x.js:10:2)",
                "/abs/my dir/x.js",
                10,
                Some(2),
            ),
            (
                "/Users/я/Проект/App.tsx:1:2: ERROR",
                "/Users/я/Проект/App.tsx",
                1,
                Some(2),
            ),
            ("file:///a/b.mjs:4:1", "/a/b.mjs", 4, Some(1)),
        ];
        for (line, path, l, c) in cases {
            let got = find_locations(line);
            let loc = got.first().unwrap_or_else(|| panic!("{line}"));
            assert_eq!(loc.path, *path, "{line}");
            assert_eq!(loc.line, *l, "{line}");
            assert_eq!(loc.column, *c, "{line}");
        }
        assert!(find_locations("http://localhost:5173/").is_empty());
        assert!(find_locations("no location here 12:30").is_empty());
        let both = find_locations("see http://localhost:5173/ and src/a.ts:1:2");
        assert_eq!(both.len(), 1);
        assert!(both[0].path.ends_with("src/a.ts"), "{}", both[0].path);
    }

    #[test]
    fn resolves_only_existing_files() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.ts"), "").unwrap();
        assert!(resolve_location("a.ts", d.path()).is_some());
        assert!(resolve_location("nope.ts", d.path()).is_none());
    }

    #[test]
    fn log_joins_split_chunks_and_keeps_streams_apart() {
        let mut log = OutputLog::default();
        log.push_chunk(Stream::Stdout, "hel");
        log.push_chunk(Stream::Stderr, "err1\n");
        log.push_chunk(Stream::Stdout, "lo\nnext\n");
        assert_eq!(log.lines.len(), 3);
        let out: Vec<_> = log
            .lines
            .iter()
            .filter(|l| l.stream == Stream::Stdout)
            .map(|l| l.text.as_str())
            .collect();
        assert_eq!(out, vec!["hello", "next"]);
        assert_eq!(
            log.lines
                .iter()
                .find(|l| l.stream == Stream::Stderr)
                .unwrap()
                .text,
            "err1"
        );
    }

    #[test]
    fn carriage_return_overwrites() {
        let mut log = OutputLog::default();
        log.push_chunk(Stream::Stdout, "10%");
        log.push_chunk(Stream::Stdout, "\r50%");
        log.push_chunk(Stream::Stdout, "\r100%\nend\n");
        let t: Vec<_> = log.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(t, vec!["100%", "end"]);
    }
}
