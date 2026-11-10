//! The `jotty config` TUI. Every accepted change is written to the
//! overrides file right away, so running windows show it as you go.

use std::io;

use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph, Wrap};

use super::schema::{self, Key, SECTIONS};
use super::{Layers, get_path, remove_path, save, set_path};
use crate::config::{Config, Rgb};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Sections,
    Keys,
}

#[derive(Debug, PartialEq, Eq)]
enum Mode {
    Normal,
    /// Editing the selected key's value; `cursor` is a char index.
    Edit {
        buffer: String,
        cursor: usize,
    },
    /// Typing the filter.
    Filter,
}

/// Everything the TUI shows, and the changes it makes, apart from drawing.
pub struct State {
    layers: Layers,
    /// The serialized effective config.
    effective: toml::Table,
    keys: Vec<Key>,
    section: usize,
    /// Index into [`State::visible`].
    selected: usize,
    focus: Focus,
    mode: Mode,
    /// Shown keys contain this, across all sections, when not empty.
    filter: String,
    /// The last result, and whether it is an error.
    message: Option<(String, bool)>,
    quit: bool,
}

impl State {
    pub fn new(layers: Layers) -> State {
        let mut state = State {
            layers,
            effective: toml::Table::new(),
            keys: Vec::new(),
            section: 0,
            selected: 0,
            focus: Focus::Sections,
            mode: Mode::Normal,
            filter: String::new(),
            message: None,
            quit: false,
        };
        state.refresh();
        state
    }

    /// Recomputes the effective config and the keys after a change.
    fn refresh(&mut self) {
        // The overrides were checked when loaded or changed, so this only
        // fails if they got out of hand; show the defaults then.
        let config = Config::from_table(self.layers.merged()).unwrap_or_default();
        self.effective = toml::Table::try_from(&config).unwrap_or_default();
        self.keys = schema::keys(&self.effective);
        self.selected = self.selected.min(self.visible().len().saturating_sub(1));
    }

    /// The keys listed on the right.
    fn visible(&self) -> Vec<&Key> {
        if self.filter.is_empty() {
            let section = SECTIONS[self.section];
            self.keys
                .iter()
                .filter(|k| k.section() == section)
                .collect()
        } else {
            let filter = self.filter.to_lowercase();
            self.keys
                .iter()
                .filter(|k| k.dotted().to_lowercase().contains(&filter))
                .collect()
        }
    }

    fn selected_key(&self) -> Option<Key> {
        self.visible().get(self.selected).map(|k| (*k).clone())
    }

    /// Checks `overrides` against the config, then makes and saves them.
    fn commit(&mut self, overrides: toml::Table) -> Result<(), String> {
        let mut merged = self.layers.base.clone();
        crate::config::merge(&mut merged, overrides.clone());
        Config::from_table(merged).map_err(|e| e.message().to_string())?;
        save(&self.layers.overrides_path, &overrides)
            .map_err(|e| format!("{}: {e}", self.layers.overrides_path.display()))?;
        self.layers.overrides = overrides;
        self.refresh();
        Ok(())
    }

    /// Overrides `key` with `value`, or removes its override for `None`.
    pub fn set(&mut self, key: &Key, value: Option<toml::Value>) -> Result<(), String> {
        let mut overrides = self.layers.overrides.clone();
        match value {
            Some(value) => set_path(&mut overrides, &key.path, value),
            None => remove_path(&mut overrides, &key.path),
        }
        self.commit(overrides)
    }

    /// Removes the overrides of every key in `section`.
    pub fn reset_section(&mut self, section: &str) -> Result<(), String> {
        let mut overrides = self.layers.overrides.clone();
        overrides.remove(section);
        self.commit(overrides)
    }

    fn report(&mut self, result: Result<String, String>) {
        self.message = Some(match result {
            Ok(done) => (done, false),
            Err(e) => (e, true),
        });
    }

    fn handle(&mut self, key: KeyEvent) {
        match std::mem::replace(&mut self.mode, Mode::Normal) {
            Mode::Normal => self.handle_normal(key),
            Mode::Edit { buffer, cursor } => self.handle_edit(key, buffer, cursor),
            Mode::Filter => self.handle_filter(key),
        }
    }

    fn handle_normal(&mut self, key: KeyEvent) {
        let ctrl_c =
            key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c');
        match key.code {
            _ if ctrl_c => self.quit = true,
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Esc if !self.filter.is_empty() => {
                self.filter.clear();
                self.selected = 0;
            }
            KeyCode::Esc => self.quit = true,
            KeyCode::Down | KeyCode::Char('j') => self.move_by(|at, last| (at + 1).min(last)),
            KeyCode::Up | KeyCode::Char('k') => self.move_by(|at, _| at.saturating_sub(1)),
            KeyCode::Home | KeyCode::Char('g') => self.move_by(|_, _| 0),
            KeyCode::End | KeyCode::Char('G') => self.move_by(|_, last| last),
            KeyCode::Tab | KeyCode::BackTab => {
                self.focus = match self.focus {
                    Focus::Sections => Focus::Keys,
                    Focus::Keys => Focus::Sections,
                }
            }
            KeyCode::Right | KeyCode::Char('l') => self.focus = Focus::Keys,
            KeyCode::Left | KeyCode::Char('h') => self.focus = Focus::Sections,
            KeyCode::Char('/') => {
                self.mode = Mode::Filter;
                self.focus = Focus::Keys;
            }
            KeyCode::Enter if self.focus == Focus::Sections => self.focus = Focus::Keys,
            KeyCode::Enter => self.edit_selected(),
            KeyCode::Char('r') if self.focus == Focus::Keys => {
                if let Some(key) = self.selected_key() {
                    let result = self.set(&key, None);
                    self.report(result.map(|()| format!("{} reset", key.dotted())));
                }
            }
            KeyCode::Char('R') => {
                let section = match (self.focus, self.selected_key()) {
                    (Focus::Keys, Some(key)) => key.section().to_string(),
                    _ => SECTIONS[self.section].to_string(),
                };
                let result = self.reset_section(&section);
                self.report(result.map(|()| format!("[{section}] reset")));
            }
            _ => {}
        }
        if self.focus == Focus::Sections && self.filter.is_empty() {
            // Moving between sections starts each at its first key.
            self.selected = 0;
        }
    }

    /// Moves the selection of the focused list to `to(current, last)`.
    fn move_by(&mut self, to: impl Fn(usize, usize) -> usize) {
        match self.focus {
            Focus::Sections => self.section = to(self.section, SECTIONS.len() - 1),
            Focus::Keys => {
                let last = self.visible().len().saturating_sub(1);
                self.selected = to(self.selected, last);
            }
        }
    }

    /// Toggles a bool or an enum, or starts editing anything else.
    fn edit_selected(&mut self) {
        let Some(key) = self.selected_key() else {
            return;
        };
        let current = get_path(&self.effective, &key.path);
        if key.kind.toggles() {
            let result = self.set(&key, key.kind.next(current));
            let shown = get_path(&self.effective, &key.path).map(schema::format);
            self.report(
                result.map(|()| format!("{} = {}", key.dotted(), shown.unwrap_or_default())),
            );
        } else {
            let buffer = current.map(schema::format).unwrap_or_default();
            let cursor = buffer.chars().count();
            self.mode = Mode::Edit { buffer, cursor };
        }
    }

    fn handle_edit(&mut self, key: KeyEvent, mut buffer: String, mut cursor: usize) {
        let byte = |buffer: &str, cursor: usize| {
            buffer
                .char_indices()
                .nth(cursor)
                .map_or(buffer.len(), |(i, _)| i)
        };
        match key.code {
            KeyCode::Esc => return,
            KeyCode::Enter => {
                if let Some(selected) = self.selected_key() {
                    let result = selected
                        .kind
                        .parse(&buffer)
                        .and_then(|value| self.set(&selected, value));
                    match result {
                        Ok(()) => self.report(Ok(format!("{} saved", selected.dotted()))),
                        Err(e) => {
                            // Keep editing, so the input isn't lost.
                            self.report(Err(e));
                            self.mode = Mode::Edit { buffer, cursor };
                        }
                    }
                }
                return;
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                buffer.clear();
                cursor = 0;
            }
            KeyCode::Char(c) => {
                buffer.insert(byte(&buffer, cursor), c);
                cursor += 1;
            }
            KeyCode::Backspace if cursor > 0 => {
                cursor -= 1;
                buffer.remove(byte(&buffer, cursor));
            }
            KeyCode::Delete if cursor < buffer.chars().count() => {
                buffer.remove(byte(&buffer, cursor));
            }
            KeyCode::Left => cursor = cursor.saturating_sub(1),
            KeyCode::Right => cursor = (cursor + 1).min(buffer.chars().count()),
            KeyCode::Home => cursor = 0,
            KeyCode::End => cursor = buffer.chars().count(),
            _ => {}
        }
        self.mode = Mode::Edit { buffer, cursor };
    }

    fn handle_filter(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => return,
            KeyCode::Esc => {
                self.filter.clear();
                return;
            }
            KeyCode::Char(c) => self.filter.push(c),
            KeyCode::Backspace => {
                self.filter.pop();
            }
            _ => {}
        }
        self.selected = 0;
        self.mode = Mode::Filter;
    }

    fn draw(&self, frame: &mut Frame) {
        let [header, body, help, footer] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(5),
            Constraint::Length(6),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        self.draw_header(frame, header);
        let [sections, keys] =
            Layout::horizontal([Constraint::Length(14), Constraint::Min(20)]).areas(body);
        self.draw_sections(frame, sections);
        self.draw_keys(frame, keys);
        self.draw_help(frame, help);
        self.draw_footer(frame, footer);
    }

    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let mut lines = vec![Line::from(vec![
            "jotty config".bold(),
            format!("  overrides: {}", tilde(&self.layers.overrides_path)).dark_gray(),
        ])];
        lines.push(match &self.layers.base_error {
            Some(e) => Line::from(format!("config file ignored: {e}")).red(),
            None => Line::from("Move them into home-manager with: jotty config export --nix")
                .dark_gray(),
        });
        frame.render_widget(Paragraph::new(lines), area);
    }

    fn focused_block(&self, title: &'static str, focus: Focus) -> Block<'static> {
        let block = Block::bordered().title(title);
        if self.focus == focus {
            block.border_style(Style::new().fg(Color::Cyan))
        } else {
            block.border_style(Style::new().fg(Color::DarkGray))
        }
    }

    fn draw_sections(&self, frame: &mut Frame, area: Rect) {
        let items = SECTIONS.iter().map(|section| {
            let overridden = self.layers.overrides.contains_key(*section);
            let mark = if overridden { "● " } else { "  " };
            ListItem::new(Line::from(vec![mark.yellow(), Span::raw(*section)]))
        });
        let mut state = ListState::default();
        if self.filter.is_empty() {
            state.select(Some(self.section));
        }
        let list = List::new(items)
            .block(self.focused_block(" sections ", Focus::Sections))
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
        frame.render_stateful_widget(list, area, &mut state);
    }

    fn draw_keys(&self, frame: &mut Frame, area: Rect) {
        let visible = self.visible();
        let label = |key: &Key| {
            if self.filter.is_empty() {
                key.name()
            } else {
                key.dotted()
            }
        };
        let width = visible
            .iter()
            .map(|k| label(k).chars().count())
            .max()
            .unwrap_or(0);
        let editing = matches!(self.mode, Mode::Edit { .. });
        let items = visible.iter().enumerate().map(|(i, key)| {
            let overridden = get_path(&self.layers.overrides, &key.path).is_some();
            let mark = if overridden { "● " } else { "  " };
            let value = get_path(&self.effective, &key.path);
            let mut spans = vec![mark.yellow(), Span::raw(format!("{:width$}  ", label(key)))];
            if let Mode::Edit { buffer, .. } = &self.mode
                && i == self.selected
            {
                spans.push(Span::raw(buffer.clone()).underlined());
            } else {
                spans.extend(value_spans(value));
            }
            ListItem::new(Line::from(spans))
        });
        let title = if self.filter.is_empty() {
            format!(" {} ", SECTIONS[self.section])
        } else {
            format!(" /{} ", self.filter)
        };
        let block = Block::bordered().title(title).border_style(Style::new().fg(
            if self.focus == Focus::Keys {
                Color::Cyan
            } else {
                Color::DarkGray
            },
        ));
        let mut state = ListState::default();
        if self.focus == Focus::Keys || !self.filter.is_empty() {
            state.select(Some(self.selected));
        }
        let highlight = if editing {
            Style::new()
        } else {
            Style::new().add_modifier(Modifier::REVERSED)
        };
        let list = List::new(items).block(block).highlight_style(highlight);
        frame.render_stateful_widget(list, area, &mut state);
        if let Mode::Edit { cursor, .. } = &self.mode {
            // Border, mark and label come before the text.
            let offset = state.offset();
            let x = area.x + 1 + 2 + width as u16 + 2 + *cursor as u16;
            let y = area.y + 1 + (self.selected.saturating_sub(offset)) as u16;
            frame.set_cursor_position(Position::new(x.min(area.right() - 2), y));
        }
    }

    fn draw_help(&self, frame: &mut Frame, area: Rect) {
        let mut lines = Vec::new();
        if let Some(key) = self.selected_key() {
            lines.push(Line::from(vec![
                key.dotted().bold(),
                format!("  {}", key.kind.describe()).dark_gray(),
            ]));
            lines.push(Line::from(key.doc.to_string()));
            let base = get_path(&self.layers.base, &key.path).map(schema::format);
            let source = match (get_path(&self.layers.overrides, &key.path), base) {
                (Some(_), Some(base)) => format!("overridden; the config file has {base}"),
                (Some(_), None) => "overridden; not in the config file".into(),
                (None, Some(_)) => "from the config file".into(),
                (None, None) => "default".into(),
            };
            lines.push(Line::from(source).dark_gray());
        }
        if let Some((message, error)) = &self.message {
            let line = Line::from(message.clone());
            lines.push(if *error { line.red() } else { line.green() });
        }
        let block = Block::bordered().border_style(Style::new().fg(Color::DarkGray));
        frame.render_widget(
            Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
            area,
        );
    }

    fn draw_footer(&self, frame: &mut Frame, area: Rect) {
        let help = match self.mode {
            Mode::Normal => {
                "Enter edit/toggle  r reset key  R reset section  / filter  Tab switch  q quit"
            }
            Mode::Edit { .. } => {
                "Enter save  Esc cancel  Ctrl+U clear  (empty resets optional keys)"
            }
            Mode::Filter => "type to filter  Enter keep  Esc clear",
        };
        frame.render_widget(Paragraph::new(help).dark_gray(), area);
    }
}

/// A value, with a swatch for colors.
fn value_spans(value: Option<&toml::Value>) -> Vec<Span<'static>> {
    let Some(value) = value else {
        return vec!["(default)".dark_gray()];
    };
    let swatch = |s: &str| {
        s.parse::<Rgb>()
            .ok()
            .map(|Rgb(r, g, b)| Span::raw("  ").bg(Color::Rgb(r, g, b)))
    };
    let mut spans = Vec::new();
    match value {
        toml::Value::String(s) => {
            if let Some(swatch) = swatch(s) {
                spans.extend([swatch, Span::raw(" ")]);
            }
        }
        toml::Value::Array(items) if !items.is_empty() => {
            let swatches: Option<Vec<_>> =
                items.iter().map(|v| v.as_str().and_then(swatch)).collect();
            if let Some(swatches) = swatches {
                spans.extend(swatches);
                spans.push(Span::raw(" "));
            }
        }
        _ => {}
    }
    match value {
        toml::Value::Array(items) if items.is_empty() => spans.push("(empty)".dark_gray()),
        _ => spans.push(Span::raw(schema::format(value))),
    }
    spans
}

/// `path` with the home directory written as `~`.
fn tilde(path: &std::path::Path) -> String {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    match home
        .as_deref()
        .and_then(|home| path.strip_prefix(home).ok())
    {
        Some(rest) if !rest.as_os_str().is_empty() => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}

/// Runs the TUI until the user quits.
pub fn run(layers: Layers) -> io::Result<()> {
    let mut state = State::new(layers);
    let mut terminal = ratatui::try_init()?;
    let result = (|| {
        while !state.quit {
            terminal.draw(|frame| state.draw(frame))?;
            if let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                state.handle(key);
            }
        }
        Ok(())
    })();
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(base: &str, overrides: &str) -> (State, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "jotty-tui-{}-{}",
            std::process::id(),
            rand_suffix()
        ));
        let layers = Layers {
            base: toml::from_str(base).unwrap(),
            base_error: None,
            overrides_path: dir.join("overrides.toml"),
            overrides: toml::from_str(overrides).unwrap(),
        };
        (State::new(layers), dir)
    }

    /// Distinguishes the tests' directories, which run in parallel.
    fn rand_suffix() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        NEXT.fetch_add(1, Ordering::Relaxed)
    }

    fn key(state: &State, dotted: &str) -> Key {
        state
            .keys
            .iter()
            .find(|k| k.dotted() == dotted)
            .unwrap()
            .clone()
    }

    fn press(state: &mut State, code: KeyCode) {
        state.handle(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn saved(dir: &std::path::Path) -> Option<toml::Table> {
        std::fs::read_to_string(dir.join("overrides.toml"))
            .ok()
            .map(|t| toml::from_str(&t).unwrap())
    }

    #[test]
    fn edits_are_saved_and_reset() {
        let (mut state, dir) = state("[font]\nsize = 10.0", "");
        let size = key(&state, "font.size");
        state.set(&size, Some(toml::Value::Float(14.0))).unwrap();
        assert_eq!(
            get_path(&state.effective, &size.path),
            Some(&toml::Value::Float(14.0))
        );
        assert_eq!(
            saved(&dir),
            Some(toml::from_str("[font]\nsize = 14.0").unwrap())
        );
        state.set(&size, None).unwrap();
        assert_eq!(
            get_path(&state.effective, &size.path),
            Some(&toml::Value::Float(10.0))
        );
        // Nothing left: the file is gone, and so is the empty [font].
        assert_eq!(saved(&dir), None);
        assert!(state.layers.overrides.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_overrides_are_rejected() {
        let (mut state, dir) = state("", "");
        let copy = key(&state, "keybinds.copy");
        let bad = toml::Value::Array(vec![toml::Value::String("ctrl+bogus".into())]);
        let err = state.set(&copy, Some(bad)).unwrap_err();
        assert!(err.contains("bogus"), "{err}");
        assert!(state.layers.overrides.is_empty());
        assert_eq!(saved(&dir), None);
    }

    #[test]
    fn typing_a_value() {
        let (mut state, dir) = state("", "");
        // window, then its padding key.
        press(&mut state, KeyCode::Char('j'));
        press(&mut state, KeyCode::Tab);
        let at = state
            .visible()
            .iter()
            .position(|k| k.name() == "padding")
            .unwrap();
        state.selected = at;
        press(&mut state, KeyCode::Enter);
        assert_eq!(
            state.mode,
            Mode::Edit {
                buffer: "4".into(),
                cursor: 1
            }
        );
        press(&mut state, KeyCode::Backspace);
        for c in "12".chars() {
            press(&mut state, KeyCode::Char(c));
        }
        press(&mut state, KeyCode::Enter);
        assert_eq!(state.mode, Mode::Normal);
        assert_eq!(
            saved(&dir),
            Some(toml::from_str("[window]\npadding = 12").unwrap())
        );
        // Out of range: an error, and the input stays.
        press(&mut state, KeyCode::Enter);
        press(&mut state, KeyCode::Char('0'));
        press(&mut state, KeyCode::Char('0'));
        press(&mut state, KeyCode::Char('0'));
        press(&mut state, KeyCode::Enter);
        assert!(matches!(state.message, Some((_, true))));
        assert_eq!(
            state.mode,
            Mode::Edit {
                buffer: "12000".into(),
                cursor: 5
            }
        );
        press(&mut state, KeyCode::Esc);
        assert_eq!(state.mode, Mode::Normal);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn toggling_and_resetting_a_section() {
        let (mut state, dir) = state("", "[font]\nsize = 20.0");
        let blur = key(&state, "window.blur");
        state.filter = "blur".into();
        state.focus = Focus::Keys;
        state.selected = state.visible().iter().position(|k| **k == blur).unwrap();
        press(&mut state, KeyCode::Enter);
        assert_eq!(
            get_path(&state.layers.overrides, &blur.path),
            Some(&toml::Value::Boolean(true))
        );
        state.reset_section("window").unwrap();
        assert_eq!(
            saved(&dir),
            Some(toml::from_str("[font]\nsize = 20.0").unwrap())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn filter_searches_every_section() {
        let (mut state, _dir) = state("", "");
        press(&mut state, KeyCode::Char('/'));
        for c in "launcher".chars() {
            press(&mut state, KeyCode::Char(c));
        }
        let names: Vec<String> = state.visible().iter().map(|k| k.dotted()).collect();
        assert!(names.contains(&"url.launcher".to_string()));
        assert!(names.contains(&"files.image.launcher".to_string()));
        assert!(names.iter().all(|n| n.contains("launcher")));
    }
}
