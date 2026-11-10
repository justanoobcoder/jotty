//! User configuration, loaded from `$XDG_CONFIG_HOME/jotty/config.toml`,
//! with the overrides `jotty config` writes to
//! `$XDG_STATE_HOME/jotty/overrides.toml` merged over it.
//!
//! Every field has a default, so an empty or missing file is valid.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::{fmt, io};

use serde::{Deserialize, Serialize, Serializer};

use crate::color::Rgb;
use crate::keybinds::Keybinds;

/// The config file. Every key is optional; missing keys keep their defaults.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
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

/// The `[url]` section: opening links.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
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
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(from = "BTreeMap<String, FileKindOverride>")]
pub struct Files(pub BTreeMap<String, FileKind>);

/// A kind of file that Ctrl+click opens, such as images.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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

/// The `[font]` section.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct FontConfig {
    /// Font family name, resolved through the system font configuration.
    pub family: String,
    /// Font size in points (at 96 DPI and a scale factor of 1).
    #[serde(serialize_with = "short_f32")]
    pub size: f32,
    /// Points added or removed by `font_larger` and `font_smaller`, per
    /// key press or wheel notch.
    #[serde(serialize_with = "short_f32")]
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

/// The `[window]` section.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowConfig {
    pub title: String,
    pub app_id: String,
    /// Padding around the cell grid, in logical pixels.
    pub padding: u32,
    /// Background opacity, from 0.0 (transparent) to 1.0 (opaque).
    #[serde(serialize_with = "short_f32")]
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
    /// Show the desktop notifications programs ask for (OSC 9, OSC 777)
    /// while the window isn't focused.
    pub notifications: bool,
    /// Initial size in cells.
    pub columns: u16,
    pub lines: u16,
}

/// The most columns and lines a window asks for, the padding around them,
/// and the longest history: past these, the sizes that follow from them
/// overflow or take more memory than any machine has.
const MAX_GRID: u16 = 2000;
const MAX_PADDING: u32 = 1000;
const MAX_SCROLLBACK: usize = 10_000_000;

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
            notifications: true,
            columns: 80,
            lines: 24,
        }
    }
}

/// Which backend draws the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RendererKind {
    /// OpenGL ES 3 through EGL. Cheap per frame at any size, but the GPU
    /// driver it loads takes 100 MiB or more of memory.
    Gpu,
    /// Shared-memory buffers drawn on the CPU, repainting only what changed.
    Cpu,
}

/// The `[terminal]` section.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct TerminalConfig {
    /// Program to run; defaults to `$SHELL`, then `/bin/sh`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    /// Value of `$TERM` for the child process. `"auto"` uses jotty's own
    /// terminfo entry when it is installed, `xterm-256color` otherwise.
    pub term: String,
    /// Number of lines kept in the scrollback history.
    pub scrollback: usize,
    /// Render bold text with the bright variant of the first eight colors.
    pub bold_is_bright: bool,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            shell: None,
            term: "auto".into(),
            scrollback: 10_000,
            bold_is_bright: true,
        }
    }
}

/// The `[colors]` section.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ColorConfig {
    pub foreground: Rgb,
    pub background: Rgb,
    /// Cursor color; defaults to the foreground color.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Rgb>,
    /// Color of the character under a block cursor; defaults to the cell's
    /// background color.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor_text: Option<Rgb>,
    /// Text color of selected cells; defaults to the cell's background
    /// (inverse video).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection_foreground: Option<Rgb>,
    /// Background of selected cells; defaults to the cell's foreground.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection_background: Option<Rgb>,
    /// Overrides for the 16 ANSI colors, in order (black, red, ..., bright white).
    /// May be shorter than 16 entries; the rest keep their defaults.
    pub palette: Vec<Rgb>,
}

impl Default for ColorConfig {
    fn default() -> Self {
        Self {
            foreground: Rgb::new(0xdc, 0xdc, 0xdc),
            background: Rgb::new(0x18, 0x18, 0x18),
            cursor: None,
            cursor_text: None,
            selection_foreground: None,
            selection_background: None,
            palette: Vec::new(),
        }
    }
}

/// The 16 ANSI colors used where `colors.palette` sets none.
pub const DEFAULT_PALETTE: [Rgb; 16] = [
    Rgb::new(0x24, 0x24, 0x24),
    Rgb::new(0xe0, 0x6c, 0x75),
    Rgb::new(0x98, 0xc3, 0x79),
    Rgb::new(0xe5, 0xc0, 0x7b),
    Rgb::new(0x61, 0xaf, 0xef),
    Rgb::new(0xc6, 0x78, 0xdd),
    Rgb::new(0x56, 0xb6, 0xc2),
    Rgb::new(0xc8, 0xc8, 0xc8),
    Rgb::new(0x5c, 0x63, 0x70),
    Rgb::new(0xff, 0x7a, 0x85),
    Rgb::new(0xb5, 0xe8, 0x90),
    Rgb::new(0xff, 0xd6, 0x8a),
    Rgb::new(0x7e, 0xc7, 0xff),
    Rgb::new(0xde, 0x93, 0xf2),
    Rgb::new(0x6e, 0xd3, 0xdf),
    Rgb::new(0xff, 0xff, 0xff),
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

    /// The cursor color: `cursor` when set, else the foreground.
    pub fn cursor(&self) -> Rgb {
        self.cursor.unwrap_or(self.foreground)
    }
}

/// Why a config file could not be read, with its path.
#[derive(Debug)]
pub enum ConfigError {
    Io(PathBuf, io::Error),
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
    /// Parses a config file's text and clamps its values to their limits.
    pub fn parse(text: &str) -> Result<Self, toml::de::Error> {
        let mut config: Config = toml::from_str(text)?;
        config.sanitize();
        Ok(config)
    }

    /// Builds the config from a table of TOML values, as [`Config::parse`]
    /// does from text. Errors carry no position.
    pub fn from_table(table: toml::Table) -> Result<Self, toml::de::Error> {
        let mut config: Config = toml::Value::Table(table).try_into()?;
        config.sanitize();
        Ok(config)
    }

    /// Loads the config from `path`, or from the default location when `None`,
    /// with the overrides merged over it. A missing file at the default
    /// location yields the default config.
    pub fn load(path: Option<&Path>) -> Result<Self, ConfigError> {
        let base = match path {
            Some(p) => Some((p.to_path_buf(), true)),
            None => default_path().map(|p| (p, false)),
        };
        let base = base.as_ref().map(|(p, required)| (p.as_path(), *required));
        Self::load_layers(base, overrides_path().as_deref())
    }

    /// Loads the base file (`required` makes a missing one an error), then
    /// merges `overlay` over it. A broken overlay is logged and ignored:
    /// it must never cost the user the config they wrote.
    fn load_layers(
        base: Option<(&Path, bool)>,
        overlay: Option<&Path>,
    ) -> Result<Self, ConfigError> {
        let text = match base {
            Some((path, required)) => match read_optional(path) {
                Ok(Some(text)) => text,
                Ok(None) if !required => String::new(),
                Ok(None) => {
                    let e = io::Error::from(io::ErrorKind::NotFound);
                    return Err(ConfigError::Io(path.to_path_buf(), e));
                }
                Err(e) => return Err(ConfigError::Io(path.to_path_buf(), e)),
            },
            None => String::new(),
        };
        let base_path = base.map_or_else(PathBuf::new, |b| b.0.into());
        // Parsed from text, not from the table, so errors say where they are.
        let config = Self::parse(&text).map_err(|e| ConfigError::Parse(base_path.clone(), e))?;
        let Some(overlay) = overlay else {
            return Ok(config);
        };
        let base = toml::from_str(&text).map_err(|e| ConfigError::Parse(base_path, e))?;
        match Self::merge_overlay(base, overlay) {
            Ok(Some(merged)) => Ok(merged),
            Ok(None) => Ok(config),
            Err(e) => {
                log::error!("{e}; ignoring the overrides");
                Ok(config)
            }
        }
    }

    /// The config from `base` (already known to be valid) with the file
    /// `overlay` merged over it, or `None` without an overlay file.
    fn merge_overlay(mut base: toml::Table, overlay: &Path) -> Result<Option<Self>, ConfigError> {
        let parse_error = |e| ConfigError::Parse(overlay.to_path_buf(), e);
        let text = read_optional(overlay).map_err(|e| ConfigError::Io(overlay.to_path_buf(), e))?;
        let Some(text) = text else {
            return Ok(None);
        };
        merge(&mut base, toml::from_str(&text).map_err(parse_error)?);
        Self::from_table(base).map(Some).map_err(|e| {
            // Values in a table have no position; the overlay's own text
            // does, and is where the mistake almost always is.
            parse_error(Self::parse(&text).err().unwrap_or(e))
        })
    }

    /// Clamps values that would otherwise break rendering.
    fn sanitize(&mut self) {
        // `clamp` keeps NaN, which TOML can spell.
        let finite = |value: f32, default: f32| if value.is_finite() { value } else { default };
        let (font, window) = (FontConfig::default(), WindowConfig::default());
        self.font.size = finite(self.font.size, font.size).clamp(4.0, 200.0);
        self.font.size_step = finite(self.font.size_step, font.size_step).clamp(0.1, 10.0);
        self.window.opacity = finite(self.window.opacity, window.opacity).clamp(0.0, 1.0);
        self.window.columns = self.window.columns.clamp(2, MAX_GRID);
        self.window.lines = self.window.lines.clamp(1, MAX_GRID);
        self.window.padding = self.window.padding.min(MAX_PADDING);
        self.terminal.scrollback = self.terminal.scrollback.min(MAX_SCROLLBACK);
    }
}

/// Writes an `f32` as the shortest decimal that reads back as it, so 12.4
/// stays 12.4 rather than widening to 12.399999618530273 (TOML floats
/// are 64-bit).
fn short_f32<S: Serializer>(value: &f32, serializer: S) -> Result<S::Ok, S::Error> {
    let shortest = value.to_string().parse().unwrap_or(f64::from(*value));
    serializer.serialize_f64(shortest)
}

/// Where the config file is when `--config` doesn't say:
/// `$XDG_CONFIG_HOME/jotty/config.toml`.
pub fn default_path() -> Option<PathBuf> {
    xdg_dir("XDG_CONFIG_HOME", ".config").map(|dir| dir.join("jotty").join("config.toml"))
}

/// Where `jotty config` keeps its overrides, which take precedence over
/// the config file.
pub fn overrides_path() -> Option<PathBuf> {
    xdg_dir("XDG_STATE_HOME", ".local/state").map(|dir| dir.join("jotty").join("overrides.toml"))
}

/// `$var` when set and not empty, else `fallback` under `$HOME`.
fn xdg_dir(var: &str, fallback: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(fallback)))
}

/// A file's contents, or `None` when it doesn't exist.
pub fn read_optional(path: &Path) -> io::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Merges `over` into `base`: tables present in both are merged key by
/// key, and any other value in `over`, lists included, replaces `base`'s.
pub fn merge(base: &mut toml::Table, over: toml::Table) {
    for (key, value) in over {
        match (base.get_mut(&key), value) {
            (Some(toml::Value::Table(base)), toml::Value::Table(over)) => merge(base, over),
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_is_default() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
    }

    #[test]
    fn nonsense_numbers_are_replaced_or_limited() {
        let config = Config::parse(
            r#"
            [font]
            size = nan
            size_step = inf
            [window]
            opacity = nan
            columns = 65535
            lines = 65535
            padding = 4294967295
            [terminal]
            scrollback = 9007199254740993
            "#,
        )
        .unwrap();
        assert_eq!(config.font.size, FontConfig::default().size);
        assert_eq!(config.font.size_step, FontConfig::default().size_step);
        assert_eq!(config.window.opacity, 1.0);
        assert_eq!(config.window.columns, MAX_GRID);
        assert_eq!(config.window.lines, MAX_GRID);
        assert_eq!(config.window.padding, MAX_PADDING);
        assert_eq!(config.terminal.scrollback, MAX_SCROLLBACK);
        // The largest window these allow fits the arithmetic that sizes it.
        let cell = 200u32 * 2;
        assert!(
            u32::from(MAX_GRID)
                .checked_mul(cell)
                .and_then(|size| size.checked_add(2 * MAX_PADDING))
                .is_some()
        );
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
        assert_eq!(config.colors.cursor(), Rgb::new(255, 0, 0));
        assert_eq!(config.colors.cursor_text, Some(Rgb::new(0, 255, 0)));
        assert_eq!(config.colors.selection_foreground, Some(Rgb::new(0, 0, 1)));
        assert_eq!(
            config.colors.selection_background,
            Some(Rgb::new(255, 255, 254))
        );
        let palette = config.colors.palette();
        assert_eq!(palette[0], Rgb::new(1, 2, 3));
        assert_eq!(palette[1], Rgb::new(0xaa, 0xbb, 0xcc));
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
        assert_eq!(c, Rgb::new(0x1a, 0x2b, 0x3c));
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

    /// A scratch directory for one test.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jotty-config-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn merge_is_deep_and_replaces_lists() {
        let mut base: toml::Table = toml::from_str(
            "[font]\nfamily = \"A\"\nfallback = [\"x\", \"y\"]\n[keybinds]\ncopy = [\"ctrl+c\"]",
        )
        .unwrap();
        let over: toml::Table =
            toml::from_str("[font]\nsize = 14.0\nfallback = [\"z\"]\n[keybinds]\npaste = []")
                .unwrap();
        merge(&mut base, over);
        let config = Config::from_table(base).unwrap();
        assert_eq!(config.font.family, "A");
        assert_eq!(config.font.size, 14.0);
        assert_eq!(config.font.fallback, ["z"]);
        let both = Config::parse("[keybinds]\ncopy = [\"ctrl+c\"]\npaste = []").unwrap();
        assert_eq!(config.keybinds, both.keybinds);
    }

    #[test]
    fn overlay_takes_precedence() {
        let dir = scratch("overlay");
        let (base, overlay) = (dir.join("config.toml"), dir.join("overrides.toml"));
        std::fs::write(&base, "[font]\nfamily = \"A\"\nsize = 10.0").unwrap();
        let load = || Config::load_layers(Some((&base, true)), Some(&overlay)).unwrap();
        // No overlay file: the base alone.
        assert_eq!(
            load(),
            Config::parse("[font]\nfamily = \"A\"\nsize = 10.0").unwrap()
        );
        std::fs::write(&overlay, "[font]\nsize = 13.0").unwrap();
        assert_eq!((load().font.family.as_str(), load().font.size), ("A", 13.0));
        // A broken overlay is ignored.
        std::fs::write(&overlay, "[font]\nsiez = 13.0").unwrap();
        assert_eq!(load().font.size, 10.0);
        std::fs::write(&overlay, "not toml").unwrap();
        assert_eq!(load().font.size, 10.0);
        // A broken base is still an error.
        std::fs::write(&base, "[font]\nsiez = 1").unwrap();
        assert!(Config::load_layers(Some((&base, true)), Some(&overlay)).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn overlay_without_base_file() {
        let dir = scratch("nobase");
        let overlay = dir.join("overrides.toml");
        std::fs::write(&overlay, "[window]\npadding = 9").unwrap();
        let missing = dir.join("config.toml");
        let config = Config::load_layers(Some((&missing, false)), Some(&overlay)).unwrap();
        assert_eq!(config.window.padding, 9);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn floats_serialize_as_written() {
        let config =
            Config::parse("[font]\nsize = 12.4\nsize_step = 0.1\n[window]\nopacity = 0.8").unwrap();
        let table = toml::Table::try_from(&config).unwrap();
        assert_eq!(table["font"]["size"].as_float(), Some(12.4));
        assert_eq!(table["font"]["size_step"].as_float(), Some(0.1));
        assert_eq!(table["window"]["opacity"].as_float(), Some(0.8));
    }

    #[test]
    fn serializes_to_a_config_that_parses_back() {
        let mut config = Config::parse(
            "[terminal]\nshell = \"fish\"\n[colors]\ncursor = \"#102030\"\n\
             [keybinds]\ncopy = [\"ctrl+insert\", \"ctrl++\"]\npaste = []\n\
             [files.pdf]\nextensions = [\"pdf\"]\nlauncher = [\"zathura\"]",
        )
        .unwrap();
        config.window.renderer = RendererKind::Gpu;
        for config in [Config::default(), config] {
            let text = toml::to_string(&config).unwrap();
            assert_eq!(Config::parse(&text).unwrap(), config, "{text}");
        }
    }
}
