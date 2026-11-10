//! What `jotty config` can edit: every config key with its type, limits
//! and a short description. Key bindings and file kinds are listed from
//! the effective config, one entry per action or kind.

use std::borrow::Cow;

use crate::config::Rgb;

/// The config's sections, in the order the TUI lists them.
pub const SECTIONS: [&str; 7] = [
    "font", "window", "terminal", "colors", "keybinds", "url", "files",
];

/// The type of a config key's value, which says how the TUI edits it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Bool,
    Float {
        min: f64,
        max: f64,
    },
    Int {
        min: i64,
        max: i64,
    },
    Str,
    /// A string the config may leave unset.
    OptStr,
    Enum(&'static [&'static str]),
    Color,
    /// A color the config may leave unset.
    OptColor,
    StrList,
}

/// One editable key.
#[derive(Debug, Clone, PartialEq)]
pub struct Key {
    /// The path of tables to the value, starting with the section.
    pub path: Vec<String>,
    pub kind: Kind,
    pub doc: Cow<'static, str>,
    /// For the entries of `colors.palette`: which of the 16 colors.
    pub index: Option<usize>,
}

/// The names of the 16 ANSI colors, as `colors.palette` lists them.
pub const PALETTE_NAMES: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "bright black",
    "bright red",
    "bright green",
    "bright yellow",
    "bright blue",
    "bright magenta",
    "bright cyan",
    "bright white",
];

impl Key {
    /// The table the key is in, such as `font`.
    pub fn section(&self) -> &str {
        &self.path[0]
    }

    /// The path without the section, as the TUI lists it.
    pub fn name(&self) -> String {
        let name = self.path[1..].join(".");
        match self.index {
            Some(i) => format!("{name}[{i}] {}", PALETTE_NAMES[i]),
            None => name,
        }
    }

    /// The name as the list shows it: padded so that color names line up
    /// past `palette[9]`.
    pub fn list_name(&self) -> String {
        match self.index {
            Some(i) => format!(
                "{:<12}{}",
                format!("{}[{i}]", self.path[1]),
                PALETTE_NAMES[i]
            ),
            None => self.name(),
        }
    }

    /// The key's path, such as `font.size`.
    pub fn dotted(&self) -> String {
        format!("{}.{}", self.section(), self.name())
    }
}

/// The keys of the fixed sections. Limits are those `Config::sanitize`
/// clamps to.
const STATIC: &[(&str, &str, Kind, &str)] = {
    use Kind::*;
    &[
        (
            "font",
            "family",
            Str,
            "A fontconfig family name or a generic name such as monospace.",
        ),
        (
            "font",
            "size",
            Float {
                min: 4.0,
                max: 200.0,
            },
            "Size in points at 96 DPI, multiplied by the output scale.",
        ),
        (
            "font",
            "size_step",
            Float {
                min: 0.1,
                max: 10.0,
            },
            "Points that font_larger and font_smaller add or remove.",
        ),
        (
            "font",
            "fallback",
            StrList,
            "Families tried, in order, for characters the main font lacks.",
        ),
        (
            "window",
            "title",
            Str,
            "Initial title; programs can change it. Applies to new windows.",
        ),
        (
            "window",
            "app_id",
            Str,
            "Wayland application ID, for compositor window rules. Applies to new windows.",
        ),
        (
            "window",
            "padding",
            Int { min: 0, max: 1000 },
            "Space around the grid, in logical pixels.",
        ),
        (
            "window",
            "opacity",
            Float { min: 0.0, max: 1.0 },
            "Background opacity. Text, the cursor and the selection stay opaque.",
        ),
        (
            "window",
            "transparent_cell_backgrounds",
            Bool,
            "Also apply opacity to cells with an explicit background color.",
        ),
        (
            "window",
            "blur",
            Bool,
            "Blur what is behind the window (ext-background-effect-v1).",
        ),
        (
            "window",
            "renderer",
            Enum(&["cpu", "gpu"]),
            "cpu draws in software; gpu draws with OpenGL ES. Applies to new windows.",
        ),
        (
            "window",
            "bell_urgent",
            Bool,
            "Mark the window urgent when the bell rings while it isn't focused.",
        ),
        (
            "window",
            "notifications",
            Bool,
            "Show the notifications programs ask for (OSC 9, OSC 777) while unfocused.",
        ),
        (
            "window",
            "columns",
            Int { min: 2, max: 2000 },
            "Initial width in cells. Applies to new windows.",
        ),
        (
            "window",
            "lines",
            Int { min: 1, max: 2000 },
            "Initial height in cells. Applies to new windows.",
        ),
        (
            "terminal",
            "shell",
            OptStr,
            "Program to run; empty means $SHELL, then /bin/sh. Applies to new windows.",
        ),
        (
            "terminal",
            "term",
            Str,
            "$TERM for the program; auto uses jotty's terminfo entry when installed.",
        ),
        (
            "terminal",
            "scrollback",
            Int {
                min: 0,
                max: 10_000_000,
            },
            "Lines kept in the scrollback history. Applies to new windows.",
        ),
        (
            "terminal",
            "bold_is_bright",
            Bool,
            "Draw bold text in the bright variant of the first eight colors.",
        ),
        ("colors", "foreground", Color, "Default text color."),
        ("colors", "background", Color, "Default background color."),
        (
            "colors",
            "cursor",
            OptColor,
            "Cursor color; empty means the foreground color.",
        ),
        (
            "colors",
            "cursor_text",
            OptColor,
            "Character under a block cursor; empty means the cell's background.",
        ),
        (
            "colors",
            "selection_foreground",
            OptColor,
            "Text of selected cells; empty means the cell's background.",
        ),
        (
            "colors",
            "selection_background",
            OptColor,
            "Background of selected cells; empty means the cell's foreground.",
        ),
        (
            "url",
            "launcher",
            StrList,
            "Command that opens a link; the link is its last argument.",
        ),
        (
            "url",
            "regex",
            StrList,
            "Regular expressions for more kinds of links, besides URLs.",
        ),
    ]
};

/// Every key, in section order. `effective` is the serialized effective
/// config, which names the actions and the file kinds.
pub fn keys(effective: &toml::Table) -> Vec<Key> {
    let mut keys: Vec<Key> = STATIC
        .iter()
        .map(|&(section, name, kind, doc)| Key {
            path: vec![section.into(), name.into()],
            kind,
            doc: doc.into(),
            index: None,
        })
        .collect();
    for (i, name) in PALETTE_NAMES.iter().enumerate() {
        keys.push(Key {
            path: vec!["colors".into(), "palette".into()],
            kind: Kind::Color,
            doc: format!("ANSI color {i}, {name}. Changing any of them saves all 16.").into(),
            index: Some(i),
        });
    }
    let names = |section: &str| -> Vec<String> {
        effective
            .get(section)
            .and_then(toml::Value::as_table)
            .map(|t| t.keys().cloned().collect())
            .unwrap_or_default()
    };
    for action in names("keybinds") {
        let doc =
            format!("Combos that run {action}, such as ctrl+shift+c; an empty list unbinds it.");
        keys.push(Key {
            path: vec!["keybinds".into(), action],
            kind: Kind::StrList,
            doc: doc.into(),
            index: None,
        });
    }
    for kind in names("files") {
        keys.push(Key {
            path: vec!["files".into(), kind.clone(), "extensions".into()],
            kind: Kind::StrList,
            doc: format!("File name extensions of {kind} files, without the dot.").into(),
            index: None,
        });
        keys.push(Key {
            path: vec!["files".into(), kind.clone(), "launcher".into()],
            kind: Kind::StrList,
            doc: format!("Command that opens {kind} files; empty means url.launcher.").into(),
            index: None,
        });
    }
    let order = |key: &Key| SECTIONS.iter().position(|s| *s == key.section());
    keys.sort_by_key(order);
    keys
}

impl Kind {
    /// What values the kind takes, for the help line.
    pub fn describe(self) -> String {
        use Kind::*;
        match self {
            Bool => "true or false (Enter toggles)".into(),
            Float { min, max } => format!("number, {min}–{max}"),
            Int { min, max } => format!("whole number, {min}–{max}"),
            Str => "text".into(),
            OptStr => "text, or empty for the default".into(),
            Enum(values) => format!("one of {} (Enter cycles)", values.join(", ")),
            Color => "#rrggbb".into(),
            OptColor => "#rrggbb, or empty for the default".into(),
            StrList => "comma-separated list, or a TOML array".into(),
        }
    }

    /// Whether Enter changes the value directly instead of editing text.
    pub fn toggles(self) -> bool {
        use Kind::*;
        matches!(self, Bool | Enum(_))
    }

    /// The value after `current` for kinds that toggle.
    pub fn next(self, current: Option<&toml::Value>) -> Option<toml::Value> {
        use Kind::*;
        match self {
            Bool => Some(toml::Value::Boolean(
                !current.and_then(toml::Value::as_bool).unwrap_or(false),
            )),
            Enum(values) => {
                let at = current
                    .and_then(toml::Value::as_str)
                    .and_then(|v| values.iter().position(|x| *x == v));
                let next = at.map_or(0, |i| (i + 1) % values.len());
                Some(toml::Value::String(values[next].into()))
            }
            _ => None,
        }
    }

    /// Parses what the user typed. `Ok(None)` means "no value": the key
    /// goes back to the config file's value or the default.
    pub fn parse(self, raw: &str) -> Result<Option<toml::Value>, String> {
        use Kind::*;
        // Text is taken as typed; everything else may have stray spaces.
        let input = raw.trim();
        let value = match self {
            Bool => match input {
                "true" => toml::Value::Boolean(true),
                "false" => toml::Value::Boolean(false),
                _ => return Err("expected true or false".into()),
            },
            Float { min, max } => {
                let v: f64 = input
                    .parse()
                    .map_err(|_| format!("{input:?} is not a number"))?;
                if !(min..=max).contains(&v) {
                    return Err(format!("{v} is outside {min}–{max}"));
                }
                toml::Value::Float(v)
            }
            Int { min, max } => {
                let v: i64 = input
                    .parse()
                    .map_err(|_| format!("{input:?} is not a whole number"))?;
                if !(min..=max).contains(&v) {
                    return Err(format!("{v} is outside {min}–{max}"));
                }
                toml::Value::Integer(v)
            }
            Str => toml::Value::String(raw.into()),
            OptStr | OptColor if input.is_empty() => return Ok(None),
            OptStr => toml::Value::String(raw.into()),
            Enum(values) => {
                if !values.contains(&input) {
                    return Err(format!("expected one of {}", values.join(", ")));
                }
                toml::Value::String(input.into())
            }
            Color | OptColor => toml::Value::String(parse_color(input)?),
            StrList => toml::Value::Array(parse_list(input)?),
        };
        Ok(Some(value))
    }
}

fn parse_color(input: &str) -> Result<String, String> {
    input.parse::<Rgb>().map(|c| c.to_string())
}

/// A list typed as `a, b, c`, or as a TOML array (`["a,b", "c"]`) when an
/// item has a comma.
fn parse_list(input: &str) -> Result<Vec<toml::Value>, String> {
    if input.starts_with('[') {
        let table: toml::Table =
            toml::from_str(&format!("v = {input}")).map_err(|e| e.message().to_string())?;
        return match table.get("v") {
            Some(toml::Value::Array(items)) if items.iter().all(toml::Value::is_str) => {
                Ok(items.clone())
            }
            _ => Err("expected an array of strings".into()),
        };
    }
    Ok(input
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| toml::Value::String(s.into()))
        .collect())
}

/// A value as the TUI shows it and as editing starts from: lists comma
/// separated when that reads back the same, TOML otherwise.
pub fn format(value: &toml::Value) -> String {
    match value {
        toml::Value::String(s) => s.clone(),
        toml::Value::Float(f) => format!("{f:?}"),
        toml::Value::Array(items) => {
            let plain: Option<Vec<&str>> = items
                .iter()
                .map(|v| {
                    v.as_str().filter(|s| {
                        !s.is_empty() && s.trim() == *s && !s.contains(',') && !s.starts_with('[')
                    })
                })
                .collect();
            match plain {
                Some(items) => items.join(", "),
                None => value.to_string(),
            }
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    /// The paths of every value that isn't a table.
    fn leaves(table: &toml::Table, prefix: &[String], out: &mut Vec<Vec<String>>) {
        for (key, value) in table {
            let mut path = prefix.to_vec();
            path.push(key.clone());
            match value {
                toml::Value::Table(t) => leaves(t, &path, out),
                _ => out.push(path),
            }
        }
    }

    #[test]
    fn every_config_key_has_an_entry_and_back() {
        use Kind::*;
        let defaults = toml::Table::try_from(Config::default()).unwrap();
        let keys = keys(&defaults);
        let mut paths = Vec::new();
        leaves(&defaults, &[], &mut paths);
        for path in &paths {
            assert!(
                keys.iter().any(|k| &k.path == path),
                "{} has no schema entry",
                path.join(".")
            );
        }
        for key in &keys {
            let optional = matches!(key.kind, OptStr | OptColor);
            assert!(
                optional || paths.contains(&key.path),
                "{} is not a config key",
                key.dotted()
            );
            assert!(SECTIONS.contains(&key.section()));
        }
    }

    /// Each kind's parsed values are accepted by the config itself.
    #[test]
    fn parsed_values_are_valid_config() {
        use Kind::*;
        let defaults = toml::Table::try_from(Config::default()).unwrap();
        // Palette entries are set through the TUI, which writes the list.
        let scalar_keys = keys(&defaults).into_iter().filter(|k| k.index.is_none());
        for key in scalar_keys {
            let input: String = match key.kind {
                Bool => "true".into(),
                Float { min, .. } => min.to_string(),
                Int { min, .. } => min.to_string(),
                Str | OptStr => "x".into(),
                Enum(values) => values[0].into(),
                Color | OptColor => "#123456".into(),
                StrList if key.section() == "keybinds" => "ctrl+shift+x, alt+F5".into(),
                StrList => "a, b".into(),
            };
            let value = key.kind.parse(&input).unwrap().unwrap();
            let mut table = toml::Table::new();
            super::super::set_path(&mut table, &key.path, value);
            let parsed = Config::from_table(table);
            assert!(parsed.is_ok(), "{}: {parsed:?}", key.dotted());
        }
    }

    #[test]
    fn parses_input() {
        use Kind::*;
        let float = Float {
            min: 4.0,
            max: 200.0,
        };
        assert_eq!(float.parse("12").unwrap(), Some(toml::Value::Float(12.0)));
        assert!(float.parse("2").is_err());
        assert!(float.parse("nan").is_err());
        assert!(Int { min: 0, max: 5 }.parse("6").is_err());
        assert_eq!(OptColor.parse(" ").unwrap(), None);
        assert_eq!(
            Color.parse("ABCDEF").unwrap(),
            Some(toml::Value::String("#abcdef".into()))
        );
        assert!(Color.parse("#abc").is_err());
        let list = |items: &[&str]| {
            Some(toml::Value::Array(
                items
                    .iter()
                    .map(|s| toml::Value::String((*s).into()))
                    .collect(),
            ))
        };
        assert_eq!(StrList.parse("a, b,,c ").unwrap(), list(&["a", "b", "c"]));
        assert_eq!(StrList.parse("").unwrap(), list(&[]));
        assert_eq!(
            StrList.parse(r#"["a,b", "c"]"#).unwrap(),
            list(&["a,b", "c"])
        );
        assert!(StrList.parse("[1, 2]").is_err());
    }

    #[test]
    fn formatted_lists_read_back() {
        use Kind::*;
        for list in [
            r#"["a", "b"]"#,
            r#"["a,b", "c"]"#,
            "[]",
            r#"["a", ""]"#,
            r#"[" a"]"#,
        ] {
            let value = StrList.parse(list).unwrap().unwrap();
            assert_eq!(StrList.parse(&format(&value)).unwrap(), Some(value));
        }
    }

    #[test]
    fn toggles() {
        use Kind::*;
        let renderer = Enum(&["cpu", "gpu"]);
        let gpu = toml::Value::String("gpu".into());
        assert_eq!(
            renderer.next(Some(&gpu)),
            Some(toml::Value::String("cpu".into()))
        );
        assert_eq!(
            Bool.next(Some(&toml::Value::Boolean(true))),
            Some(toml::Value::Boolean(false))
        );
        assert_eq!(Str.next(None), None);
    }
}
