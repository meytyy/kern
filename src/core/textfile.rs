//! Decoding and encoding of text files. The editor always works on LF text;
//! the original byte order mark and line endings are restored on save.
use similar::{Algorithm, DiffOp, capture_diff_slices};

const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];
/// Files above this size are not opened in the editor.
pub const MAX_EDITABLE_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eol {
    Lf,
    Crlf,
}

impl Eol {
    fn as_str(self) -> &'static str {
        match self {
            Eol::Lf => "\n",
            Eol::Crlf => "\r\n",
        }
    }
}

/// How a file looked on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFormat {
    pub bom: bool,
    /// Line ending used for lines the user adds.
    pub eol: Eol,
    /// For files with mixed endings: the normalized text and ending per line,
    /// so that untouched lines keep their exact original ending.
    pub mixed: Option<MixedEols>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MixedEols {
    pub lines: Vec<String>,
    pub ends: Vec<Option<Eol>>,
}

impl Default for FileFormat {
    fn default() -> Self {
        Self {
            bom: false,
            eol: Eol::Lf,
            mixed: None,
        }
    }
}

impl FileFormat {
    pub fn label(&self) -> &'static str {
        match (&self.mixed, self.eol) {
            (Some(_), _) => "Mixed",
            (None, Eol::Lf) => "LF",
            (None, Eol::Crlf) => "CRLF",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum DecodeError {
    Binary,
    NotUtf8,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Binary => write!(f, "binary file"),
            DecodeError::NotUtf8 => write!(f, "file is not valid UTF-8"),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Decoded {
    pub text: String,
    pub format: FileFormat,
}

/// Decode raw bytes. Refuses binary and non UTF-8 data rather than lossily
/// converting it, because saving a lossy conversion would destroy the file.
pub fn decode(bytes: &[u8]) -> Result<Decoded, DecodeError> {
    let (bom, body) = match bytes.strip_prefix(BOM) {
        Some(rest) => (true, rest),
        None => (false, bytes),
    };
    if body[..body.len().min(8192)].contains(&0) {
        return Err(DecodeError::Binary);
    }
    let text = std::str::from_utf8(body).map_err(|_| DecodeError::NotUtf8)?;

    let crlf = text.matches("\r\n").count();
    let lf = text.matches('\n').count() - crlf;
    let eol = if crlf > lf { Eol::Crlf } else { Eol::Lf };

    if crlf == 0 {
        return Ok(Decoded {
            text: text.to_string(),
            format: FileFormat {
                bom,
                eol: Eol::Lf,
                mixed: None,
            },
        });
    }
    let normalized = text.replace("\r\n", "\n");
    let mixed = (lf > 0).then(|| split_mixed(text));
    Ok(Decoded {
        text: normalized,
        format: FileFormat { bom, eol, mixed },
    })
}

fn split_mixed(text: &str) -> MixedEols {
    let mut lines = Vec::new();
    let mut ends = Vec::new();
    let mut rest = text;
    while let Some(ix) = rest.find('\n') {
        let (line, end) = match rest[..ix].strip_suffix('\r') {
            Some(line) => (line, Eol::Crlf),
            None => (&rest[..ix], Eol::Lf),
        };
        lines.push(line.to_string());
        ends.push(Some(end));
        rest = &rest[ix + 1..];
    }
    lines.push(rest.to_string());
    ends.push(None);
    MixedEols { lines, ends }
}

/// Encode LF text back using the original format of the file.
pub fn encode(text: &str, format: &FileFormat) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 16);
    if format.bom {
        out.extend_from_slice(BOM);
    }
    match &format.mixed {
        Some(mixed) => encode_mixed(text, mixed, format.eol, &mut out),
        None => match format.eol {
            Eol::Lf => out.extend_from_slice(text.as_bytes()),
            Eol::Crlf => out.extend_from_slice(text.replace('\n', "\r\n").as_bytes()),
        },
    }
    out
}

fn encode_mixed(text: &str, old: &MixedEols, default: Eol, out: &mut Vec<u8>) {
    let new_lines: Vec<&str> = text.split('\n').collect();
    let old_lines: Vec<&str> = old.lines.iter().map(String::as_str).collect();
    let mut ends: Vec<Eol> = vec![default; new_lines.len()];
    for op in capture_diff_slices(Algorithm::Myers, &old_lines, &new_lines) {
        if let DiffOp::Equal {
            old_index,
            new_index,
            len,
        } = op
        {
            for i in 0..len {
                if let Some(end) = old.ends[old_index + i] {
                    ends[new_index + i] = end;
                }
            }
        }
    }
    let last = new_lines.len() - 1;
    for (i, line) in new_lines.iter().enumerate() {
        out.extend_from_slice(line.as_bytes());
        if i != last {
            out.extend_from_slice(ends[i].as_str().as_bytes());
        }
    }
}

pub fn hash_bytes(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(input: &[u8]) {
        let d = decode(input).unwrap();
        assert!(!d.text.contains("\r\n"));
        assert_eq!(
            encode(&d.text, &d.format),
            input,
            "{:?}",
            String::from_utf8_lossy(input)
        );
    }

    #[test]
    fn roundtrip_preserves_bytes_exactly() {
        roundtrip(b"a\nb\n");
        roundtrip(b"a\nb");
        roundtrip(b"a\r\nb\r\n");
        roundtrip(b"a\r\nb");
        roundtrip(b"\xEF\xBB\xBFa\r\nb\r\n");
        roundtrip(b"\xEF\xBB\xBFa\nb");
        roundtrip(b"a\r\nb\nc\r\nd\ne");
        roundtrip("Привет\r\nмир\n".as_bytes());
        roundtrip(b"");
        roundtrip(b"\xEF\xBB\xBF");
        roundtrip(b"lone\rcr\n");
    }

    #[test]
    fn crlf_new_lines_use_crlf() {
        let d = decode(b"a\r\nb\r\n").unwrap();
        assert_eq!(d.format.eol, Eol::Crlf);
        assert_eq!(encode("a\nnew\nb\n", &d.format), b"a\r\nnew\r\nb\r\n");
    }

    #[test]
    fn mixed_edit_keeps_untouched_endings() {
        let d = decode(b"one\r\ntwo\nthree\r\n").unwrap();
        assert_eq!(d.format.label(), "Mixed");
        assert_eq!(d.text, "one\ntwo\nthree\n");
        // Insert a line in the middle: neighbours keep their endings.
        let out = encode("one\ntwo\nNEW\nthree\n", &d.format);
        assert!(out.starts_with(b"one\r\ntwo\n"));
        assert!(out.ends_with(b"three\r\n"));
    }

    #[test]
    fn refuses_binary_and_invalid_utf8() {
        assert_eq!(decode(b"a\0b").unwrap_err(), DecodeError::Binary);
        assert_eq!(
            decode(&[0xFF, 0xFE, 0x41]).unwrap_err(),
            DecodeError::NotUtf8
        );
    }
}
