//! User configuration, loaded from `$XDG_CONFIG_HOME/jotty/config.toml`.
//!
//! Every field has a default, so an empty or missing file is valid.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::keybinds::Keybinds;

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub font: FontConfig,
    pub window: WindowConfig,
    pub terminal: TerminalConfig,
    pub colors: ColorConfig,
    /// Key bindings for jotty's actions; see `keybinds.rs`.
    pub keybinds: Keybinds,
    pub url: UrlConfig,
    /// Kinds of files that Ctrl+click opens, such as images and videos.
    pub files: Files,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UrlConfig {
    /// Command that opens a link; the link is appended as its last argument.
    pub launcher: Vec<String>,
    /// Regular expressions for more kinds of links, besides URLs.
    pub regex: Vec<String>,
}

impl Default for UrlConfig {
    fn default() -> Self {
        Self {
            launcher: vec!["xdg-open".into()],
            regex: Vec::new(),
        }
    }
}

/// Kinds of files whose paths are links, by name: `image` and `video`
/// built in, and any the user adds. A kind in the config only overrides
/// the fields it sets.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(from = "BTreeMap<String, FileKindOverride>")]
pub struct Files(pub BTreeMap<String, FileKind>);

#[derive(Debug, Clone, PartialEq)]
pub struct FileKind {
    /// File name extensions, without the dot, matched case-insensitively.
    pub extensions: Vec<String>,
    /// Command that opens these files; empty means `url.launcher`.
    pub launcher: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileKindOverride {
    extensions: Option<Vec<String>>,
    launcher: Option<Vec<String>>,
}

impl Default for Files {
    fn default() -> Self {
        let kind = |extensions: &[&str]| FileKind {
            extensions: extensions.iter().map(|e| e.to_string()).collect(),
            launcher: Vec::new(),
        };
        Files(BTreeMap::from([
            (
                "image".to_string(),
                kind(&[
                    "png", "jpg", "jpeg", "gif", "webp", "avif", "jxl", "heic", "heif", "bmp",
                    "tif", "tiff", "svg", "ico",
                ]),
            ),
            (
                "video".to_string(),
                kind(&[
                    "mp4", "mkv", "webm", "mov", "avi", "m4v", "wmv", "flv", "mpg", "mpeg", "ogv",
                    "3gp",
                ]),
            ),
        ]))
    }
}

impl From<BTreeMap<String, FileKindOverride>> for Files {
    fn from(overrides: BTreeMap<String, FileKindOverride>) -> Files {
        let mut files = Files::default();
        for (name, o) in overrides {
            let kind = files.0.entry(name).or_insert(FileKind {
                extensions: Vec::new(),
                launcher: Vec::new(),
            });
            if let Some(extensions) = o.extensions {
                kind.extensions = extensions;
            }
            if let Some(launcher) = o.launcher {
                kind.launcher = launcher;
            }
        }
        files
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FontConfig {
    /// Font family name, resolved through the system font configuration.
    pub family: String,
    /// Font size in points (at 96 DPI and a scale factor of 1).
    pub size: f32,
    /// Points added or removed by `font_larger` and `font_smaller`, per
    /// key press or wheel notch.
    pub size_step: f32,
    /// Families tried, in order, for characters the primary font lacks.
    pub fallback: Vec<String>,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: "monospace".into(),
            size: 11.0,
            size_step: 1.0,
            fallback: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowConfig {
    pub title: String,
    pub app_id: String,
    /// Padding around the cell grid, in logical pixels.
    pub padding: u32,
    /// Background opacity, from 0.0 (transparent) to 1.0 (opaque).
    pub opacity: f32,
    /// Also apply `opacity` to cells with an explicit background color.
    pub transparent_cell_backgrounds: bool,
    /// Ask the compositor to blur what is behind the window
    /// (ext-background-effect-v1).
    pub blur: bool,
    /// Draw with the GPU (OpenGL ES) or on the CPU.
    pub renderer: RendererKind,
    /// Mark the window urgent (xdg-activation) when the bell rings while
    /// it isn't focused.
    pub bell_urgent: bool,
    /// Initial size in cells.
    pub columns: u16,
    pub lines: u16,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            title: "jotty".into(),
            app_id: "jotty".into(),
            padding: 4,
            opacity: 1.0,
            transparent_cell_backgrounds: false,
            blur: false,
            renderer: RendererKind::Cpu,
            bell_urgent: true,
            columns: 80,
            lines: 24,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RendererKind {
    /// OpenGL ES 3 through EGL. Cheap per frame at any size, but the GPU
    /// driver it loads takes 100 MiB or more of memory.
    Gpu,
    /// Shared-memory buffers drawn on the CPU, repainting only what changed.
    Cpu,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TerminalConfig {
    /// Program to run; defaults to `$SHELL`, then `/bin/sh`.
    pub shell: Option<String>,
    /// Value of `$TERM` for the child process. `"auto"` uses jotty's own
    /// terminfo entry when it is installed, `xterm-256color` otherwise.
    pub term: String,
    /// Number of lines kept in the scrollback history.
    pub scrollback: usize,
    /// Render bold text with the bright variant of the first eight colors.
    pub bold_is_bright: bool,
    /// Double-clicking a command's output selects all of it (needs the
    /// shell to mark commands with OSC 133).
    pub double_click_selects_output: bool,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            shell: None,
            term: "auto".into(),
            scrollback: 10_000,
            bold_is_bright: true,
            double_click_selects_output: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ColorConfig {
    pub foreground: Rgb,
    pub background: Rgb,
    /// Cursor color; defaults to the foreground color.
    pub cursor: Option<Rgb>,
    /// Color of the character under a block cursor; defaults to the cell's
    /// background color.
    pub cursor_text: Option<Rgb>,
    /// Text color of selected cells; defaults to the cell's background
    /// (inverse video).
    pub selection_foreground: Option<Rgb>,
    /// Background of selected cells; defaults to the cell's foreground.
    pub selection_background: Option<Rgb>,
    /// Overrides for the 16 ANSI colors, in order (black, red, ..., bright white).
    /// May be shorter than 16 entries; the rest keep their defaults.
    pub palette: Vec<Rgb>,
}

impl Default for ColorConfig {
    fn default() -> Self {
        Self {
            foreground: Rgb(0xdc, 0xdc, 0xdc),
            background: Rgb(0x18, 0x18, 0x18),
            cursor: None,
            cursor_text: None,
            selection_foreground: None,
            selection_background: None,
            palette: Vec::new(),
        }
    }
}

pub const DEFAULT_PALETTE: [Rgb; 16] = [
    Rgb(0x24, 0x24, 0x24),
    Rgb(0xe0, 0x6c, 0x75),
    Rgb(0x98, 0xc3, 0x79),
    Rgb(0xe5, 0xc0, 0x7b),
    Rgb(0x61, 0xaf, 0xef),
    Rgb(0xc6, 0x78, 0xdd),
    Rgb(0x56, 0xb6, 0xc2),
    Rgb(0xc8, 0xc8, 0xc8),
    Rgb(0x5c, 0x63, 0x70),
    Rgb(0xff, 0x7a, 0x85),
    Rgb(0xb5, 0xe8, 0x90),
    Rgb(0xff, 0xd6, 0x8a),
    Rgb(0x7e, 0xc7, 0xff),
    Rgb(0xde, 0x93, 0xf2),
    Rgb(0x6e, 0xd3, 0xdf),
    Rgb(0xff, 0xff, 0xff),
];

impl ColorConfig {
    /// The 16 ANSI colors with user overrides applied.
    pub fn palette(&self) -> [Rgb; 16] {
        let mut palette = DEFAULT_PALETTE;
        for (slot, color) in palette.iter_mut().zip(&self.palette) {
            *slot = *color;
        }
        palette
    }

    pub fn cursor(&self) -> Rgb {
        self.cursor.unwrap_or(self.foreground)
    }
}

/// An sRGB color, written as `"#rrggbb"` in the config file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub fn to_f32(self) -> [f32; 3] {
        [self.0, self.1, self.2].map(|c| f32::from(c) / 255.0)
    }
}

impl TryFrom<String> for Rgb {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl std::str::FromStr for Rgb {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex = s.strip_prefix('#').unwrap_or(s);
        if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("invalid color {s:?}, expected \"#rrggbb\""));
        }
        let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap();
        Ok(Rgb(channel(0), channel(2), channel(4)))
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }
}

#[derive(Debug)]
pub enum ConfigError {
    Io(PathBuf, std::io::Error),
    Parse(PathBuf, toml::de::Error),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Io(path, e) => write!(f, "{}: {e}", path.display()),
            ConfigError::Parse(path, e) => write!(f, "{}: {e}", path.display()),
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    pub fn parse(text: &str) -> Result<Self, toml::de::Error> {
        let mut config: Config = toml::from_str(text)?;
        config.sanitize();
        Ok(config)
    }

    /// Loads the config from `path`, or from the default location when `None`.
    /// A missing file at the default location yields the default config.
    pub fn load(path: Option<&Path>) -> Result<Self, ConfigError> {
        let (path, required) = match path {
            Some(p) => (p.to_path_buf(), true),
            None => match default_path() {
                Some(p) => (p, false),
                None => return Ok(Self::default()),
            },
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text).map_err(|e| ConfigError::Parse(path, e)),
            Err(e) if !required && e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(ConfigError::Io(path, e)),
        }
    }

    /// Clamps values that would otherwise break rendering.
    fn sanitize(&mut self) {
        self.font.size = self.font.size.clamp(4.0, 200.0);
        self.font.size_step = self.font.size_step.clamp(0.1, 10.0);
        self.window.opacity = self.window.opacity.clamp(0.0, 1.0);
        self.window.columns = self.window.columns.max(2);
        self.window.lines = self.window.lines.max(1);
    }
}

pub fn default_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("jotty").join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_is_default() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
    }

    #[test]
    fn parses_all_sections() {
        let config = Config::parse(
            r##"
            [font]
            family = "JetBrains Mono"
            size = 11.5
            fallback = ["Noto Color Emoji"]

            [window]
            padding = 8
            opacity = 0.9
            transparent_cell_backgrounds = true
            blur = true

            [terminal]
            shell = "/bin/zsh"
            scrollback = 500

            [colors]
            foreground = "#ffffff"
            background = "#000000"
            cursor = "#ff0000"
            cursor_text = "#00ff00"
            selection_foreground = "#000001"
            selection_background = "#fffffe"
            palette = ["#010203", "#aabbcc"]
            "##,
        )
        .unwrap();

        assert_eq!(config.font.family, "JetBrains Mono");
        assert_eq!(config.font.size, 11.5);
        assert_eq!(config.font.fallback, ["Noto Color Emoji"]);
        assert_eq!(config.window.padding, 8);
        assert_eq!(config.window.opacity, 0.9);
        assert!(config.window.transparent_cell_backgrounds);
        assert!(config.window.blur);
        assert_eq!(config.terminal.shell.as_deref(), Some("/bin/zsh"));
        assert_eq!(config.terminal.scrollback, 500);
        assert_eq!(config.colors.cursor(), Rgb(255, 0, 0));
        assert_eq!(config.colors.cursor_text, Some(Rgb(0, 255, 0)));
        assert_eq!(config.colors.selection_foreground, Some(Rgb(0, 0, 1)));
        assert_eq!(config.colors.selection_background, Some(Rgb(255, 255, 254)));
        let palette = config.colors.palette();
        assert_eq!(palette[0], Rgb(1, 2, 3));
        assert_eq!(palette[1], Rgb(0xaa, 0xbb, 0xcc));
        assert_eq!(palette[2], DEFAULT_PALETTE[2]);
    }

    #[test]
    fn keybinds_section() {
        let config = Config::parse("[keybinds]\ncopy = [\"ctrl+insert\"]").unwrap();
        assert_ne!(config.keybinds, Keybinds::default());
        let err = Config::parse("[keybinds]\ncopy = [\"ctrl+bogus\"]").unwrap_err();
        assert!(err.to_string().contains("unknown key"), "{err}");
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(Config::parse("[font]\nfamliy = \"x\"").is_err());
    }

    #[test]
    fn rejects_bad_colors() {
        assert!(Config::parse("[colors]\nforeground = \"#12345\"").is_err());
        assert!(Config::parse("[colors]\nforeground = \"#gggggg\"").is_err());
    }

    #[test]
    fn color_roundtrip() {
        let c: Rgb = "#1a2B3c".parse().unwrap();
        assert_eq!(c, Rgb(0x1a, 0x2b, 0x3c));
        assert_eq!(c.to_string(), "#1a2b3c");
        assert_eq!("1a2b3c".parse::<Rgb>().unwrap(), c);
    }

    #[test]
    fn clamps_values() {
        let config =
            Config::parse("[font]\nsize = 0\nsize_step = -1\n[window]\nopacity = 3.0").unwrap();
        assert_eq!(config.font.size, 4.0);
        assert_eq!(config.font.size_step, 0.1);
        assert_eq!(config.window.opacity, 1.0);
    }

    #[test]
    fn cursor_defaults_to_foreground() {
        let config = Config::default();
        assert_eq!(config.colors.cursor(), config.colors.foreground);
    }

    /// The documented example must stay valid and match the defaults.
    #[test]
    fn example_config_matches_defaults() {
        let example = Config::parse(include_str!("../docs/config.example.toml")).unwrap();
        let mut expected = Config::default();
        expected.colors.palette = DEFAULT_PALETTE.to_vec();
        assert_eq!(example, expected);
    }

    #[test]
    fn file_kinds_override_only_what_they_set() {
        let config = Config::parse(
            "[files.video]\nlauncher = [\"mpv\"]\n[files.pdf]\nextensions = [\"pdf\"]\n[files.image]\nextensions = []",
        )
        .unwrap();
        let files = &config.files.0;
        let defaults = Files::default().0;
        assert_eq!(files["video"].launcher, ["mpv"]);
        assert_eq!(files["video"].extensions, defaults["video"].extensions);
        assert_eq!(files["pdf"].extensions, ["pdf"]);
        assert!(files["pdf"].launcher.is_empty());
        assert!(files["image"].extensions.is_empty());
        assert!(Config::parse("[files.video]\nprogram = []").is_err());
    }

    #[test]
    fn missing_explicit_file_is_an_error() {
        let err = Config::load(Some(Path::new("/nonexistent/jotty.toml"))).unwrap_err();
        assert!(matches!(err, ConfigError::Io(..)));
    }
}
