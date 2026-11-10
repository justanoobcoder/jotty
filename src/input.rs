//! Translates key presses into the bytes a terminal application expects,
//! and recognizes jotty's own shortcuts. Keys use xterm's encoding unless
//! the program turned on the kitty keyboard protocol.

use smithay_client_toolkit::seat::keyboard::{Keysym, Modifiers};

use crate::terminal::MouseMode;

/// Terminal modes that change how keys are encoded.
#[derive(Debug, Clone, Copy, Default)]
pub struct KeyModes {
    pub app_cursor: bool,
    pub app_keypad: bool,
    /// The kitty keyboard protocol flags; 0 for xterm's encoding.
    pub keyboard: u8,
}

/// Kitty keyboard protocol flags (progressive enhancements).
pub mod kitty {
    /// Escape, and keys with Ctrl, Alt or Super, as `CSI u`.
    pub const DISAMBIGUATE: u8 = 1;
    /// Repeat and release events.
    pub const EVENTS: u8 = 2;
    /// The shifted key besides the unshifted one.
    pub const ALTERNATES: u8 = 4;
    /// Every key as an escape code, text included.
    pub const ALL_KEYS: u8 = 8;
    /// The text of a key in its escape code (with `ALL_KEYS`).
    pub const TEXT: u8 = 16;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    Press,
    Repeat,
    Release,
}

/// A key event to encode.
#[derive(Debug, Clone, Copy)]
pub struct Key<'text> {
    pub keysym: Keysym,
    /// The key's symbol without modifiers, in the active layout.
    pub base: Option<Keysym>,
    /// The UTF-8 text xkb produced.
    pub text: Option<&'text str>,
    pub action: KeyAction,
}

/// Encodes a key event with the kitty keyboard protocol when the program
/// turned it on, otherwise as xterm does (only presses and repeats).
pub fn encode(key: &Key, mods: Modifiers, modes: KeyModes) -> Option<Vec<u8>> {
    if modes.keyboard != 0 {
        return encode_kitty(key, mods, modes);
    }
    if key.action == KeyAction::Release {
        return None;
    }
    encode_key(key.keysym, key.text, mods, modes)
}

/// The xterm modifier parameter: 1 + shift + 2·alt + 4·ctrl + 8·super.
fn modifier_param(mods: Modifiers) -> u8 {
    1 + mods.shift as u8 + 2 * mods.alt as u8 + 4 * mods.ctrl as u8 + 8 * mods.logo as u8
}

/// `CSI 1 ; m <final>` with modifiers, otherwise `CSI <final>` or, in
/// application mode, `SS3 <final>`.
fn cursor_key(final_byte: char, mods: Modifiers, app: bool) -> Vec<u8> {
    let m = modifier_param(mods);
    if m > 1 {
        format!("\x1b[1;{m}{final_byte}").into_bytes()
    } else if app {
        format!("\x1bO{final_byte}").into_bytes()
    } else {
        format!("\x1b[{final_byte}").into_bytes()
    }
}

/// `CSI n ~` or `CSI n ; m ~`.
fn tilde_key(n: u8, mods: Modifiers) -> Vec<u8> {
    let m = modifier_param(mods);
    if m > 1 {
        format!("\x1b[{n};{m}~").into_bytes()
    } else {
        format!("\x1b[{n}~").into_bytes()
    }
}

fn with_alt(mods: Modifiers, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 1);
    if mods.alt {
        out.push(0x1b);
    }
    out.extend_from_slice(bytes);
    out
}

/// Encodes a key press. `text` is the key's UTF-8 text as produced by xkb,
/// which already accounts for Shift, Caps Lock and Ctrl on letters.
pub fn encode_key(
    keysym: Keysym,
    text: Option<&str>,
    mods: Modifiers,
    modes: KeyModes,
) -> Option<Vec<u8>> {
    let bytes = match keysym {
        Keysym::Up | Keysym::KP_Up => cursor_key('A', mods, modes.app_cursor),
        Keysym::Down | Keysym::KP_Down => cursor_key('B', mods, modes.app_cursor),
        Keysym::Right | Keysym::KP_Right => cursor_key('C', mods, modes.app_cursor),
        Keysym::Left | Keysym::KP_Left => cursor_key('D', mods, modes.app_cursor),
        Keysym::Home | Keysym::KP_Home => cursor_key('H', mods, modes.app_cursor),
        Keysym::End | Keysym::KP_End => cursor_key('F', mods, modes.app_cursor),
        Keysym::Insert | Keysym::KP_Insert => tilde_key(2, mods),
        Keysym::Delete | Keysym::KP_Delete => tilde_key(3, mods),
        Keysym::Page_Up | Keysym::KP_Page_Up => tilde_key(5, mods),
        Keysym::Page_Down | Keysym::KP_Page_Down => tilde_key(6, mods),
        Keysym::F1 => cursor_key('P', mods, /* app= */ true),
        Keysym::F2 => cursor_key('Q', mods, /* app= */ true),
        Keysym::F3 => cursor_key('R', mods, /* app= */ true),
        Keysym::F4 => cursor_key('S', mods, /* app= */ true),
        Keysym::F5 => tilde_key(15, mods),
        Keysym::F6 => tilde_key(17, mods),
        Keysym::F7 => tilde_key(18, mods),
        Keysym::F8 => tilde_key(19, mods),
        Keysym::F9 => tilde_key(20, mods),
        Keysym::F10 => tilde_key(21, mods),
        Keysym::F11 => tilde_key(23, mods),
        Keysym::F12 => tilde_key(24, mods),
        // xterm's F13-F24 are Shift+F1-F12.
        _ if (Keysym::F13.raw()..=Keysym::F24.raw()).contains(&keysym.raw()) => {
            let base = Keysym::new(keysym.raw() - Keysym::F13.raw() + Keysym::F1.raw());
            let shifted = Modifiers {
                shift: true,
                ..mods
            };
            return encode_key(base, None, shifted, modes);
        }
        Keysym::KP_Begin => cursor_key('E', mods, modes.app_cursor),
        Keysym::BackSpace => {
            let byte = if mods.ctrl { 0x08 } else { 0x7f };
            with_alt(mods, &[byte])
        }
        Keysym::Tab | Keysym::ISO_Left_Tab if mods.shift => with_alt(mods, b"\x1b[Z"),
        Keysym::Tab | Keysym::ISO_Left_Tab => with_alt(mods, b"\t"),
        Keysym::Return => with_alt(mods, b"\r"),
        Keysym::KP_Enter if modes.app_keypad => b"\x1bOM".to_vec(),
        Keysym::KP_Enter => with_alt(mods, b"\r"),
        // With Num Lock on, the keypad types, as in xterm.
        _ if modes.app_keypad && !mods.num_lock && keypad_final(keysym).is_some() => {
            let final_byte = keypad_final(keysym)?;
            format!("\x1bO{final_byte}").into_bytes()
        }
        Keysym::Escape => with_alt(mods, b"\x1b"),
        _ => return encode_text(keysym, text?, mods),
    };
    Some(bytes)
}

/// The final byte of a keypad operator key in application keypad mode.
fn keypad_final(keysym: Keysym) -> Option<char> {
    Some(match keysym {
        Keysym::KP_Multiply => 'j',
        Keysym::KP_Add => 'k',
        Keysym::KP_Separator => 'l',
        Keysym::KP_Subtract => 'm',
        Keysym::KP_Divide => 'o',
        Keysym::KP_Equal => 'X',
        _ => return None,
    })
}

fn encode_text(keysym: Keysym, text: &str, mods: Modifiers) -> Option<Vec<u8>> {
    if text.is_empty() {
        return None;
    }
    let mut chars = text.chars();
    let (first, single) = (chars.next()?, chars.next().is_none());
    if mods.ctrl
        && single
        && let Some(control) = control_byte(first, keysym)
    {
        return Some(with_alt(mods, &[control]));
    }
    Some(with_alt(mods, text.as_bytes()))
}

/// The control character for Ctrl+`c`, for keys xkb leaves untranslated.
fn control_byte(c: char, keysym: Keysym) -> Option<u8> {
    match c {
        // xkb already produced a control character.
        '\0'..='\x1f' | '\x7f' => Some(c as u8),
        'a'..='z' | 'A'..='Z' => Some(c.to_ascii_uppercase() as u8 & 0x1f),
        ' ' | '@' | '2' => Some(0x00),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '7' | '/' => Some(0x1f),
        '8' | '?' => Some(0x7f),
        _ if keysym == Keysym::space => Some(0x00),
        _ => None,
    }
}

/// The kitty protocol's number for a key without text, and the final byte
/// of its escape code.
fn functional_key(keysym: Keysym) -> Option<(u32, u8)> {
    let raw = keysym.raw();
    Some(match keysym {
        Keysym::Escape => (27, b'u'),
        Keysym::Return => (13, b'u'),
        Keysym::Tab | Keysym::ISO_Left_Tab => (9, b'u'),
        Keysym::BackSpace => (127, b'u'),
        Keysym::Insert => (2, b'~'),
        Keysym::Delete => (3, b'~'),
        Keysym::Left => (1, b'D'),
        Keysym::Right => (1, b'C'),
        Keysym::Up => (1, b'A'),
        Keysym::Down => (1, b'B'),
        Keysym::Page_Up => (5, b'~'),
        Keysym::Page_Down => (6, b'~'),
        Keysym::Home => (1, b'H'),
        Keysym::End => (1, b'F'),
        Keysym::Caps_Lock => (57358, b'u'),
        Keysym::Scroll_Lock => (57359, b'u'),
        Keysym::Num_Lock => (57360, b'u'),
        Keysym::Print => (57361, b'u'),
        Keysym::Pause => (57362, b'u'),
        Keysym::Menu => (57363, b'u'),
        Keysym::F1 => (1, b'P'),
        Keysym::F2 => (1, b'Q'),
        Keysym::F3 => (13, b'~'),
        Keysym::F4 => (1, b'S'),
        Keysym::F5 => (15, b'~'),
        Keysym::F6 => (17, b'~'),
        Keysym::F7 => (18, b'~'),
        Keysym::F8 => (19, b'~'),
        Keysym::F9 => (20, b'~'),
        Keysym::F10 => (21, b'~'),
        Keysym::F11 => (23, b'~'),
        Keysym::F12 => (24, b'~'),
        Keysym::KP_Decimal => (57409, b'u'),
        Keysym::KP_Divide => (57410, b'u'),
        Keysym::KP_Multiply => (57411, b'u'),
        Keysym::KP_Subtract => (57412, b'u'),
        Keysym::KP_Add => (57413, b'u'),
        Keysym::KP_Enter => (57414, b'u'),
        Keysym::KP_Equal => (57415, b'u'),
        Keysym::KP_Separator => (57416, b'u'),
        Keysym::KP_Left => (57417, b'u'),
        Keysym::KP_Right => (57418, b'u'),
        Keysym::KP_Up => (57419, b'u'),
        Keysym::KP_Down => (57420, b'u'),
        Keysym::KP_Page_Up => (57421, b'u'),
        Keysym::KP_Page_Down => (57422, b'u'),
        Keysym::KP_Home => (57423, b'u'),
        Keysym::KP_End => (57424, b'u'),
        Keysym::KP_Insert => (57425, b'u'),
        Keysym::KP_Delete => (57426, b'u'),
        Keysym::KP_Begin => (1, b'E'),
        Keysym::Shift_L => (57441, b'u'),
        Keysym::Control_L => (57442, b'u'),
        Keysym::Alt_L => (57443, b'u'),
        Keysym::Super_L => (57444, b'u'),
        Keysym::Hyper_L => (57445, b'u'),
        Keysym::Meta_L => (57446, b'u'),
        Keysym::Shift_R => (57447, b'u'),
        Keysym::Control_R => (57448, b'u'),
        Keysym::Alt_R => (57449, b'u'),
        Keysym::Super_R => (57450, b'u'),
        Keysym::Hyper_R => (57451, b'u'),
        Keysym::Meta_R => (57452, b'u'),
        Keysym::ISO_Level3_Shift => (57453, b'u'),
        Keysym::ISO_Level5_Shift => (57454, b'u'),
        // F13–F35 and KP_0–KP_9 are consecutive in both numberings.
        _ if (Keysym::F13.raw()..=Keysym::F35.raw()).contains(&raw) => {
            (57376 + raw - Keysym::F13.raw(), b'u')
        }
        _ if (Keysym::KP_0.raw()..=Keysym::KP_9.raw()).contains(&raw) => {
            (57399 + raw - Keysym::KP_0.raw(), b'u')
        }
        _ => {
            // Media keys (XF86Audio…).
            let number = match raw {
                0x1008_ff14 => 57428,
                0x1008_ff31 => 57429,
                0x1008_ff15 => 57432,
                0x1008_ff97 => 57433,
                0x1008_ff3e => 57434,
                0x1008_ff17 => 57435,
                0x1008_ff16 => 57436,
                0x1008_ff1c => 57437,
                0x1008_ff11 => 57438,
                0x1008_ff13 => 57439,
                0x1008_ff12 => 57440,
                _ => return None,
            };
            (number, b'u')
        }
    })
}

/// The kitty protocol's modifier bits: shift, alt, ctrl, super, then caps
/// lock (64) and num lock (128).
fn kitty_modifiers(mods: Modifiers) -> u32 {
    u32::from(mods.shift)
        | u32::from(mods.alt) << 1
        | u32::from(mods.ctrl) << 2
        | u32::from(mods.logo) << 3
        | u32::from(mods.caps_lock) << 6
        | u32::from(mods.num_lock) << 7
}

/// The bit a modifier key sets, so its own press or release can be
/// reflected (the compositor reports the new state only afterwards).
fn own_modifier(keysym: Keysym) -> u32 {
    match keysym {
        Keysym::Shift_L | Keysym::Shift_R => 1,
        Keysym::Alt_L | Keysym::Alt_R => 2,
        Keysym::Control_L | Keysym::Control_R => 4,
        Keysym::Super_L | Keysym::Super_R => 8,
        Keysym::Caps_Lock => 64,
        Keysym::Num_Lock => 128,
        _ => 0,
    }
}

/// Encodes a key with the kitty keyboard protocol, following the flags in
/// `modes.keyboard`.
fn encode_kitty(key: &Key, mods: Modifiers, modes: KeyModes) -> Option<Vec<u8>> {
    use kitty::{ALL_KEYS, ALTERNATES, DISAMBIGUATE, EVENTS, TEXT};
    let flags = modes.keyboard;
    let all_keys = flags & ALL_KEYS != 0;
    let disambiguate = flags & DISAMBIGUATE != 0 || all_keys;
    let events = flags & EVENTS != 0;
    let release = key.action == KeyAction::Release;
    if release && !events {
        return None;
    }
    let legacy = || {
        if release {
            None
        } else {
            encode_key(key.keysym, key.text, mods, modes)
        }
    };
    // Text without control characters, which a key event can carry.
    let text = key
        .text
        .filter(|t| !t.is_empty() && !t.chars().any(char::is_control));
    let functional = functional_key(key.keysym);
    let is_keypad = (Keysym::KP_Space.raw()..=Keysym::KP_Equal.raw()).contains(&key.keysym.raw());
    // A key that types text: a character key, or a keypad key with Num Lock.
    let types_text = functional.is_none() || (is_keypad && text.is_some());
    let mut bits = kitty_modifiers(mods);
    let own = own_modifier(key.keysym);
    let is_modifier = own != 0 || key.keysym.is_modifier_key();
    if is_modifier {
        if !all_keys {
            return None;
        }
        if release {
            bits &= !own;
        } else {
            bits |= own;
        }
    }
    let held = mods.ctrl || mods.alt || mods.logo;

    if !all_keys {
        if types_text {
            // Text goes as is; events aren't reported for it.
            if release {
                return None;
            }
            if !held {
                return text.map(|t| t.as_bytes().to_vec());
            }
            if !disambiguate {
                return legacy();
            }
            // Lock keys don't count for text keys.
            bits &= !(64 | 128);
        } else if matches!(functional, Some((13 | 9 | 127, b'u'))) {
            // Enter, Tab and Backspace stay as they were without other
            // modifiers, so `reset` can be typed if a program leaves the
            // mode on; they have no release events.
            if release || !disambiguate || !(held || mods.shift) {
                return legacy();
            }
        } else if !disambiguate && !release {
            return legacy();
        }
    }

    let (number, final_byte) = match functional {
        Some(f) if !types_text || is_keypad => f,
        _ => (key_number(key).or(text.map(|_| 0))?, b'u'),
    };
    let mut out = format!("\x1b[{number}");
    if flags & ALTERNATES != 0 && types_text && mods.shift {
        let shifted = key.keysym.key_char().map(u32::from);
        if let Some(shifted) = shifted.filter(|&c| c != number) {
            out.push_str(&format!(":{shifted}"));
        }
    }
    let event = match key.action {
        KeyAction::Press => 1,
        KeyAction::Repeat if events => 2,
        KeyAction::Repeat => 1,
        KeyAction::Release => 3,
    };
    let text = text.filter(|_| all_keys && flags & TEXT != 0 && !release);
    if bits != 0 || event != 1 || text.is_some() {
        out.push_str(&format!(";{}", bits + 1));
        if event != 1 {
            out.push_str(&format!(":{event}"));
        }
    }
    if let Some(text) = text {
        let codes: Vec<String> = text.chars().map(|c| u32::from(c).to_string()).collect();
        out.push_str(&format!(";{}", codes.join(":")));
    }
    // `CSI 1 A` is sent as `CSI A` when nothing follows the number.
    if final_byte != b'u' && final_byte != b'~' && out == "\x1b[1" {
        out.truncate(2);
    }
    out.push(final_byte as char);
    Some(out.into_bytes())
}

/// The Unicode code point of a character key without modifiers (lower
/// case), from the active layout.
fn key_number(key: &Key) -> Option<u32> {
    let c = key
        .base
        .and_then(Keysym::key_char)
        .or_else(|| key.keysym.key_char())
        .filter(|c| !c.is_control())?;
    let mut lower = c.to_lowercase();
    let c = match (lower.next(), lower.next()) {
        (Some(l), None) => l,
        _ => c,
    };
    Some(u32::from(c))
}

/// Wraps pasted text for the child: strips control characters that could
/// end bracketed paste early, and normalizes newlines to carriage returns.
pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    let cleaned: String = text
        .replace("\r\n", "\r")
        .chars()
        .map(|c| if c == '\n' { '\r' } else { c })
        .filter(|&c| !bracketed || c != '\x1b')
        .collect();
    if bracketed {
        let mut out = b"\x1b[200~".to_vec();
        out.extend_from_slice(cleaned.as_bytes());
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        cleaned.into_bytes()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
    WheelLeft,
    WheelRight,
}

impl MouseButton {
    /// xterm's button number.
    fn code(self) -> u8 {
        match self {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            MouseButton::WheelUp => 64,
            MouseButton::WheelDown => 65,
            MouseButton::WheelLeft => 66,
            MouseButton::WheelRight => 67,
        }
    }

    fn is_wheel(self) -> bool {
        self.code() >= 64
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    Press(MouseButton),
    Release(MouseButton),
    /// Pointer motion, with the button held down, if any.
    Motion(Option<MouseButton>),
}

/// A pointer event on the cell at `col`, `row` (0-based, screen
/// coordinates).
#[derive(Debug, Clone, Copy)]
pub struct MouseEvent {
    pub action: MouseAction,
    pub col: usize,
    pub row: usize,
    pub mods: Modifiers,
}

/// Encodes a pointer event for a program that enabled mouse reporting, or
/// returns `None` if `mode` doesn't report this kind of event.
pub fn encode_mouse(event: MouseEvent, mode: MouseMode, sgr: bool) -> Option<Vec<u8>> {
    use MouseAction::*;
    let reported = match (mode, event.action) {
        (MouseMode::Off, _) => false,
        (_, Release(button)) if button.is_wheel() => false,
        (MouseMode::X10, action) => matches!(action, Press(_)),
        (MouseMode::Normal, action) => !matches!(action, Motion(_)),
        (MouseMode::ButtonMotion, action) => action != Motion(None),
        (MouseMode::AnyMotion, _) => true,
    };
    if !reported {
        return None;
    }

    let mut code = match event.action {
        Press(button) => button.code(),
        // The legacy encoding can't tell which button was released.
        Release(button) if sgr => button.code(),
        Release(_) => 3,
        Motion(Some(button)) => button.code() + 32,
        Motion(None) => 3 + 32,
    };
    if mode != MouseMode::X10 {
        let m = event.mods;
        code += 4 * u8::from(m.shift) + 8 * u8::from(m.alt) + 16 * u8::from(m.ctrl);
    }

    let (x, y) = (event.col + 1, event.row + 1);
    if sgr {
        let end = if matches!(event.action, Release(_)) {
            'm'
        } else {
            'M'
        };
        return Some(format!("\x1b[<{code};{x};{y}{end}").into_bytes());
    }
    // Each value is one byte offset by 32, so positions stop at 223.
    if x > 223 || y > 223 {
        return None;
    }
    Some(vec![
        0x1b,
        b'[',
        b'M',
        32 + code,
        32 + x as u8,
        32 + y as u8,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_keys_xterm_also_sends() {
        let none = Modifiers::default();
        let modes = KeyModes::default();
        let key = |k, m| encode_key(k, None, m, modes);
        assert_eq!(key(Keysym::F13, none).unwrap(), b"\x1b[1;2P");
        assert_eq!(key(Keysym::F24, none).unwrap(), b"\x1b[24;2~");
        assert_eq!(key(Keysym::KP_Begin, none).unwrap(), b"\x1b[E");
        let logo = Modifiers { logo: true, ..none };
        assert_eq!(key(Keysym::Up, logo).unwrap(), b"\x1b[1;9A");
        let shift_alt = Modifiers {
            shift: true,
            alt: true,
            ..none
        };
        assert_eq!(key(Keysym::ISO_Left_Tab, shift_alt).unwrap(), b"\x1b\x1b[Z");
    }

    #[test]
    fn application_keypad_operators_use_ss3_without_num_lock() {
        let modes = KeyModes {
            app_keypad: true,
            ..KeyModes::default()
        };
        let none = Modifiers::default();
        assert_eq!(
            encode_key(Keysym::KP_Add, Some("+"), none, modes).unwrap(),
            b"\x1bOk"
        );
        let num_lock = Modifiers {
            num_lock: true,
            ..none
        };
        assert_eq!(
            encode_key(Keysym::KP_Add, Some("+"), num_lock, modes).unwrap(),
            b"+"
        );
    }

    const NONE: Modifiers = Modifiers {
        ctrl: false,
        alt: false,
        shift: false,
        caps_lock: false,
        logo: false,
        num_lock: false,
    };
    const CTRL: Modifiers = Modifiers { ctrl: true, ..NONE };
    const ALT: Modifiers = Modifiers { alt: true, ..NONE };
    const SHIFT: Modifiers = Modifiers {
        shift: true,
        ..NONE
    };
    const CTRL_SHIFT: Modifiers = Modifiers {
        ctrl: true,
        shift: true,
        ..NONE
    };

    fn enc(keysym: Keysym, text: Option<&str>, mods: Modifiers) -> Vec<u8> {
        encode_key(keysym, text, mods, KeyModes::default()).unwrap()
    }

    /// Encodes a press, repeat or release with kitty keyboard `flags`.
    fn kitty(
        keysym: Keysym,
        base: Keysym,
        text: Option<&str>,
        mods: Modifiers,
        flags: u8,
        action: KeyAction,
    ) -> Option<String> {
        let key = Key {
            keysym,
            base: Some(base),
            text,
            action,
        };
        let modes = KeyModes {
            keyboard: flags,
            ..KeyModes::default()
        };
        encode(&key, mods, modes).map(|b| String::from_utf8(b).unwrap())
    }

    fn press(keysym: Keysym, text: Option<&str>, mods: Modifiers, flags: u8) -> Option<String> {
        kitty(keysym, keysym, text, mods, flags, KeyAction::Press)
    }

    #[test]
    fn disambiguate_keeps_plain_keys_and_encodes_the_rest() {
        let f = kitty::DISAMBIGUATE;
        assert_eq!(press(Keysym::a, Some("a"), NONE, f).unwrap(), "a");
        assert_eq!(
            kitty(Keysym::A, Keysym::a, Some("A"), SHIFT, f, KeyAction::Press).unwrap(),
            "A"
        );
        assert_eq!(press(Keysym::Return, Some("\r"), NONE, f).unwrap(), "\r");
        assert_eq!(
            press(Keysym::Return, Some("\r"), SHIFT, f).unwrap(),
            "\x1b[13;2u"
        );
        assert_eq!(press(Keysym::BackSpace, None, NONE, f).unwrap(), "\x7f");
        assert_eq!(press(Keysym::Tab, None, NONE, f).unwrap(), "\t");
        assert_eq!(
            press(Keysym::ISO_Left_Tab, None, SHIFT, f).unwrap(),
            "\x1b[9;2u"
        );
        assert_eq!(press(Keysym::Escape, None, NONE, f).unwrap(), "\x1b[27u");
        assert_eq!(
            press(Keysym::c, Some("\x03"), CTRL, f).unwrap(),
            "\x1b[99;5u"
        );
        assert_eq!(press(Keysym::a, Some("a"), ALT, f).unwrap(), "\x1b[97;3u");
        assert_eq!(
            kitty(
                Keysym::A,
                Keysym::a,
                Some("\x01"),
                CTRL_SHIFT,
                f,
                KeyAction::Press
            )
            .unwrap(),
            "\x1b[97;6u"
        );
        assert_eq!(press(Keysym::Up, None, NONE, f).unwrap(), "\x1b[A");
        assert_eq!(press(Keysym::Up, None, CTRL, f).unwrap(), "\x1b[1;5A");
        assert_eq!(press(Keysym::F3, None, NONE, f).unwrap(), "\x1b[13~");
        assert_eq!(press(Keysym::F13, None, NONE, f).unwrap(), "\x1b[57376u");
        assert_eq!(press(Keysym::Page_Up, None, SHIFT, f).unwrap(), "\x1b[5;2~");
        // Keypad keys: text with Num Lock, their own numbers otherwise.
        assert_eq!(press(Keysym::KP_1, Some("1"), NONE, f).unwrap(), "1");
        assert_eq!(
            press(Keysym::KP_Enter, Some("\r"), NONE, f).unwrap(),
            "\x1b[57414u"
        );
        assert_eq!(
            press(Keysym::KP_1, Some("1"), CTRL, f).unwrap(),
            "\x1b[57400;5u"
        );
        // Modifier keys and releases are not reported.
        assert_eq!(press(Keysym::Shift_L, None, NONE, f), None);
        assert_eq!(
            kitty(Keysym::Up, Keysym::Up, None, NONE, f, KeyAction::Release),
            None
        );
    }

    #[test]
    fn lock_modifiers_only_count_for_keys_without_text() {
        let f = kitty::DISAMBIGUATE;
        let caps = Modifiers {
            caps_lock: true,
            num_lock: true,
            ..CTRL
        };
        assert_eq!(
            press(Keysym::a, Some("\x01"), caps, f).unwrap(),
            "\x1b[97;5u"
        );
        assert_eq!(
            press(Keysym::Escape, None, caps, f).unwrap(),
            "\x1b[27;197u"
        );
        let num = Modifiers {
            num_lock: true,
            ..NONE
        };
        assert_eq!(press(Keysym::Return, Some("\r"), num, f).unwrap(), "\r");
    }

    #[test]
    fn event_types() {
        let f = kitty::DISAMBIGUATE | kitty::EVENTS;
        let up = |action| kitty(Keysym::Up, Keysym::Up, None, NONE, f, action);
        assert_eq!(up(KeyAction::Press).unwrap(), "\x1b[A");
        assert_eq!(up(KeyAction::Repeat).unwrap(), "\x1b[1;1:2A");
        assert_eq!(up(KeyAction::Release).unwrap(), "\x1b[1;1:3A");
        let release = |keysym, text| kitty(keysym, keysym, text, NONE, f, KeyAction::Release);
        assert_eq!(release(Keysym::a, Some("a")), None);
        assert_eq!(release(Keysym::Return, Some("\r")), None);
        assert_eq!(release(Keysym::Escape, None).unwrap(), "\x1b[27;1:3u");
    }

    #[test]
    fn alternate_keys() {
        let f = kitty::DISAMBIGUATE | kitty::ALTERNATES;
        assert_eq!(
            kitty(
                Keysym::A,
                Keysym::a,
                Some("\x01"),
                CTRL_SHIFT,
                f,
                KeyAction::Press
            )
            .unwrap(),
            "\x1b[97:65;6u"
        );
        assert_eq!(
            press(Keysym::a, Some("\x01"), CTRL, f).unwrap(),
            "\x1b[97;5u"
        );
    }

    #[test]
    fn all_keys_with_text() {
        let f = kitty::DISAMBIGUATE | kitty::EVENTS | kitty::ALL_KEYS | kitty::TEXT;
        assert_eq!(
            press(Keysym::a, Some("a"), NONE, f).unwrap(),
            "\x1b[97;1;97u"
        );
        assert_eq!(
            kitty(Keysym::A, Keysym::a, Some("A"), SHIFT, f, KeyAction::Press).unwrap(),
            "\x1b[97;2;65u"
        );
        assert_eq!(
            kitty(Keysym::a, Keysym::a, Some("a"), NONE, f, KeyAction::Release).unwrap(),
            "\x1b[97;1:3u"
        );
        assert_eq!(
            press(Keysym::Return, Some("\r"), NONE, f).unwrap(),
            "\x1b[13u"
        );
        // A modifier key reports its own state change.
        assert_eq!(
            press(Keysym::Shift_L, None, NONE, f).unwrap(),
            "\x1b[57441;2u"
        );
        assert_eq!(
            kitty(
                Keysym::Shift_L,
                Keysym::Shift_L,
                None,
                SHIFT,
                f,
                KeyAction::Release
            )
            .unwrap(),
            "\x1b[57441;1:3u"
        );
    }

    #[test]
    fn legacy_encoding_without_flags() {
        assert_eq!(press(Keysym::Return, Some("\r"), SHIFT, 0).unwrap(), "\r");
        assert_eq!(
            kitty(Keysym::a, Keysym::a, Some("a"), NONE, 0, KeyAction::Release),
            None
        );
    }

    #[test]
    fn plain_text() {
        assert_eq!(enc(Keysym::a, Some("a"), NONE), b"a");
        assert_eq!(enc(Keysym::A, Some("A"), SHIFT), b"A");
        assert_eq!(enc(Keysym::ediaeresis, Some("ë"), NONE), "ë".as_bytes());
    }

    #[test]
    fn no_text_no_bytes() {
        assert_eq!(
            encode_key(Keysym::Shift_L, None, SHIFT, KeyModes::default()),
            None
        );
        assert_eq!(
            encode_key(Keysym::a, Some(""), NONE, KeyModes::default()),
            None
        );
    }

    #[test]
    fn control_characters() {
        assert_eq!(enc(Keysym::c, Some("\x03"), CTRL), b"\x03");
        assert_eq!(enc(Keysym::c, Some("c"), CTRL), b"\x03");
        assert_eq!(enc(Keysym::space, Some(" "), CTRL), b"\x00");
        assert_eq!(enc(Keysym::bracketleft, Some("["), CTRL), b"\x1b");
        assert_eq!(enc(Keysym::slash, Some("/"), CTRL), b"\x1f");
    }

    #[test]
    fn alt_prefixes_escape() {
        assert_eq!(enc(Keysym::b, Some("b"), ALT), b"\x1bb");
        assert_eq!(enc(Keysym::BackSpace, None, ALT), b"\x1b\x7f");
        assert_eq!(
            enc(
                Keysym::x,
                Some("x"),
                Modifiers {
                    alt: true,
                    ctrl: true,
                    ..NONE
                }
            ),
            b"\x1b\x18"
        );
    }

    #[test]
    fn special_keys() {
        assert_eq!(enc(Keysym::Return, None, NONE), b"\r");
        assert_eq!(enc(Keysym::BackSpace, None, NONE), b"\x7f");
        assert_eq!(enc(Keysym::BackSpace, None, CTRL), b"\x08");
        assert_eq!(enc(Keysym::Tab, None, NONE), b"\t");
        assert_eq!(enc(Keysym::ISO_Left_Tab, None, SHIFT), b"\x1b[Z");
        assert_eq!(enc(Keysym::Escape, None, NONE), b"\x1b");
    }

    #[test]
    fn cursor_keys_follow_mode() {
        assert_eq!(enc(Keysym::Up, None, NONE), b"\x1b[A");
        let app = KeyModes {
            app_cursor: true,
            ..KeyModes::default()
        };
        assert_eq!(encode_key(Keysym::Up, None, NONE, app).unwrap(), b"\x1bOA");
        assert_eq!(
            encode_key(Keysym::Home, None, NONE, app).unwrap(),
            b"\x1bOH"
        );
        // Modified keys always use the CSI form.
        assert_eq!(
            encode_key(Keysym::Left, None, CTRL, app).unwrap(),
            b"\x1b[1;5D"
        );
        assert_eq!(enc(Keysym::Right, None, CTRL_SHIFT), b"\x1b[1;6C");
    }

    #[test]
    fn tilde_and_function_keys() {
        assert_eq!(enc(Keysym::Delete, None, NONE), b"\x1b[3~");
        assert_eq!(enc(Keysym::Page_Up, None, CTRL), b"\x1b[5;5~");
        assert_eq!(enc(Keysym::F1, None, NONE), b"\x1bOP");
        assert_eq!(enc(Keysym::F1, None, SHIFT), b"\x1b[1;2P");
        assert_eq!(enc(Keysym::F5, None, NONE), b"\x1b[15~");
        assert_eq!(enc(Keysym::F12, None, ALT), b"\x1b[24;3~");
    }

    #[test]
    fn keypad_enter() {
        let app = KeyModes {
            app_keypad: true,
            ..KeyModes::default()
        };
        assert_eq!(enc(Keysym::KP_Enter, None, NONE), b"\r");
        assert_eq!(
            encode_key(Keysym::KP_Enter, None, NONE, app).unwrap(),
            b"\x1bOM"
        );
    }

    fn mouse(action: MouseAction, col: usize, row: usize, mods: Modifiers) -> MouseEvent {
        MouseEvent {
            action,
            col,
            row,
            mods,
        }
    }

    use MouseAction::{Motion, Press, Release};
    use MouseButton::{Left, Right, WheelDown, WheelUp};

    #[test]
    fn sgr_mouse_encoding() {
        let enc = |e| encode_mouse(e, MouseMode::Normal, /* sgr= */ true);
        assert_eq!(
            enc(mouse(Press(Left), 0, 0, NONE)).unwrap(),
            b"\x1b[<0;1;1M"
        );
        assert_eq!(
            enc(mouse(Release(Left), 4, 9, NONE)).unwrap(),
            b"\x1b[<0;5;10m"
        );
        assert_eq!(
            enc(mouse(Press(Right), 299, 99, NONE)).unwrap(),
            b"\x1b[<2;300;100M"
        );
        assert_eq!(
            enc(mouse(Press(WheelUp), 1, 1, NONE)).unwrap(),
            b"\x1b[<64;2;2M"
        );
        assert_eq!(
            enc(mouse(Press(WheelDown), 1, 1, CTRL)).unwrap(),
            b"\x1b[<81;2;2M"
        );
        assert_eq!(
            enc(mouse(Press(Left), 0, 0, CTRL_SHIFT)).unwrap(),
            b"\x1b[<20;1;1M"
        );
        assert_eq!(enc(mouse(Press(Left), 0, 0, ALT)).unwrap(), b"\x1b[<8;1;1M");
    }

    #[test]
    fn legacy_mouse_encoding() {
        let enc = |e| encode_mouse(e, MouseMode::Normal, /* sgr= */ false);
        assert_eq!(enc(mouse(Press(Left), 0, 0, NONE)).unwrap(), b"\x1b[M !!");
        // Releases don't say which button.
        assert_eq!(
            enc(mouse(Release(Right), 2, 3, NONE)).unwrap(),
            b"\x1b[M##$"
        );
        // Positions past 223 can't be encoded.
        assert_eq!(enc(mouse(Press(Left), 223, 0, NONE)), None);
    }

    #[test]
    fn modes_filter_events() {
        let all = [
            mouse(Press(Left), 0, 0, NONE),
            mouse(Release(Left), 0, 0, NONE),
            mouse(Motion(Some(Left)), 0, 0, NONE),
            mouse(Motion(None), 0, 0, NONE),
        ];
        let reported = |mode| {
            all.iter()
                .map(|&e| encode_mouse(e, mode, /* sgr= */ true).is_some())
                .collect::<Vec<_>>()
        };
        assert_eq!(reported(MouseMode::Off), [false, false, false, false]);
        assert_eq!(reported(MouseMode::X10), [true, false, false, false]);
        assert_eq!(reported(MouseMode::Normal), [true, true, false, false]);
        assert_eq!(reported(MouseMode::ButtonMotion), [true, true, true, false]);
        assert_eq!(reported(MouseMode::AnyMotion), [true, true, true, true]);
        // Wheel "releases" are never reported.
        let wheel = mouse(Release(WheelUp), 0, 0, NONE);
        assert_eq!(
            encode_mouse(wheel, MouseMode::AnyMotion, /* sgr= */ true),
            None
        );
    }

    #[test]
    fn motion_codes() {
        let drag = mouse(Motion(Some(Left)), 3, 4, NONE);
        assert_eq!(
            encode_mouse(drag, MouseMode::ButtonMotion, /* sgr= */ true).unwrap(),
            b"\x1b[<32;4;5M"
        );
        let hover = mouse(Motion(None), 3, 4, SHIFT);
        assert_eq!(
            encode_mouse(hover, MouseMode::AnyMotion, /* sgr= */ true).unwrap(),
            b"\x1b[<39;4;5M"
        );
    }

    #[test]
    fn x10_ignores_modifiers() {
        let press = mouse(Press(Left), 0, 0, CTRL);
        assert_eq!(
            encode_mouse(press, MouseMode::X10, /* sgr= */ true).unwrap(),
            b"\x1b[<0;1;1M"
        );
    }

    #[test]
    fn paste_encoding() {
        assert_eq!(
            encode_paste("a\nb\r\nc", /* bracketed= */ false),
            b"a\rb\rc"
        );
        assert_eq!(
            encode_paste("x\x1b[201~y", /* bracketed= */ true),
            b"\x1b[200~x[201~y\x1b[201~"
        );
    }
}
