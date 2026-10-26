//! Configurable key bindings for jotty's own actions.
//!
//! Bindings are written as `mod+mod+key`, e.g. `ctrl+shift+c`, where `key`
//! is an xkb keysym name (`c`, `page_up`, `plus`, `F5`, ...) or a single
//! character.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::Deserialize;
use smithay_client_toolkit::seat::keyboard::{Keysym, Modifiers};
use xkbcommon::xkb;

/// Actions handled by jotty itself instead of being sent to the child.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Action {
    Copy,
    /// Copy if text is selected, otherwise send the key to the program
    /// (Ctrl+C then interrupts).
    CopyOrInterrupt,
    Paste,
    ScrollLineUp,
    ScrollLineDown,
    ScrollPageUp,
    ScrollPageDown,
    ScrollToTop,
    ScrollToBottom,
    FontLarger,
    FontSmaller,
    FontReset,
    /// Search the scrollback.
    Search,
    /// Open another jotty in the current working directory.
    NewWindow,
    /// Turn vi mode on or off.
    ViMode,
    /// Read the config file again.
    ReloadConfig,
}

impl Action {
    const ALL: [(&'static str, Action); 16] = [
        ("copy", Action::Copy),
        ("copy_or_interrupt", Action::CopyOrInterrupt),
        ("paste", Action::Paste),
        ("scroll_line_up", Action::ScrollLineUp),
        ("scroll_line_down", Action::ScrollLineDown),
        ("scroll_page_up", Action::ScrollPageUp),
        ("scroll_page_down", Action::ScrollPageDown),
        ("scroll_to_top", Action::ScrollToTop),
        ("scroll_to_bottom", Action::ScrollToBottom),
        ("font_larger", Action::FontLarger),
        ("font_smaller", Action::FontSmaller),
        ("font_reset", Action::FontReset),
        ("search", Action::Search),
        ("new_window", Action::NewWindow),
        ("vi_mode", Action::ViMode),
        ("reload_config", Action::ReloadConfig),
    ];

    fn from_name(name: &str) -> Option<Action> {
        Self::ALL.iter().find(|(n, _)| *n == name).map(|&(_, a)| a)
    }

    /// The bindings used when the config doesn't mention the action.
    fn defaults(self) -> &'static [&'static str] {
        match self {
            Action::Copy => &["ctrl+shift+c"],
            Action::Paste => &["ctrl+shift+v", "shift+insert"],
            Action::CopyOrInterrupt | Action::ScrollLineUp | Action::ScrollLineDown => &[],
            Action::ScrollPageUp => &["shift+page_up"],
            Action::ScrollPageDown => &["shift+page_down"],
            Action::ScrollToTop => &["shift+home"],
            Action::ScrollToBottom => &["shift+end"],
            Action::FontLarger => &["ctrl+plus", "ctrl+equal", "ctrl+kp_add"],
            Action::FontSmaller => &["ctrl+minus", "ctrl+kp_subtract"],
            Action::FontReset => &["ctrl+0", "ctrl+kp_0"],
            Action::Search => &["ctrl+shift+f"],
            Action::NewWindow => &["ctrl+shift+n"],
            Action::ViMode => &["ctrl+shift+space"],
            Action::ReloadConfig => &["ctrl+shift+r"],
        }
    }
}

/// The modifiers that take part in bindings (lock keys don't).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub logo: bool,
}

impl From<Modifiers> for Mods {
    fn from(m: Modifiers) -> Mods {
        Mods {
            ctrl: m.ctrl,
            alt: m.alt,
            shift: m.shift,
            logo: m.logo,
        }
    }
}

/// A key with modifiers, as written in the config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyCombo {
    pub mods: Mods,
    /// Always the lowercase form, so `ctrl+shift+C` and `ctrl+shift+c` agree.
    pub keysym: Keysym,
}

/// The lowercase keysym for letters, the keysym itself otherwise.
fn lower(keysym: Keysym) -> Keysym {
    match keysym.key_char() {
        Some(c) if c.is_uppercase() => {
            let mut lower = c.to_lowercase();
            match (lower.next(), lower.next()) {
                (Some(l), None) => Keysym::from_char(l),
                _ => keysym,
            }
        }
        _ => keysym,
    }
}

impl FromStr for KeyCombo {
    type Err = String;

    fn from_str(s: &str) -> Result<KeyCombo, String> {
        let (mod_names, key) = match s.rsplit_once('+') {
            // "ctrl++" binds the plus key.
            Some((rest, "")) if rest.ends_with('+') => (&rest[..rest.len() - 1], "+"),
            Some((mods, key)) => (mods, key),
            None => ("", s),
        };
        let mut mods = Mods::default();
        for name in mod_names.split('+').filter(|n| !n.is_empty()) {
            let flag = match name.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => &mut mods.ctrl,
                "alt" | "meta" | "mod1" => &mut mods.alt,
                "shift" => &mut mods.shift,
                "super" | "logo" | "mod4" => &mut mods.logo,
                _ => return Err(format!("unknown modifier {name:?} in {s:?}")),
            };
            *flag = true;
        }
        if key.is_empty() {
            return Err(format!("missing key in {s:?}"));
        }
        let mut chars = key.chars();
        let keysym = match (chars.next(), chars.next()) {
            (Some(c), None) => Keysym::from_char(c),
            _ => xkb::keysym_from_name(key, xkb::KEYSYM_CASE_INSENSITIVE),
        };
        if keysym == Keysym::NoSymbol {
            return Err(format!("unknown key {key:?} in {s:?}"));
        }
        Ok(KeyCombo {
            mods,
            keysym: lower(keysym),
        })
    }
}

impl fmt::Display for KeyCombo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (on, name) in [
            (self.mods.ctrl, "ctrl"),
            (self.mods.alt, "alt"),
            (self.mods.shift, "shift"),
            (self.mods.logo, "super"),
        ] {
            if on {
                write!(f, "{name}+")?;
            }
        }
        write!(f, "{}", xkb::keysym_get_name(self.keysym))
    }
}

/// All bindings: the defaults, with actions named in the config replaced.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "BTreeMap<String, Vec<String>>")]
pub struct Keybinds {
    bindings: Vec<(KeyCombo, Action)>,
}

impl Default for Keybinds {
    fn default() -> Keybinds {
        Keybinds::with_overrides(&BTreeMap::new()).expect("valid default bindings")
    }
}

impl TryFrom<BTreeMap<String, Vec<String>>> for Keybinds {
    type Error = String;

    fn try_from(map: BTreeMap<String, Vec<String>>) -> Result<Keybinds, String> {
        Keybinds::with_overrides(&map)
    }
}

impl Keybinds {
    /// Builds the bindings, taking each action's combos from `overrides`
    /// when present (an empty list unbinds it) and the defaults otherwise.
    pub fn with_overrides(overrides: &BTreeMap<String, Vec<String>>) -> Result<Keybinds, String> {
        if let Some(name) = overrides.keys().find(|n| Action::from_name(n).is_none()) {
            let known: Vec<_> = Action::ALL.iter().map(|(n, _)| *n).collect();
            return Err(format!(
                "unknown action {name:?}, expected one of {}",
                known.join(", ")
            ));
        }
        let mut bindings = Vec::new();
        for &(name, action) in &Action::ALL {
            let combos: Vec<&str> = match overrides.get(name) {
                Some(list) => list.iter().map(String::as_str).collect(),
                None => action.defaults().to_vec(),
            };
            for combo in combos {
                bindings.push((combo.parse()?, action));
            }
        }
        Ok(Keybinds { bindings })
    }

    /// The action bound to a key press. `keysym` is the symbol produced with
    /// the current modifiers; `base` is the key's symbol without them, when
    /// the keymap is known.
    pub fn lookup(&self, keysym: Keysym, base: Option<Keysym>, mods: Modifiers) -> Option<Action> {
        let mods = Mods::from(mods);
        let shifted = lower(keysym);
        let base = lower(base.unwrap_or(keysym));
        // Shift was "used up" when it changed the symbol (`=` → `+`), but
        // not for letters, where it only changes case.
        let shift_consumed = mods.shift && shifted != base;
        let unshifted_mods = Mods {
            shift: false,
            ..mods
        };
        let find = |pred: &dyn Fn(&KeyCombo) -> bool| {
            self.bindings.iter().find(|(c, _)| pred(c)).map(|&(_, a)| a)
        };
        find(&|c| c.mods == mods && (c.keysym == base || c.keysym == shifted)).or_else(|| {
            find(&|c| shift_consumed && c.mods == unshifted_mods && c.keysym == shifted)
        })
    }

    /// The combos bound to `action`, for messages and tests.
    #[cfg(test)]
    fn combos(&self, action: Action) -> Vec<String> {
        self.bindings
            .iter()
            .filter(|(_, a)| *a == action)
            .map(|(c, _)| c.to_string())
            .collect()
    }
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
    const SHIFT: Modifiers = Modifiers {
        shift: true,
        ..NONE
    };
    const CTRL_SHIFT: Modifiers = Modifiers {
        ctrl: true,
        shift: true,
        ..NONE
    };

    fn binds(toml: &str) -> Result<Keybinds, String> {
        let map: BTreeMap<String, Vec<String>> = toml::from_str(toml).map_err(|e| e.to_string())?;
        Keybinds::with_overrides(&map)
    }

    #[test]
    fn parses_combos() {
        let combo: KeyCombo = "Ctrl+Shift+C".parse().unwrap();
        assert_eq!(
            combo.mods,
            Mods {
                ctrl: true,
                shift: true,
                ..Mods::default()
            }
        );
        assert_eq!(combo.keysym, Keysym::c);
        assert_eq!(
            "page_up".parse::<KeyCombo>().unwrap().keysym,
            Keysym::Page_Up
        );
        assert_eq!("alt+F5".parse::<KeyCombo>().unwrap().keysym, Keysym::F5);
        assert_eq!("ctrl++".parse::<KeyCombo>().unwrap().keysym, Keysym::plus);
        assert_eq!(
            "super+ä".parse::<KeyCombo>().unwrap().keysym,
            Keysym::adiaeresis
        );
        assert_eq!(
            "ctrl+shift+c".parse::<KeyCombo>().unwrap().to_string(),
            "ctrl+shift+c"
        );
    }

    #[test]
    fn rejects_bad_combos() {
        assert!("hyper+c".parse::<KeyCombo>().is_err());
        assert!("ctrl+notakey".parse::<KeyCombo>().is_err());
        assert!("ctrl+".parse::<KeyCombo>().is_err());
    }

    #[test]
    fn defaults() {
        let k = Keybinds::default();
        assert_eq!(
            k.lookup(Keysym::C, Some(Keysym::c), CTRL_SHIFT),
            Some(Action::Copy)
        );
        assert_eq!(
            k.lookup(Keysym::Insert, Some(Keysym::Insert), SHIFT),
            Some(Action::Paste)
        );
        assert_eq!(
            k.lookup(Keysym::Page_Up, None, SHIFT),
            Some(Action::ScrollPageUp)
        );
        assert_eq!(
            k.lookup(Keysym::_0, Some(Keysym::_0), CTRL),
            Some(Action::FontReset)
        );
        // Ctrl+C and plain keys go to the program.
        assert_eq!(k.lookup(Keysym::c, Some(Keysym::c), CTRL), None);
        assert_eq!(k.lookup(Keysym::Page_Up, None, NONE), None);
    }

    #[test]
    fn shifted_symbols_match_without_shift() {
        let k = Keybinds::default();
        // Ctrl+Shift+= produces "plus" on US layouts; ctrl+plus applies.
        assert_eq!(
            k.lookup(Keysym::plus, Some(Keysym::equal), CTRL_SHIFT),
            Some(Action::FontLarger)
        );
        assert_eq!(
            k.lookup(Keysym::equal, Some(Keysym::equal), CTRL),
            Some(Action::FontLarger)
        );
        // Shift changing a letter's case isn't "used up": ctrl+c doesn't
        // match Ctrl+Shift+C.
        let k = binds(r#"copy = ["ctrl+c"]"#).unwrap();
        assert_eq!(k.lookup(Keysym::C, Some(Keysym::c), CTRL_SHIFT), None);
        assert_eq!(
            k.lookup(Keysym::c, Some(Keysym::c), CTRL),
            Some(Action::Copy)
        );
    }

    #[test]
    fn base_symbol_matches_with_shift() {
        let k = binds(r#"font_larger = ["ctrl+shift+equal"]"#).unwrap();
        assert_eq!(
            k.lookup(Keysym::plus, Some(Keysym::equal), CTRL_SHIFT),
            Some(Action::FontLarger)
        );
        // Without the keymap, the shifted symbol is all there is.
        assert_eq!(k.lookup(Keysym::plus, None, CTRL_SHIFT), None);
    }

    #[test]
    fn overrides_replace_defaults() {
        let k = binds(
            r#"
            copy = ["ctrl+insert"]
            paste = []
            scroll_line_up = ["shift+up"]
            "#,
        )
        .unwrap();
        assert_eq!(k.combos(Action::Copy), ["ctrl+Insert"]);
        assert_eq!(k.lookup(Keysym::C, Some(Keysym::c), CTRL_SHIFT), None);
        assert_eq!(k.lookup(Keysym::V, Some(Keysym::v), CTRL_SHIFT), None);
        assert_eq!(
            k.lookup(Keysym::Up, None, SHIFT),
            Some(Action::ScrollLineUp)
        );
        // Untouched actions keep their defaults.
        assert_eq!(k.combos(Action::ScrollToTop), ["shift+Home"]);
    }

    #[test]
    fn copy_or_interrupt_is_opt_in() {
        let ctrl_c = |k: &Keybinds| k.lookup(Keysym::c, Some(Keysym::c), CTRL);
        assert_eq!(ctrl_c(&Keybinds::default()), None);
        let k = binds(r#"copy_or_interrupt = ["ctrl+c"]"#).unwrap();
        assert_eq!(ctrl_c(&k), Some(Action::CopyOrInterrupt));
        // The plain copy binding is unaffected.
        assert_eq!(
            k.lookup(Keysym::C, Some(Keysym::c), CTRL_SHIFT),
            Some(Action::Copy)
        );
    }

    #[test]
    fn rejects_unknown_actions_and_keys() {
        let err = binds(r#"cpoy = ["ctrl+c"]"#).unwrap_err();
        assert!(err.contains("unknown action \"cpoy\""), "{err}");
        assert!(binds(r#"copy = ["ctrl+nope"]"#).is_err());
    }
}
