use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Encode a key event for a PTY child.
/// If `kitty_mode` is true, modified keys use CSI u format.
pub fn encode_key(key: KeyEvent, kitty_mode: bool) -> Vec<u8> {
    if kitty_mode {
        if let Some(bytes) = try_encode_csi_u(&key) {
            return bytes;
        }
    }
    encode_legacy(key)
}

/// CSI u encoding: \e[{codepoint};{modifiers}u
/// Used when the child has pushed Kitty keyboard enhancement.
/// Returns None if the key doesn't need CSI u (unmodified basic keys).
fn try_encode_csi_u(key: &KeyEvent) -> Option<Vec<u8>> {
    let mods = key.modifiers;

    // Unmodified keys use legacy encoding (more compatible)
    if mods.is_empty() {
        return None;
    }

    // Plain Ctrl+letter is well-represented in legacy (bytes 1-26)
    if mods == KeyModifiers::CONTROL {
        if let KeyCode::Char(c) = key.code {
            if c.is_ascii_alphabetic() {
                return None; // let legacy handle it
            }
        }
    }

    // Special keys (arrows, F-keys, etc.) have well-established legacy
    // xterm modified formats (\x1b[1;3A for Alt+Up, etc.) that are universally
    // understood. Even Ghostty sends these in legacy format with kitty mode on.
    // Only use CSI u for character keys and keys without legacy representations.
    match key.code {
        KeyCode::Up
        | KeyCode::Down
        | KeyCode::Left
        | KeyCode::Right
        | KeyCode::Home
        | KeyCode::End
        | KeyCode::PageUp
        | KeyCode::PageDown
        | KeyCode::Insert
        | KeyCode::Delete
        | KeyCode::F(_) => {
            return None; // let legacy handle these
        }
        _ => {}
    }

    let codepoint = match key.code {
        KeyCode::Char(c) => c as u32,
        KeyCode::Enter => 13,
        KeyCode::Tab => 9,
        KeyCode::Backspace => 127,
        KeyCode::Esc => 27,
        _ => return None, // fall back to legacy for unhandled keys
    };

    let modifier = kitty_modifier(mods);

    Some(format!("\x1b[{codepoint};{modifier}u").into_bytes())
}

/// Legacy terminal encoding (standard escape sequences).
fn encode_legacy(key: KeyEvent) -> Vec<u8> {
    let mods = key.modifiers;

    // Modified special keys (arrows, home, end, etc.) use xterm format:
    //   \x1b[1;{modifier}A  for arrows/home/end
    //   \x1b[{n};{modifier}~ for insert/delete/pgup/pgdn
    // The ESC-prefix hack doesn't work for these since they're already escape sequences.
    if !mods.is_empty() {
        if let Some(bytes) = encode_modified_special(key.code, mods) {
            return bytes;
        }
    }

    // Alt modifier on character keys: prefix with ESC
    if mods.contains(KeyModifiers::ALT) {
        let inner = KeyEvent::new(key.code, mods.difference(KeyModifiers::ALT));
        let mut bytes = vec![0x1b];
        bytes.extend(encode_legacy_inner(inner));
        return bytes;
    }
    encode_legacy_inner(key)
}

/// xterm-style encoding for modified special keys.
/// Modifier value: 1 + (shift?1:0) + (alt?2:0) + (ctrl?4:0)
fn encode_modified_special(code: KeyCode, mods: KeyModifiers) -> Option<Vec<u8>> {
    let modifier = xterm_modifier(mods);
    if modifier <= 1 {
        return None; // no modifiers to encode
    }

    match code {
        // CSI 1;{mod}{letter} format
        KeyCode::Up => Some(format!("\x1b[1;{modifier}A").into_bytes()),
        KeyCode::Down => Some(format!("\x1b[1;{modifier}B").into_bytes()),
        KeyCode::Right => Some(format!("\x1b[1;{modifier}C").into_bytes()),
        KeyCode::Left => Some(format!("\x1b[1;{modifier}D").into_bytes()),
        KeyCode::Home => Some(format!("\x1b[1;{modifier}H").into_bytes()),
        KeyCode::End => Some(format!("\x1b[1;{modifier}F").into_bytes()),
        // CSI {n};{mod}~ format
        KeyCode::Insert => Some(format!("\x1b[2;{modifier}~").into_bytes()),
        KeyCode::Delete => Some(format!("\x1b[3;{modifier}~").into_bytes()),
        KeyCode::PageUp => Some(format!("\x1b[5;{modifier}~").into_bytes()),
        KeyCode::PageDown => Some(format!("\x1b[6;{modifier}~").into_bytes()),
        // F1-F4: CSI 1;{mod}{P-S}
        KeyCode::F(1) => Some(format!("\x1b[1;{modifier}P").into_bytes()),
        KeyCode::F(2) => Some(format!("\x1b[1;{modifier}Q").into_bytes()),
        KeyCode::F(3) => Some(format!("\x1b[1;{modifier}R").into_bytes()),
        KeyCode::F(4) => Some(format!("\x1b[1;{modifier}S").into_bytes()),
        // F5-F12: CSI {n};{mod}~
        KeyCode::F(n @ 5..=12) => {
            let code = match n {
                5 => 15,
                6 => 17,
                7 => 18,
                8 => 19,
                9 => 20,
                10 => 21,
                11 => 23,
                12 => 24,
                _ => unreachable!(),
            };
            Some(format!("\x1b[{code};{modifier}~").into_bytes())
        }
        _ => None,
    }
}

/// xterm modifier encoding: 1 + shift(1) + alt(2) + ctrl(4)
/// Used for legacy modified special keys (arrows, function keys, etc.)
fn xterm_modifier(mods: KeyModifiers) -> u32 {
    let mut m = 1u32;
    if mods.contains(KeyModifiers::SHIFT) {
        m += 1;
    }
    if mods.contains(KeyModifiers::ALT) {
        m += 2;
    }
    if mods.contains(KeyModifiers::CONTROL) {
        m += 4;
    }
    m
}

/// Kitty protocol modifier encoding: 1 + shift(1) + alt(2) + ctrl(4) + super(8) + hyper(16) + meta(32)
/// Superset of xterm — adds Super/Hyper/Meta bits.
fn kitty_modifier(mods: KeyModifiers) -> u32 {
    let mut m = xterm_modifier(mods);
    if mods.contains(KeyModifiers::SUPER) {
        m += 8;
    }
    if mods.contains(KeyModifiers::HYPER) {
        m += 16;
    }
    if mods.contains(KeyModifiers::META) {
        m += 32;
    }
    m
}

fn encode_legacy_inner(key: KeyEvent) -> Vec<u8> {
    match key.code {
        KeyCode::Char(ch) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                let upper = ch.to_ascii_uppercase();
                match upper {
                    'A'..='Z' => vec![upper as u8 - 64],
                    ' ' | '@' | '2' => vec![0],
                    '[' | '3' => vec![27],
                    '\\' | '4' => vec![28],
                    ']' | '5' => vec![29],
                    '^' | '6' => vec![30],
                    '_' | '7' | '-' => vec![31],
                    _ => vec![ch as u8],
                }
            } else {
                let mut buf = [0u8; 4];
                ch.encode_utf8(&mut buf).as_bytes().to_vec()
            }
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![127],
        KeyCode::Tab => vec![9],
        KeyCode::BackTab => vec![27, 91, 90],
        KeyCode::Esc => vec![27],
        KeyCode::Left => vec![27, 91, 68],
        KeyCode::Right => vec![27, 91, 67],
        KeyCode::Up => vec![27, 91, 65],
        KeyCode::Down => vec![27, 91, 66],
        KeyCode::Home => vec![27, 91, 72],
        KeyCode::End => vec![27, 91, 70],
        KeyCode::PageUp => vec![27, 91, 53, 126],
        KeyCode::PageDown => vec![27, 91, 54, 126],
        KeyCode::Delete => vec![27, 91, 51, 126],
        KeyCode::Insert => vec![27, 91, 50, 126],
        KeyCode::F(n) => encode_f_key(n),
        _ => vec![],
    }
}

fn encode_f_key(n: u8) -> Vec<u8> {
    match n {
        1 => vec![27, 79, 80],
        2 => vec![27, 79, 81],
        3 => vec![27, 79, 82],
        4 => vec![27, 79, 83],
        5 => vec![27, 91, 49, 53, 126],
        6 => vec![27, 91, 49, 55, 126],
        7 => vec![27, 91, 49, 56, 126],
        8 => vec![27, 91, 49, 57, 126],
        9 => vec![27, 91, 50, 48, 126],
        10 => vec![27, 91, 50, 49, 126],
        11 => vec![27, 91, 50, 51, 126],
        12 => vec![27, 91, 50, 52, 126],
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_enter() {
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::empty());
        assert_eq!(encode_key(key, false), vec![b'\r']);
    }

    #[test]
    fn legacy_ctrl_c() {
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(encode_key(key, false), vec![3]);
    }

    #[test]
    fn legacy_shift_enter_is_just_cr() {
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
        // Enter/Tab/Backspace/Esc aren't special keys with xterm modifier encoding,
        // so Shift+Enter falls through to legacy which just sends CR
        assert_eq!(encode_key(key, false), vec![b'\r']);
    }

    #[test]
    fn legacy_alt_up() {
        let key = KeyEvent::new(KeyCode::Up, KeyModifiers::ALT);
        // xterm modified key format: CSI 1;3A (3 = 1 + Alt)
        assert_eq!(encode_key(key, false), b"\x1b[1;3A");
    }

    #[test]
    fn legacy_shift_right() {
        let key = KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT);
        assert_eq!(encode_key(key, false), b"\x1b[1;2C");
    }

    #[test]
    fn legacy_ctrl_left() {
        let key = KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL);
        assert_eq!(encode_key(key, false), b"\x1b[1;5D");
    }

    #[test]
    fn legacy_ctrl_shift_end() {
        let key = KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
        assert_eq!(encode_key(key, false), b"\x1b[1;6F");
    }

    #[test]
    fn legacy_alt_delete() {
        let key = KeyEvent::new(KeyCode::Delete, KeyModifiers::ALT);
        assert_eq!(encode_key(key, false), b"\x1b[3;3~");
    }

    #[test]
    fn legacy_shift_f5() {
        let key = KeyEvent::new(KeyCode::F(5), KeyModifiers::SHIFT);
        assert_eq!(encode_key(key, false), b"\x1b[15;2~");
    }

    #[test]
    fn legacy_alt_char_still_esc_prefix() {
        // Alt+a on character keys still uses ESC prefix (not xterm modified)
        let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::ALT);
        assert_eq!(encode_key(key, false), b"\x1ba");
    }

    #[test]
    fn kitty_shift_enter() {
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
        assert_eq!(encode_key(key, true), b"\x1b[13;2u");
    }

    #[test]
    fn kitty_ctrl_shift_a() {
        let key = KeyEvent::new(
            KeyCode::Char('a'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        assert_eq!(encode_key(key, true), b"\x1b[97;6u");
    }

    #[test]
    fn kitty_alt_enter() {
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT);
        assert_eq!(encode_key(key, true), b"\x1b[13;3u");
    }

    #[test]
    fn kitty_plain_ctrl_c_uses_legacy() {
        // Plain Ctrl+letter is well-represented in legacy
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(encode_key(key, true), vec![3]);
    }

    #[test]
    fn kitty_unmodified_uses_legacy() {
        let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty());
        assert_eq!(encode_key(key, true), b"a");
    }

    #[test]
    fn kitty_shift_tab() {
        let key = KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT);
        assert_eq!(encode_key(key, true), b"\x1b[9;2u");
    }

    #[test]
    fn kitty_ctrl_shift_enter() {
        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
        assert_eq!(encode_key(key, true), b"\x1b[13;6u");
    }
}
