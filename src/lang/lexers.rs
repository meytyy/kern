//! Small line-based lexers for formats without a Tree-sitter grammar in this
//! build: Inno Setup, Dockerfile, env and ignore files. Pure and fully testable.
//! Token kinds are highlight theme names.
use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub range: Range<usize>,
    pub kind: &'static str,
}

pub fn tokenize(language: &str, text: &str) -> Vec<Token> {
    let mut out = vec![];
    let mut offset = 0;
    let mut inno_code = false;
    for raw in text.split_inclusive('\n') {
        let line = raw.trim_end_matches(['\n', '\r']);
        match language {
            "inno" => inno_line(line, offset, &mut inno_code, &mut out),
            "dockerfile" => docker_line(line, offset, &mut out),
            "dotenv" => env_line(line, offset, &mut out),
            "ignore" => ignore_line(line, offset, &mut out),
            _ => {}
        }
        offset += raw.len();
    }
    out
}

fn push(out: &mut Vec<Token>, base: usize, range: Range<usize>, kind: &'static str) {
    if !range.is_empty() {
        out.push(Token {
            range: base + range.start..base + range.end,
            kind,
        });
    }
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// `"..."` strings, `{constants}` and `{#preprocessor}` inside a value.
fn inno_value(line: &str, start: usize, base: usize, out: &mut Vec<Token>) {
    let b = line.as_bytes();
    let mut i = start;
    let mut in_string = false;
    let mut string_start = 0;
    while i < b.len() {
        match b[i] {
            b'"' if in_string => {
                push(out, base, string_start..i + 1, "string");
                in_string = false;
            }
            b'"' => {
                in_string = true;
                string_start = i;
            }
            b'{' if !in_string => {
                let end = line[i..].find('}').map_or(b.len(), |e| i + e + 1);
                let kind = if line[i..].starts_with("{#") {
                    "preproc"
                } else {
                    "variable"
                };
                push(out, base, i..end, kind);
                i = end;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    if in_string {
        push(out, base, string_start..b.len(), "string");
    }
}

const PASCAL: &[&str] = &[
    "begin",
    "end",
    "function",
    "procedure",
    "var",
    "const",
    "type",
    "if",
    "then",
    "else",
    "for",
    "to",
    "do",
    "while",
    "repeat",
    "until",
    "case",
    "of",
    "try",
    "except",
    "finally",
    "and",
    "or",
    "not",
    "uses",
    "array",
    "record",
    "true",
    "false",
    "result",
];

fn pascal_line(line: &str, base: usize, out: &mut Vec<Token>) {
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if line[i..].starts_with("//") {
            push(out, base, i..b.len(), "comment");
            return;
        }
        if c == b'{' {
            let end = line[i..].find('}').map_or(b.len(), |e| i + e + 1);
            push(out, base, i..end, "comment");
            i = end;
            continue;
        }
        if c == b'\'' {
            let end = line[i + 1..].find('\'').map_or(b.len(), |e| i + 1 + e + 1);
            push(out, base, i..end, "string");
            i = end;
            continue;
        }
        if c.is_ascii_digit() {
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            push(out, base, start..i, "number");
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            if PASCAL.contains(&line[start..i].to_ascii_lowercase().as_str()) {
                push(out, base, start..i, "keyword");
            }
            continue;
        }
        i += line[i..].chars().next().map_or(1, char::len_utf8);
    }
}

fn inno_line(line: &str, base: usize, in_code: &mut bool, out: &mut Vec<Token>) {
    let start = indent(line);
    let t = &line[start..];
    if t.is_empty() {
        return;
    }
    if t.starts_with('#') {
        // #define, #include, #if ...
        let end = t
            .find(char::is_whitespace)
            .map_or(line.len(), |e| start + e);
        push(out, base, start..end, "preproc");
        inno_value(line, end, base, out);
        return;
    }
    if t.starts_with('[')
        && let Some(close) = t.find(']')
    {
        *in_code = t[1..close].eq_ignore_ascii_case("code");
        push(out, base, start..start + close + 1, "title");
        return;
    }
    if *in_code {
        pascal_line(line, base, out);
        return;
    }
    if t.starts_with(';') {
        push(out, base, start..line.len(), "comment");
        return;
    }
    // Key=value of [Setup], or `Name: value; Name: value` of entry sections.
    let colon = t.find(": ");
    let eq = t.find('=');
    match (colon, eq) {
        (Some(c), e) if e.is_none_or(|e| c < e) => {
            let mut pos = start;
            while pos < line.len() {
                let rest = &line[pos..];
                let segment_end = find_outside_quotes(rest, ';').map_or(line.len(), |e| pos + e);
                let segment = &line[pos..segment_end];
                if let Some(c) = segment.find(':') {
                    let key_start = pos + indent(segment);
                    push(out, base, key_start..pos + c, "property");
                    inno_value(&line[..segment_end], pos + c + 1, base, out);
                }
                pos = segment_end + 1;
            }
        }
        (_, Some(e)) => {
            push(out, base, start..start + e, "property");
            push(out, base, start + e..start + e + 1, "operator");
            inno_value(line, start + e + 1, base, out);
        }
        _ => {}
    }
}

fn find_outside_quotes(s: &str, needle: char) -> Option<usize> {
    let mut quoted = false;
    for (i, c) in s.char_indices() {
        if c == '"' {
            quoted = !quoted;
        } else if c == needle && !quoted {
            return Some(i);
        }
    }
    None
}

const DOCKER: &[&str] = &[
    "FROM",
    "RUN",
    "CMD",
    "LABEL",
    "MAINTAINER",
    "EXPOSE",
    "ENV",
    "ADD",
    "COPY",
    "ENTRYPOINT",
    "VOLUME",
    "USER",
    "WORKDIR",
    "ARG",
    "ONBUILD",
    "STOPSIGNAL",
    "HEALTHCHECK",
    "SHELL",
    "AS",
];

fn docker_line(line: &str, base: usize, out: &mut Vec<Token>) {
    let start = indent(line);
    let t = &line[start..];
    if t.starts_with('#') {
        push(out, base, start..line.len(), "comment");
        return;
    }
    let word_end = t.find(char::is_whitespace).unwrap_or(t.len());
    let is_instruction = DOCKER.contains(&t[..word_end].to_ascii_uppercase().as_str());
    if is_instruction {
        push(out, base, start..start + word_end, "keyword");
    }
    let b = line.as_bytes();
    let mut i = if is_instruction {
        start + word_end
    } else {
        start
    };
    while i < b.len() {
        match b[i] {
            q @ (b'"' | b'\'') => {
                let end = line[i + 1..]
                    .find(q as char)
                    .map_or(b.len(), |e| i + 1 + e + 1);
                push(out, base, i..end, "string");
                i = end;
                continue;
            }
            b'$' => {
                let mut end = i + 1;
                if b.get(end) == Some(&b'{') {
                    end = line[i..].find('}').map_or(b.len(), |e| i + e + 1);
                } else {
                    while end < b.len() && (b[end].is_ascii_alphanumeric() || b[end] == b'_') {
                        end += 1;
                    }
                }
                push(out, base, i..end, "variable");
                i = end;
                continue;
            }
            b'-' if b.get(i + 1) == Some(&b'-') && (i == 0 || b[i - 1] == b' ') => {
                let end = line[i..].find([' ', '=']).map_or(b.len(), |e| i + e);
                push(out, base, i..end, "attribute");
                i = end;
                continue;
            }
            b'A' if line[i..].to_ascii_uppercase().starts_with("AS ")
                && i > 0
                && b[i - 1] == b' ' =>
            {
                push(out, base, i..i + 2, "keyword");
                i += 2;
                continue;
            }
            _ => {}
        }
        i += line[i..].chars().next().map_or(1, char::len_utf8);
    }
}

fn env_line(line: &str, base: usize, out: &mut Vec<Token>) {
    let start = indent(line);
    let t = &line[start..];
    if t.starts_with('#') {
        push(out, base, start..line.len(), "comment");
        return;
    }
    let mut key_start = start;
    if t.starts_with("export ") {
        push(out, base, start..start + 6, "keyword");
        key_start = start + 7;
    }
    let Some(eq) = line[key_start..].find('=') else {
        return;
    };
    let eq = key_start + eq;
    push(out, base, key_start..eq, "property");
    push(out, base, eq..eq + 1, "operator");
    let v = eq + 1;
    let value = &line[v..];
    let first = value.chars().next();
    let kind = match first {
        Some('"' | '\'') => "string",
        _ if !value.is_empty() && value.trim().parse::<f64>().is_ok() => "number",
        _ if matches!(value.trim(), "true" | "false") => "boolean",
        _ => "string",
    };
    let comment = if matches!(first, Some('"' | '\'')) {
        None
    } else {
        value.find(" #")
    };
    match comment {
        Some(c) => {
            push(out, base, v..v + c, kind);
            push(out, base, v + c + 1..line.len(), "comment");
        }
        None => push(out, base, v..line.len(), kind),
    }
}

fn ignore_line(line: &str, base: usize, out: &mut Vec<Token>) {
    let t = line.trim_end();
    if t.starts_with('#') {
        push(out, base, 0..line.len(), "comment");
        return;
    }
    if t.starts_with('!') {
        push(out, base, 0..1, "keyword");
    }
    for (i, c) in t.char_indices() {
        if matches!(c, '*' | '?') {
            push(out, base, i..i + 1, "operator");
        } else if c == '/' {
            push(out, base, i..i + 1, "punctuation");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds<'a>(lang: &str, text: &'a str) -> Vec<(&'a str, &'static str)> {
        tokenize(lang, text)
            .into_iter()
            .map(|t| (&text[t.range.clone()], t.kind))
            .collect()
    }

    #[test]
    fn inno_setup_structure() {
        let src = "; comment\n#define MyApp \"Mirra\"\n[Setup]\nAppName={#MyApp}\n[Files]\nSource: \"a.exe\"; DestDir: \"{app}\"; Flags: ignoreversion\n";
        let t = kinds("inno", src);
        assert!(t.contains(&("; comment", "comment")));
        assert!(t.contains(&("#define", "preproc")));
        assert!(t.contains(&("\"Mirra\"", "string")));
        assert!(t.contains(&("[Setup]", "title")));
        assert!(t.contains(&("AppName", "property")));
        assert!(t.contains(&("{#MyApp}", "preproc")));
        assert!(t.contains(&("Source", "property")));
        assert!(t.contains(&("\"a.exe\"", "string")));
        assert!(t.contains(&("DestDir", "property")));
        assert!(t.contains(&("Flags", "property")));
    }

    #[test]
    fn inno_code_section_is_basic_pascal() {
        let src = "[Code]\nfunction Foo(): Boolean;\nbegin\n  // Привет\n  Result := 'x' = 'y'; { block }\nend;\n[Run]\nFilename: \"x\"\n";
        let t = kinds("inno", src);
        assert!(t.contains(&("function", "keyword")));
        assert!(t.contains(&("begin", "keyword")));
        assert!(t.contains(&("// Привет", "comment")));
        assert!(t.contains(&("'x'", "string")));
        assert!(t.contains(&("{ block }", "comment")));
        // Leaving [Code] restores the key: value rules.
        assert!(t.contains(&("Filename", "property")));
    }

    #[test]
    fn dockerfile() {
        let t = kinds(
            "dockerfile",
            "# base\nFROM node:20 AS build\nENV X=$HOME\nRUN echo \"hi\" --flag\nCOPY --from=build a b\n",
        );
        for want in [
            ("# base", "comment"),
            ("FROM", "keyword"),
            ("AS", "keyword"),
            ("$HOME", "variable"),
            ("\"hi\"", "string"),
            ("--from", "attribute"),
        ] {
            assert!(t.contains(&want), "{want:?} in {t:?}");
        }
    }

    #[test]
    fn env_files() {
        let t = kinds(
            "dotenv",
            "# c\nexport A=1\nB=\"x y\"\nC=hello # note\nD=true\n",
        );
        for want in [
            ("# c", "comment"),
            ("export", "keyword"),
            ("A", "property"),
            ("1", "number"),
            ("\"x y\"", "string"),
            ("hello", "string"),
            ("# note", "comment"),
            ("true", "boolean"),
        ] {
            assert!(t.contains(&want), "{want:?} in {t:?}");
        }
    }

    #[test]
    fn ignore_files() {
        let t = kinds("ignore", "# c\n!keep.txt\nnode_modules/\n*.log\n");
        for want in [
            ("# c", "comment"),
            ("!", "keyword"),
            ("/", "punctuation"),
            ("*", "operator"),
        ] {
            assert!(t.contains(&want), "{want:?} in {t:?}");
        }
    }

    #[test]
    fn offsets_survive_crlf_and_multibyte() {
        let src = "; я\r\n[Setup]\r\n";
        let t = tokenize("inno", src);
        assert_eq!(&src[t[1].range.clone()], "[Setup]");
        // Never panics on char boundaries.
        for lang in ["inno", "dockerfile", "dotenv", "ignore"] {
            tokenize(lang, "я=ю \"😀\" $я {я}\n[я\nRUN 'я\n");
        }
    }
}
