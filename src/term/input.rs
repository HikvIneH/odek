//! Key and mouse encoding: what bytes a key press sends to the program.

#[derive(Clone, Copy, Default, Debug)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub cmd: bool,
}

impl Mods {
    /// xterm modifier parameter: 1 + shift + 2·alt + 4·ctrl.
    fn param(self) -> u8 {
        1 + self.shift as u8 + 2 * self.alt as u8 + 4 * self.ctrl as u8
    }
}

// AppKit's function-key code points (NSUpArrowFunctionKey etc.).
const UP: u32 = 0xF700;
const DOWN: u32 = 0xF701;
const LEFT: u32 = 0xF702;
const RIGHT: u32 = 0xF703;
const F1: u32 = 0xF704;
const F12: u32 = 0xF70F;
const INSERT: u32 = 0xF727;
const DELETE: u32 = 0xF728;
const HOME: u32 = 0xF729;
const END: u32 = 0xF72B;
const PAGE_UP: u32 = 0xF72C;
const PAGE_DOWN: u32 = 0xF72D;

/// `chars` is the event's text, `bare` the text ignoring modifiers.
/// Option is treated as Meta (ESC prefix), like most developer setups.
pub fn encode_key(chars: &str, bare: &str, mods: Mods, app_cursor: bool) -> Option<Vec<u8>> {
    let key = bare.chars().next().map(|c| c as u32)?;
    let csi = |s: &str| Some(format!("\x1b[{s}").into_bytes());

    if mods.cmd {
        // The usual macOS terminal conventions for line editing.
        return match key {
            LEFT => Some(vec![0x01]),
            RIGHT => Some(vec![0x05]),
            0x7f => Some(vec![0x15]),
            _ => None,
        };
    }

    let m = mods.param();
    let arrow = |c: char| {
        if mods.alt && !mods.shift && !mods.ctrl {
            // Option+←/→ jump words, as in Terminal.app.
            match c {
                'D' => return Some(b"\x1bb".to_vec()),
                'C' => return Some(b"\x1bf".to_vec()),
                _ => {}
            }
        }
        if m > 1 {
            csi(&format!("1;{m}{c}"))
        } else if app_cursor {
            Some(format!("\x1bO{c}").into_bytes())
        } else {
            csi(&c.to_string())
        }
    };
    let tilde = |n: u8| {
        if m > 1 {
            csi(&format!("{n};{m}~"))
        } else {
            csi(&format!("{n}~"))
        }
    };

    match key {
        UP => return arrow('A'),
        DOWN => return arrow('B'),
        RIGHT => return arrow('C'),
        LEFT => return arrow('D'),
        HOME => return arrow('H'),
        END => return arrow('F'),
        INSERT => return tilde(2),
        DELETE => return tilde(3),
        PAGE_UP => return tilde(5),
        PAGE_DOWN => return tilde(6),
        F1..=F12 => {
            let n = key - F1;
            if n < 4 {
                let c = (b'P' + n as u8) as char;
                return if m > 1 {
                    csi(&format!("1;{m}{c}"))
                } else {
                    Some(format!("\x1bO{c}").into_bytes())
                };
            }
            return tilde([15, 17, 18, 19, 20, 21, 23, 24][n as usize - 4]);
        }
        k if (0xF700..=0xF8FF).contains(&k) => return None,
        _ => {}
    }

    match key {
        // Return: Shift/Option insert a newline in Claude Code and friends.
        0x0d | 0x03 => {
            return Some(if mods.shift || mods.alt {
                b"\x1b\r".to_vec()
            } else {
                b"\r".to_vec()
            });
        }
        0x09 => {
            return Some(if mods.shift {
                b"\x1b[Z".to_vec()
            } else {
                b"\t".to_vec()
            });
        }
        0x19 => return Some(b"\x1b[Z".to_vec()),
        0x7f => {
            return Some(if mods.alt {
                b"\x1b\x7f".to_vec()
            } else if mods.ctrl {
                vec![0x08]
            } else {
                vec![0x7f]
            });
        }
        0x1b => return Some(vec![0x1b]),
        _ => {}
    }

    if mods.ctrl {
        let c = bare.chars().next()?;
        let byte = match c {
            'a'..='z' => Some(c as u8 - b'a' + 1),
            'A'..='Z' => Some(c as u8 - b'A' + 1),
            '@' | ' ' | '2' => Some(0),
            '[' | '3' => Some(0x1b),
            '\\' | '4' => Some(0x1c),
            ']' | '5' => Some(0x1d),
            '^' | '6' => Some(0x1e),
            '_' | '-' | '7' => Some(0x1f),
            '/' => Some(0x1f),
            '8' => Some(0x7f),
            _ => None,
        };
        if let Some(b) = byte {
            return Some(if mods.alt { vec![0x1b, b] } else { vec![b] });
        }
    }

    if mods.alt {
        // Meta: ESC + the unmodified key (shifted if Shift is held).
        let base: String = if mods.shift {
            bare.to_uppercase()
        } else {
            bare.to_string()
        };
        let mut v = vec![0x1b];
        v.extend_from_slice(base.as_bytes());
        return Some(v);
    }

    (!chars.is_empty()).then(|| chars.as_bytes().to_vec())
}

/// Bracketed paste wraps the text so the program can tell it from typing;
/// newlines become CR like a typed Return.
pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed {
        // Strip ESC so pasted text can't end the bracket early.
        let clean = text.replace('\x1b', "");
        format!("\x1b[200~{clean}\x1b[201~").into_bytes()
    } else {
        text.into_bytes()
    }
}

/// SGR mouse report (1006). `button`: 0 left, 1 middle, 2 right, 64/65 wheel.
pub fn encode_mouse_sgr(button: u8, col: usize, row: usize, press: bool, mods: Mods) -> Vec<u8> {
    let b = button as usize + 4 * mods.shift as usize + 8 * mods.alt as usize + 16 * mods.ctrl as usize;
    format!(
        "\x1b[<{b};{};{}{}",
        col + 1,
        row + 1,
        if press { 'M' } else { 'm' }
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(chars: &str, bare: &str, mods: Mods) -> Vec<u8> {
        encode_key(chars, bare, mods, false).unwrap_or_default()
    }

    const NONE: Mods = Mods {
        shift: false,
        ctrl: false,
        alt: false,
        cmd: false,
    };
    const SHIFT: Mods = Mods { shift: true, ..NONE };
    const CTRL: Mods = Mods { ctrl: true, ..NONE };
    const ALT: Mods = Mods { alt: true, ..NONE };
    const CMD: Mods = Mods { cmd: true, ..NONE };

    #[test]
    fn claude_code_keys() {
        assert_eq!(k("\r", "\r", NONE), b"\r");
        assert_eq!(k("\r", "\r", SHIFT), b"\x1b\r");
        assert_eq!(k("\u{19}", "\t", SHIFT), b"\x1b[Z");
        assert_eq!(k("\x1b", "\x1b", NONE), b"\x1b");
        assert_eq!(k("\u{3}", "c", CTRL), b"\x03");
    }

    #[test]
    fn arrows_and_mods() {
        let up = char::from_u32(UP).unwrap().to_string();
        assert_eq!(k(&up, &up, NONE), b"\x1b[A");
        assert_eq!(encode_key(&up, &up, NONE, true).unwrap(), b"\x1bOA");
        assert_eq!(k(&up, &up, SHIFT), b"\x1b[1;2A");
        let left = char::from_u32(LEFT).unwrap().to_string();
        assert_eq!(k(&left, &left, ALT), b"\x1bb");
        assert_eq!(k(&left, &left, CMD), b"\x01");
        assert_eq!(k("\x7f", "\x7f", ALT), b"\x1b\x7f");
        assert_eq!(k("\x7f", "\x7f", CMD), b"\x15");
    }

    #[test]
    fn meta_and_text() {
        assert_eq!(k("å", "a", ALT), b"\x1ba");
        assert_eq!(k("é", "é", NONE), "é".as_bytes());
        assert_eq!(k("", "a", CMD), b"");
    }

    #[test]
    fn paste() {
        assert_eq!(encode_paste("a\nb", true), b"\x1b[200~a\rb\x1b[201~");
        assert_eq!(encode_paste("x\x1b[201~y", true), b"\x1b[200~x[201~y\x1b[201~");
    }
}
