//! Übersetzt crossterm-Ereignisse in die Bytefolgen, die Programme im PTY erwarten (xterm-kompatibel).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use vt100::{MouseProtocolEncoding, MouseProtocolMode};

fn modifier_param(m: KeyModifiers) -> u8 {
    1 + m.contains(KeyModifiers::SHIFT) as u8
        + 2 * m.contains(KeyModifiers::ALT) as u8
        + 4 * m.contains(KeyModifiers::CONTROL) as u8
}

/// Kodiert einen Tastendruck. `app_cursor` = DECCKM (Pfeiltasten als `ESC O x`).
pub fn encode_key(key: KeyEvent, app_cursor: bool) -> Option<Vec<u8>> {
    let mods = key.modifiers;
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    let alt = mods.contains(KeyModifiers::ALT);
    let modded = mods.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT | KeyModifiers::CONTROL);

    // Cursor- und Sondertasten: CSI mit optionalem Modifier-Parameter.
    let csi_letter = |c: char| -> Vec<u8> {
        if modded {
            format!("\x1b[1;{}{c}", modifier_param(mods)).into_bytes()
        } else if app_cursor {
            format!("\x1bO{c}").into_bytes()
        } else {
            format!("\x1b[{c}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if modded {
            format!("\x1b[{n};{}~", modifier_param(mods)).into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };

    let mut bytes = match key.code {
        KeyCode::Char(c) if ctrl => {
            let b = match c.to_ascii_lowercase() {
                c @ 'a'..='z' => c as u8 & 0x1f,
                ' ' | '@' | '2' => 0,
                '[' | '3' => 0x1b,
                '\\' | '4' => 0x1c,
                ']' | '5' => 0x1d,
                '^' | '6' => 0x1e,
                '_' | '-' | '7' => 0x1f,
                '?' | '8' => 0x7f,
                _ => return None,
            };
            vec![b]
        }
        KeyCode::Char(c) => c.to_string().into_bytes(),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace if ctrl => vec![0x08],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Tab if mods.contains(KeyModifiers::SHIFT) => b"\x1b[Z".to_vec(),
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => csi_letter('A'),
        KeyCode::Down => csi_letter('B'),
        KeyCode::Right => csi_letter('C'),
        KeyCode::Left => csi_letter('D'),
        KeyCode::Home => csi_letter('H'),
        KeyCode::End => csi_letter('F'),
        KeyCode::Insert => tilde(2),
        KeyCode::Delete => tilde(3),
        KeyCode::PageUp => tilde(5),
        KeyCode::PageDown => tilde(6),
        KeyCode::F(n @ 1..=4) => {
            let c = (b'P' + n - 1) as char;
            if modded {
                format!("\x1b[1;{}{c}", modifier_param(mods)).into_bytes()
            } else {
                format!("\x1bO{c}").into_bytes()
            }
        }
        KeyCode::F(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][(n - 5) as usize]),
        _ => return None,
    };
    // Alt als ESC-Präfix (Meta), außer bei Sequenzen, die den Modifier schon enthalten.
    if alt
        && matches!(
            key.code,
            KeyCode::Char(_) | KeyCode::Enter | KeyCode::Backspace
        )
    {
        bytes.insert(0, 0x1b);
    }
    Some(bytes)
}

/// Kodiert ein Mausereignis gemäß dem vom Programm angeforderten Modus.
/// `col`/`row` sind 0-basierte Zellkoordinaten innerhalb der Terminalfläche.
pub fn encode_mouse(
    event: MouseEvent,
    col: u16,
    row: u16,
    mode: MouseProtocolMode,
    encoding: MouseProtocolEncoding,
) -> Option<Vec<u8>> {
    let button_code = |b: MouseButton| match b {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    };
    let (mut code, release) = match event.kind {
        MouseEventKind::Down(b) => (button_code(b), false),
        MouseEventKind::Up(b) => {
            if mode == MouseProtocolMode::Press {
                return None;
            }
            (button_code(b), true)
        }
        MouseEventKind::Drag(b) => {
            if !matches!(
                mode,
                MouseProtocolMode::ButtonMotion | MouseProtocolMode::AnyMotion
            ) {
                return None;
            }
            (button_code(b) + 32, false)
        }
        MouseEventKind::Moved => {
            if mode != MouseProtocolMode::AnyMotion {
                return None;
            }
            (3 + 32, false)
        }
        MouseEventKind::ScrollUp => (64, false),
        MouseEventKind::ScrollDown => (65, false),
        MouseEventKind::ScrollLeft => (66, false),
        MouseEventKind::ScrollRight => (67, false),
    };
    if mode == MouseProtocolMode::None {
        return None;
    }
    let m = event.modifiers;
    code += 4 * m.contains(KeyModifiers::SHIFT) as u16
        + 8 * m.contains(KeyModifiers::ALT) as u16
        + 16 * m.contains(KeyModifiers::CONTROL) as u16;
    let (x, y) = (col as u32 + 1, row as u32 + 1);
    match encoding {
        MouseProtocolEncoding::Sgr => {
            let end = if release { 'm' } else { 'M' };
            Some(format!("\x1b[<{code};{x};{y}{end}").into_bytes())
        }
        MouseProtocolEncoding::Default | MouseProtocolEncoding::Utf8 => {
            // Ohne SGR kennt das Protokoll keine Taste beim Loslassen.
            let code = if release { 3 | (code & !3) } else { code } as u32;
            let mut out = b"\x1b[M".to_vec();
            for v in [code + 32, x + 32, y + 32] {
                if encoding == MouseProtocolEncoding::Utf8 {
                    let c = char::from_u32(v.min(2047))?;
                    out.extend(c.to_string().as_bytes());
                } else {
                    out.push(u8::try_from(v.min(255)).ok()?);
                }
            }
            Some(out)
        }
    }
}

/// Eingefügten Text so verpacken, wie Shells/Editoren ihn erwarten.
pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed {
        // Endmarker im Text entschärfen, damit nichts „ausbrechen“ kann.
        let text = text.replace("\x1b[201~", "");
        format!("\x1b[200~{text}\x1b[201~").into_bytes()
    } else {
        text.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, m)
    }

    #[test]
    fn keys() {
        let none = KeyModifiers::NONE;
        let enc = |code, m| encode_key(key(code, m), false).unwrap();
        assert_eq!(enc(KeyCode::Char('a'), none), b"a");
        assert_eq!(enc(KeyCode::Char('ä'), none), "ä".as_bytes());
        assert_eq!(enc(KeyCode::Char('c'), KeyModifiers::CONTROL), [3]);
        assert_eq!(enc(KeyCode::Char('x'), KeyModifiers::ALT), b"\x1bx");
        assert_eq!(enc(KeyCode::Enter, none), b"\r");
        assert_eq!(enc(KeyCode::Backspace, none), [0x7f]);
        assert_eq!(enc(KeyCode::Up, none), b"\x1b[A");
        assert_eq!(encode_key(key(KeyCode::Up, none), true).unwrap(), b"\x1bOA");
        assert_eq!(enc(KeyCode::Right, KeyModifiers::CONTROL), b"\x1b[1;5C");
        assert_eq!(enc(KeyCode::Delete, none), b"\x1b[3~");
        assert_eq!(enc(KeyCode::PageUp, KeyModifiers::SHIFT), b"\x1b[5;2~");
        assert_eq!(enc(KeyCode::F(1), none), b"\x1bOP");
        assert_eq!(enc(KeyCode::F(5), none), b"\x1b[15~");
        assert_eq!(enc(KeyCode::F(12), none), b"\x1b[24~");
        assert_eq!(enc(KeyCode::BackTab, KeyModifiers::SHIFT), b"\x1b[Z");
    }

    fn mouse(kind: MouseEventKind) -> MouseEvent {
        MouseEvent {
            kind,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn mouse_sgr_and_default() {
        use MouseProtocolEncoding as E;
        use MouseProtocolMode as M;
        let down = mouse(MouseEventKind::Down(MouseButton::Left));
        let up = mouse(MouseEventKind::Up(MouseButton::Left));
        assert_eq!(
            encode_mouse(down, 4, 9, M::PressRelease, E::Sgr).unwrap(),
            b"\x1b[<0;5;10M"
        );
        assert_eq!(
            encode_mouse(up, 4, 9, M::PressRelease, E::Sgr).unwrap(),
            b"\x1b[<0;5;10m"
        );
        assert_eq!(
            encode_mouse(down, 0, 0, M::PressRelease, E::Default).unwrap(),
            [0x1b, b'[', b'M', 32, 33, 33]
        );
        assert_eq!(
            encode_mouse(up, 0, 0, M::PressRelease, E::Default).unwrap(),
            [0x1b, b'[', b'M', 35, 33, 33]
        );
        assert!(encode_mouse(up, 0, 0, M::Press, E::Sgr).is_none());
        assert!(encode_mouse(down, 0, 0, M::None, E::Sgr).is_none());
        let drag = mouse(MouseEventKind::Drag(MouseButton::Left));
        assert!(encode_mouse(drag, 0, 0, M::PressRelease, E::Sgr).is_none());
        assert_eq!(
            encode_mouse(drag, 1, 1, M::ButtonMotion, E::Sgr).unwrap(),
            b"\x1b[<32;2;2M"
        );
        let wheel = mouse(MouseEventKind::ScrollDown);
        assert_eq!(
            encode_mouse(wheel, 0, 0, M::Press, E::Sgr).unwrap(),
            b"\x1b[<65;1;1M"
        );
    }

    #[test]
    fn paste() {
        assert_eq!(encode_paste("a\nb", false), b"a\rb");
        assert_eq!(encode_paste("x\x1b[201~y", true), b"\x1b[200~xy\x1b[201~");
    }
}
