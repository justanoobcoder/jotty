//! Translates key presses into the bytes a terminal application expects,
//! and recognizes jotty's own shortcuts.

use smithay_client_toolkit::seat::keyboard::{Keysym, Modifiers};

use crate::terminal::MouseMode;

/// Terminal modes that change how keys are encoded.
#[derive(Debug, Clone, Copy, Default)]
pub struct KeyModes {
    pub app_cursor: bool,
    pub app_keypad: bool,
}

/// Actions handled by jotty itself instead of being sent to the child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Copy,
    Paste,
    ScrollPageUp,
    ScrollPageDown,
    ScrollToTop,
    ScrollToBottom,
    FontLarger,
    FontSmaller,
    FontReset,
}

pub fn shortcut(keysym: Keysym, mods: Modifiers) -> Option<Action> {
    let ctrl_shift = mods.ctrl && mods.shift && !mods.alt && !mods.logo;
    let shift_only = mods.shift && !mods.ctrl && !mods.alt && !mods.logo;
    let ctrl_only = mods.ctrl && !mods.shift && !mods.alt && !mods.logo;
    match keysym {
        Keysym::C | Keysym::c if ctrl_shift => Some(Action::Copy),
        Keysym::V | Keysym::v if ctrl_shift => Some(Action::Paste),
        Keysym::Insert if shift_only => Some(Action::Paste),
        Keysym::Page_Up if shift_only => Some(Action::ScrollPageUp),
        Keysym::Page_Down if shift_only => Some(Action::ScrollPageDown),
        Keysym::Home if shift_only => Some(Action::ScrollToTop),
        Keysym::End if shift_only => Some(Action::ScrollToBottom),
        Keysym::plus | Keysym::equal | Keysym::KP_Add if ctrl_only || ctrl_shift => {
            Some(Action::FontLarger)
        }
        Keysym::minus | Keysym::underscore | Keysym::KP_Subtract if ctrl_only || ctrl_shift => {
            Some(Action::FontSmaller)
        }
        Keysym::_0 | Keysym::parenright | Keysym::KP_0 if ctrl_only || ctrl_shift => {
            Some(Action::FontReset)
        }
        _ => None,
    }
}

/// The xterm modifier parameter: 1 + shift + 2·alt + 4·ctrl.
fn modifier_param(mods: Modifiers) -> u8 {
    1 + mods.shift as u8 + 2 * mods.alt as u8 + 4 * mods.ctrl as u8
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
        Keysym::F1 => cursor_key('P', mods, true),
        Keysym::F2 => cursor_key('Q', mods, true),
        Keysym::F3 => cursor_key('R', mods, true),
        Keysym::F4 => cursor_key('S', mods, true),
        Keysym::F5 => tilde_key(15, mods),
        Keysym::F6 => tilde_key(17, mods),
        Keysym::F7 => tilde_key(18, mods),
        Keysym::F8 => tilde_key(19, mods),
        Keysym::F9 => tilde_key(20, mods),
        Keysym::F10 => tilde_key(21, mods),
        Keysym::F11 => tilde_key(23, mods),
        Keysym::F12 => tilde_key(24, mods),
        Keysym::BackSpace => {
            let byte = if mods.ctrl { 0x08 } else { 0x7f };
            with_alt(mods, &[byte])
        }
        Keysym::Tab | Keysym::ISO_Left_Tab if mods.shift => b"\x1b[Z".to_vec(),
        Keysym::Tab | Keysym::ISO_Left_Tab => with_alt(mods, b"\t"),
        Keysym::Return => with_alt(mods, b"\r"),
        Keysym::KP_Enter if modes.app_keypad => b"\x1bOM".to_vec(),
        Keysym::KP_Enter => with_alt(mods, b"\r"),
        Keysym::Escape => with_alt(mods, b"\x1b"),
        _ => return encode_text(keysym, text?, mods),
    };
    Some(bytes)
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

    #[test]
    fn shortcuts() {
        assert_eq!(shortcut(Keysym::C, CTRL_SHIFT), Some(Action::Copy));
        assert_eq!(shortcut(Keysym::V, CTRL_SHIFT), Some(Action::Paste));
        assert_eq!(shortcut(Keysym::Insert, SHIFT), Some(Action::Paste));
        assert_eq!(shortcut(Keysym::Page_Up, SHIFT), Some(Action::ScrollPageUp));
        assert_eq!(shortcut(Keysym::equal, CTRL), Some(Action::FontLarger));
        assert_eq!(shortcut(Keysym::minus, CTRL), Some(Action::FontSmaller));
        assert_eq!(shortcut(Keysym::_0, CTRL), Some(Action::FontReset));
        // Plain Ctrl+C and Ctrl+V go to the application.
        assert_eq!(shortcut(Keysym::c, CTRL), None);
        assert_eq!(shortcut(Keysym::v, CTRL), None);
        assert_eq!(shortcut(Keysym::Page_Up, NONE), None);
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
        let enc = |e| encode_mouse(e, MouseMode::Normal, true);
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
        let enc = |e| encode_mouse(e, MouseMode::Normal, false);
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
                .map(|&e| encode_mouse(e, mode, true).is_some())
                .collect::<Vec<_>>()
        };
        assert_eq!(reported(MouseMode::Off), [false, false, false, false]);
        assert_eq!(reported(MouseMode::X10), [true, false, false, false]);
        assert_eq!(reported(MouseMode::Normal), [true, true, false, false]);
        assert_eq!(reported(MouseMode::ButtonMotion), [true, true, true, false]);
        assert_eq!(reported(MouseMode::AnyMotion), [true, true, true, true]);
        // Wheel "releases" are never reported.
        let wheel = mouse(Release(WheelUp), 0, 0, NONE);
        assert_eq!(encode_mouse(wheel, MouseMode::AnyMotion, true), None);
    }

    #[test]
    fn motion_codes() {
        let drag = mouse(Motion(Some(Left)), 3, 4, NONE);
        assert_eq!(
            encode_mouse(drag, MouseMode::ButtonMotion, true).unwrap(),
            b"\x1b[<32;4;5M"
        );
        let hover = mouse(Motion(None), 3, 4, SHIFT);
        assert_eq!(
            encode_mouse(hover, MouseMode::AnyMotion, true).unwrap(),
            b"\x1b[<39;4;5M"
        );
    }

    #[test]
    fn x10_ignores_modifiers() {
        let press = mouse(Press(Left), 0, 0, CTRL);
        assert_eq!(
            encode_mouse(press, MouseMode::X10, true).unwrap(),
            b"\x1b[<0;1;1M"
        );
    }

    #[test]
    fn paste_encoding() {
        assert_eq!(encode_paste("a\nb\r\nc", false), b"a\rb\rc");
        assert_eq!(
            encode_paste("x\x1b[201~y", true),
            b"\x1b[200~x[201~y\x1b[201~"
        );
    }
}
