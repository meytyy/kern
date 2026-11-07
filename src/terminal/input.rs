//! Keyboard → terminal byte sequences (xterm conventions), independent of the UI.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

/// Bytes for a key press, or `None` when it should go through text input.
pub fn key_to_bytes(key: &str, mods: Mods, app_cursor: bool) -> Option<Vec<u8>> {
    let modifier_param = 1 + (mods.shift as u8) + 2 * (mods.alt as u8) + 4 * (mods.ctrl as u8);
    let csi_key = |letter: char| -> Vec<u8> {
        if modifier_param > 1 {
            format!("\x1b[1;{modifier_param}{letter}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{letter}").into_bytes()
        } else {
            format!("\x1b[{letter}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if modifier_param > 1 {
            format!("\x1b[{n};{modifier_param}~").into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };
    let alt_prefix = |bytes: &[u8]| -> Vec<u8> {
        let mut v = Vec::with_capacity(bytes.len() + 1);
        if mods.alt {
            v.push(0x1b);
        }
        v.extend_from_slice(bytes);
        v
    };
    Some(match key {
        "enter" => {
            if mods.shift {
                // Understood by programs such as Claude Code as a newline in the prompt.
                b"\x1b\r".to_vec()
            } else {
                alt_prefix(b"\r")
            }
        }
        "tab" => {
            if mods.shift {
                b"\x1b[Z".to_vec()
            } else {
                alt_prefix(b"\t")
            }
        }
        "backspace" => {
            if mods.ctrl {
                alt_prefix(&[0x08])
            } else {
                alt_prefix(&[0x7f])
            }
        }
        "escape" => alt_prefix(&[0x1b]),
        "space" if mods.ctrl => alt_prefix(&[0x00]),
        "up" => csi_key('A'),
        "down" => csi_key('B'),
        "right" => csi_key('C'),
        "left" => csi_key('D'),
        "home" => csi_key('H'),
        "end" => csi_key('F'),
        "delete" => tilde(3),
        "insert" => tilde(2),
        "pageup" => tilde(5),
        "pagedown" => tilde(6),
        "f1" => b"\x1bOP".to_vec(),
        "f2" => b"\x1bOQ".to_vec(),
        "f3" => b"\x1bOR".to_vec(),
        "f4" => b"\x1bOS".to_vec(),
        "f5" => tilde(15),
        "f6" => tilde(17),
        "f7" => tilde(18),
        "f8" => tilde(19),
        "f9" => tilde(20),
        "f10" => tilde(21),
        "f11" => tilde(23),
        "f12" => tilde(24),
        k if mods.ctrl && k.chars().count() == 1 => {
            let c = k.chars().next()?;
            let b = match c.to_ascii_lowercase() {
                c @ 'a'..='z' => c as u8 - b'a' + 1,
                '[' | '3' => 0x1b,
                '\\' | '4' => 0x1c,
                ']' | '5' => 0x1d,
                '^' | '6' => 0x1e,
                '_' | '7' | '-' => 0x1f,
                '@' | '2' => 0x00,
                _ => return None,
            };
            alt_prefix(&[b])
        }
        // Alt+letter: meta prefix. (Text-producing Option keys are handled by the UI layer.)
        k if mods.alt && k.chars().count() == 1 && k.is_ascii() => {
            let mut v = vec![0x1b];
            v.extend_from_slice(k.as_bytes());
            v
        }
        _ => return None,
    })
}

/// Text from paste; bracketed when the program asked for it.
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    // Strip embedded end markers: pasted data must not break out of bracketed mode.
    let clean = text
        .replace("\x1b[201~", "")
        .replace("\r\n", "\r")
        .replace('\n', "\r");
    if bracketed {
        let mut v = b"\x1b[200~".to_vec();
        v.extend_from_slice(clean.as_bytes());
        v.extend_from_slice(b"\x1b[201~");
        v
    } else {
        clean.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Mods = Mods {
        ctrl: false,
        alt: false,
        shift: false,
    };

    #[test]
    fn control_keys() {
        let ctrl = Mods { ctrl: true, ..NONE };
        assert_eq!(key_to_bytes("c", ctrl, false).unwrap(), vec![3]);
        assert_eq!(key_to_bytes("d", ctrl, false).unwrap(), vec![4]);
        assert_eq!(key_to_bytes("[", ctrl, false).unwrap(), vec![0x1b]);
        assert_eq!(key_to_bytes("space", ctrl, false).unwrap(), vec![0]);
    }

    #[test]
    fn cursor_modes() {
        assert_eq!(key_to_bytes("up", NONE, false).unwrap(), b"\x1b[A");
        assert_eq!(key_to_bytes("up", NONE, true).unwrap(), b"\x1bOA");
        assert_eq!(
            key_to_bytes("left", Mods { ctrl: true, ..NONE }, true).unwrap(),
            b"\x1b[1;5D"
        );
        assert_eq!(key_to_bytes("delete", NONE, false).unwrap(), b"\x1b[3~");
    }

    #[test]
    fn plain_text_goes_through_text_input() {
        assert_eq!(key_to_bytes("a", NONE, false), None);
        assert_eq!(key_to_bytes("я", NONE, false), None);
        assert_eq!(key_to_bytes("enter", NONE, false).unwrap(), b"\r");
        assert_eq!(key_to_bytes("backspace", NONE, false).unwrap(), vec![0x7f]);
        assert_eq!(
            key_to_bytes(
                "tab",
                Mods {
                    shift: true,
                    ..NONE
                },
                false
            )
            .unwrap(),
            b"\x1b[Z"
        );
    }

    #[test]
    fn paste_modes() {
        assert_eq!(paste_bytes("a\nb", false), b"a\rb");
        assert_eq!(paste_bytes("a", true), b"\x1b[200~a\x1b[201~");
        // Cannot escape bracketed paste.
        assert_eq!(
            paste_bytes("x\x1b[201~rm -rf /\n", true),
            b"\x1b[200~xrm -rf /\r\x1b[201~"
        );
    }
}
