//! Conversions between editor offsets and LSP positions.
//!
//! The server counts columns in the negotiated encoding (UTF-16 by default).
//! The editor widget's `Position` counts columns in Unicode scalars, and its
//! rope uses byte offsets. Mixing these up corrupts edits on lines containing
//! emoji or Cyrillic, so all conversions live here.
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Encoding {
    Utf8,
    #[default]
    Utf16,
    Utf32,
}

impl Encoding {
    /// Read `capabilities.positionEncoding` from an initialize response.
    pub fn from_capabilities(caps: &Value) -> Self {
        match caps.get("positionEncoding").and_then(Value::as_str) {
            Some("utf-8") => Encoding::Utf8,
            Some("utf-32") => Encoding::Utf32,
            _ => Encoding::Utf16,
        }
    }

    fn width(self, c: char) -> usize {
        match self {
            Encoding::Utf8 => c.len_utf8(),
            Encoding::Utf16 => c.len_utf16(),
            Encoding::Utf32 => 1,
        }
    }
}

/// Byte column in `line` → server column. `line` has no line terminator.
pub fn byte_to_server_col(line: &str, byte_col: usize, enc: Encoding) -> u32 {
    let mut col = 0;
    for (i, c) in line.char_indices() {
        if i >= byte_col {
            break;
        }
        col += enc.width(c);
    }
    col as u32
}

/// Server column → byte column. Clamps past end of line and never lands inside
/// a character, as the spec requires for positions pointing into a surrogate pair.
pub fn server_col_to_byte(line: &str, server_col: u32, enc: Encoding) -> usize {
    let target = server_col as usize;
    let mut col = 0;
    for (i, c) in line.char_indices() {
        let w = enc.width(c);
        if col + w > target {
            return i;
        }
        col += w;
    }
    line.len()
}

/// Server column → editor widget column (Unicode scalars).
pub fn server_col_to_scalar(line: &str, server_col: u32, enc: Encoding) -> u32 {
    let byte = server_col_to_byte(line, server_col, enc);
    line[..byte].chars().count() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use Encoding::*;

    // "a" 1 byte, "я" 2 bytes, "😀" 4 bytes / 2 UTF-16 units, "b".
    const L: &str = "aя😀b";

    #[test]
    fn byte_to_server() {
        assert_eq!(byte_to_server_col(L, 0, Utf16), 0);
        assert_eq!(byte_to_server_col(L, 1, Utf16), 1);
        assert_eq!(byte_to_server_col(L, 3, Utf16), 2);
        assert_eq!(byte_to_server_col(L, 7, Utf16), 4);
        assert_eq!(byte_to_server_col(L, 8, Utf16), 5);
        assert_eq!(byte_to_server_col(L, 7, Utf8), 7);
        assert_eq!(byte_to_server_col(L, 7, Utf32), 3);
        assert_eq!(byte_to_server_col(L, 999, Utf16), 5);
    }

    #[test]
    fn server_to_byte_roundtrip_all_encodings() {
        for enc in [Utf8, Utf16, Utf32] {
            for (byte, _) in L.char_indices().chain([(L.len(), ' ')]) {
                let col = byte_to_server_col(L, byte, enc);
                assert_eq!(server_col_to_byte(L, col, enc), byte, "{enc:?} {byte}");
            }
        }
    }

    #[test]
    fn inside_surrogate_pair_snaps_to_char_start() {
        // UTF-16 column 3 is the middle of the emoji.
        assert_eq!(server_col_to_byte(L, 3, Utf16), 3);
    }

    #[test]
    fn past_end_clamps() {
        assert_eq!(server_col_to_byte(L, 100, Utf16), L.len());
        assert_eq!(server_col_to_byte("", 5, Utf16), 0);
    }

    #[test]
    fn scalar_columns_for_widget() {
        assert_eq!(server_col_to_scalar(L, 4, Utf16), 3);
        assert_eq!(server_col_to_scalar(L, 7, Utf8), 3);
        assert_eq!(server_col_to_scalar(L, 3, Utf32), 3);
    }

    #[test]
    fn capability_negotiation() {
        assert_eq!(Encoding::from_capabilities(&serde_json::json!({})), Utf16);
        assert_eq!(
            Encoding::from_capabilities(&serde_json::json!({"positionEncoding":"utf-8"})),
            Utf8
        );
    }
}
