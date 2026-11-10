//! The `jotty config` TUI. Every accepted change is written to the
//! overrides file right away, so running windows show it as you go.

use std::io;

use ratatui::Frame;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};

use super::Layers;
use super::schema::{self, Key, SECTIONS};
use crate::config::{Config, DEFAULT_PALETTE, Rgb};
use crate::settings;
use unicode_width::UnicodeWidthChar;

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
    /// The key binding table, over everything else.
    Help,
}

/// Everything the TUI shows, and the changes it makes, apart from drawing.
pub struct State {
    layers: Layers,
    /// The serialized effective config.
    effective: toml::Table,
    /// The 16 colors in effect, and those without the overrides.
    palette: [Rgb; 16],
    base_palette: [Rgb; 16],
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
    /// Why the overrides can't apply over the config file, when they can't.
    merge_error: Option<String>,
    /// The section `R` was pressed for, waiting for a second `R`.
    confirm_reset: Option<String>,
    quit: bool,
}

impl State {
    pub fn new(layers: Layers) -> State {
        let mut state = State {
            layers,
            effective: toml::Table::new(),
            palette: DEFAULT_PALETTE,
            base_palette: DEFAULT_PALETTE,
            keys: Vec::new(),
            section: 0,
            selected: 0,
            focus: Focus::Sections,
            mode: Mode::Normal,
            filter: String::new(),
            message: None,
            merge_error: None,
            confirm_reset: None,
            quit: false,
        };
        state.refresh();
        state
    }

    /// Recomputes the effective config and the keys after a change.
    fn refresh(&mut self) {
        // A hand edit can make overrides that are valid alone but not over
        // the config file. jotty then ignores them; show what it uses.
        let base = Config::from_table(self.layers.base.clone()).unwrap_or_default();
        let config = match Config::from_table(self.layers.merged()) {
            Ok(config) => {
                self.merge_error = None;
                config
            }
            Err(e) => {
                self.merge_error = Some(e.message().to_string());
                base.clone()
            }
        };
        self.effective = toml::Table::try_from(&config).unwrap_or_default();
        self.palette = config.colors.palette();
        self.base_palette = base.colors.palette();
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
        settings::save(&self.layers.overrides_path, &overrides)
            .map_err(|e| format!("{}: {e}", self.layers.overrides_path.display()))?;
        self.layers.overrides = overrides;
        self.refresh();
        Ok(())
    }

    /// The overrides as saved now: another `jotty config`, a `reset` or a
    /// hand edit may have changed them since this one started.
    fn current_overrides(&self) -> Result<toml::Table, String> {
        settings::read_table(&self.layers.overrides_path)
            .map(Option::unwrap_or_default)
            .map_err(|e| format!("not saving over a broken file: {e}"))
    }

    /// The value of `key` in effect.
    fn effective_of(&self, key: &Key) -> Option<toml::Value> {
        match key.index {
            Some(i) => Some(toml::Value::String(self.palette[i].to_string())),
            None => settings::get_path(&self.effective, &key.path).cloned(),
        }
    }

    /// The value the overrides give `key`, if they change it.
    fn override_of(&self, key: &Key) -> Option<toml::Value> {
        let value = settings::get_path(&self.layers.overrides, &key.path)?;
        match key.index {
            // The list replaces the config file's as a whole, so a color
            // counts as overridden when it ends up different, also when a
            // hand-written list is too short to name it.
            Some(i) => (self.palette[i] != self.base_palette[i])
                .then(|| toml::Value::String(self.palette[i].to_string())),
            None => Some(value.clone()),
        }
    }

    /// The value the config file gives `key`.
    fn base_of(&self, key: &Key) -> Option<toml::Value> {
        let value = settings::get_path(&self.layers.base, &key.path)?;
        match key.index {
            Some(i) => value.as_array()?.get(i).cloned(),
            None => Some(value.clone()),
        }
    }

    /// Overrides `key` with `value`, or removes its override for `None`.
    pub fn set(&mut self, key: &Key, value: Option<toml::Value>) -> Result<(), String> {
        let mut overrides = self.current_overrides()?;
        match (key.index, value) {
            (Some(i), value) => {
                // One color changes the whole list, starting from the colors
                // as saved now. Without differences from the config file
                // left, the override goes.
                let mut merged = self.layers.base.clone();
                crate::config::merge(&mut merged, overrides.clone());
                let mut palette = Config::from_table(merged)
                    .map_err(|e| e.message().to_string())?
                    .colors
                    .palette();
                palette[i] = match value.as_ref().and_then(toml::Value::as_str) {
                    Some(color) => color.parse()?,
                    None => self.base_palette[i],
                };
                if palette == self.base_palette {
                    settings::remove_path(&mut overrides, &key.path);
                } else {
                    let list = palette.iter().map(|c| toml::Value::String(c.to_string()));
                    settings::set_path(
                        &mut overrides,
                        &key.path,
                        toml::Value::Array(list.collect()),
                    );
                }
            }
            (None, Some(value)) => settings::set_path(&mut overrides, &key.path, value),
            (None, None) => settings::remove_path(&mut overrides, &key.path),
        }
        self.commit(overrides)
    }

    /// Removes the overrides of every key in `section`.
    pub fn reset_section(&mut self, section: &str) -> Result<(), String> {
        let mut overrides = self.current_overrides()?;
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
            Mode::Normal => {
                // The last result stays only until the next key.
                self.message = None;
                self.handle_normal(key)
            }
            Mode::Edit { buffer, cursor } => self.handle_edit(key, buffer, cursor),
            Mode::Filter => self.handle_filter(key),
            // Any key closes the help; it does nothing else.
            Mode::Help => {}
        }
    }

    fn handle_normal(&mut self, key: KeyEvent) {
        // Any key but a second R cancels a pending section reset.
        let confirm = self.confirm_reset.take();
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
            KeyCode::Tab | KeyCode::BackTab if self.focus == Focus::Keys => self.focus_sections(),
            KeyCode::Tab | KeyCode::BackTab => self.focus = Focus::Keys,
            KeyCode::Right | KeyCode::Char('l') => self.focus = Focus::Keys,
            KeyCode::Left | KeyCode::Char('h') => self.focus_sections(),
            KeyCode::Char('/') => {
                self.mode = Mode::Filter;
                self.focus = Focus::Keys;
            }
            KeyCode::Enter if self.focus == Focus::Sections => self.focus = Focus::Keys,
            KeyCode::Enter => self.edit_selected(),
            KeyCode::Char(' ') if self.focus == Focus::Keys => self.toggle_selected(),
            KeyCode::Char('i') if self.focus == Focus::Keys => self.start_edit(),
            KeyCode::Char('?') => self.mode = Mode::Help,
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
                if confirm.as_deref() == Some(section.as_str()) {
                    let result = self.reset_section(&section);
                    self.report(result.map(|()| format!("[{section}] reset")));
                } else {
                    self.report(Ok(format!("Press R again to reset all of [{section}]")));
                    self.confirm_reset = Some(section);
                }
            }
            _ => {}
        }
        if self.focus == Focus::Sections && self.filter.is_empty() {
            // Moving between sections starts each at its first key.
            self.selected = 0;
        }
    }

    /// Moves to the section list. A filter spans every section, so it is
    /// cleared: the list then shows which section is current.
    fn focus_sections(&mut self) {
        self.focus = Focus::Sections;
        self.filter.clear();
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
        match self.selected_key() {
            Some(key) if key.kind.toggles() => self.toggle_selected(),
            Some(_) => self.start_edit(),
            None => {}
        }
    }

    /// Flips a bool or moves a choice to its next value.
    fn toggle_selected(&mut self) {
        let Some(key) = self.selected_key() else {
            return;
        };
        if !key.kind.toggles() {
            return self.report(Err(
                "Space toggles bools and choices; press i to edit".into()
            ));
        }
        let result = self.set(&key, key.kind.next(self.effective_of(&key).as_ref()));
        let shown = self.effective_of(&key).as_ref().map(schema::format);
        self.report(result.map(|()| format!("{} = {}", key.dotted(), shown.unwrap_or_default())));
    }

    /// Edits the selected value as text, whatever its kind.
    fn start_edit(&mut self) {
        let Some(key) = self.selected_key() else {
            return;
        };
        let buffer = self
            .effective_of(&key)
            .as_ref()
            .map(schema::format)
            .unwrap_or_default();
        let cursor = buffer.chars().count();
        self.mode = Mode::Edit { buffer, cursor };
    }

    fn handle_edit(&mut self, key: KeyEvent, mut buffer: String, mut cursor: usize) {
        let byte = |buffer: &str, cursor: usize| {
            buffer
                .char_indices()
                .nth(cursor)
                .map_or(buffer.len(), |(i, _)| i)
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            // Cancelled: its error, if any, no longer applies.
            KeyCode::Esc => {
                self.message = None;
                return;
            }
            KeyCode::Char('c') if ctrl => {
                self.message = None;
                return;
            }
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
            KeyCode::Char('u') if ctrl => {
                buffer.clear();
                cursor = 0;
            }
            KeyCode::Char(c) if !ctrl && !alt => {
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
                self.selected = 0;
                return;
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.filter.push(c)
            }
            KeyCode::Backspace => {
                self.filter.pop();
            }
            _ => {}
        }
        self.selected = 0;
        self.mode = Mode::Filter;
    }

    /// Pasted text goes into the value or the filter being typed, on one
    /// line: a pasted newline must not save half of it.
    fn paste(&mut self, text: &str) {
        let text: String = text.chars().filter(|c| !c.is_control()).collect();
        match &mut self.mode {
            Mode::Edit { buffer, cursor } => {
                let at = buffer
                    .char_indices()
                    .nth(*cursor)
                    .map_or(buffer.len(), |(i, _)| i);
                buffer.insert_str(at, &text);
                *cursor += text.chars().count();
            }
            Mode::Filter => {
                self.filter.push_str(&text);
                self.selected = 0;
            }
            Mode::Normal | Mode::Help => {}
        }
    }

    fn draw(&self, frame: &mut Frame) {
        let [header, body, help, footer] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(5),
            Constraint::Length(7),
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
        if self.mode == Mode::Help {
            draw_bindings(frame);
        }
    }

    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let mut lines = vec![Line::from(vec![
            "jotty config".bold(),
            format!("  overrides: {}", tilde(&self.layers.overrides_path)).dark_gray(),
        ])];
        lines.push(match (&self.layers.base_error, &self.merge_error) {
            (Some(e), _) => Line::from(format!(
                "The config file is broken, so jotty applies no overrides until it is fixed: {e}"
            ))
            .red(),
            (None, Some(e)) => Line::from(format!(
                "jotty ignores these overrides, which conflict with the config file: {e}"
            ))
            .red(),
            (None, None) => {
                Line::from("Move them into home-manager with: jotty config export --nix")
                    .dark_gray()
            }
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
            // A filter searches every section; none of them is current.
            let name = if self.filter.is_empty() && self.mode != Mode::Filter {
                Span::raw(*section)
            } else {
                section.dark_gray()
            };
            ListItem::new(Line::from(vec![mark.yellow(), name]))
        });
        let mut state = ListState::default();
        if self.filter.is_empty() && self.mode != Mode::Filter {
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
                key.list_name()
            } else {
                format!("{}.{}", key.section(), key.list_name())
            }
        };
        let width = visible
            .iter()
            .map(|k| text_width(&label(k)))
            .max()
            .unwrap_or(0);
        let editing = matches!(self.mode, Mode::Edit { .. });
        // Border, mark and label come before a value; one column is left
        // for the cursor at the end.
        let text_x = area
            .x
            .saturating_add(u16::try_from(5 + width).unwrap_or(u16::MAX));
        let room = usize::from(area.right().saturating_sub(1).saturating_sub(text_x));
        let window = match &self.mode {
            Mode::Edit { buffer, cursor } => Some(edit_window(buffer, *cursor, room)),
            _ => None,
        };
        let highlight_row = |i: usize| {
            i == self.selected && !editing && (self.focus == Focus::Keys || !self.filter.is_empty())
        };
        let items = visible.iter().enumerate().map(|(i, key)| {
            let overridden = self.override_of(key).is_some();
            let mark = if overridden { "● " } else { "  " };
            let value = self.effective_of(key);
            let mut spans = vec![mark.yellow(), Span::raw(format!("{:width$}  ", label(key)))];
            if let (Mode::Edit { buffer, .. }, Some((start, end, _))) = (&self.mode, window)
                && i == self.selected
            {
                spans.push(Span::raw(buffer[start..end].to_string()).underlined());
            } else {
                spans.extend(value_spans(value.as_ref()));
            }
            // Highlighted as the row's own style, which the swatches can
            // undo; a highlight style would be drawn over them.
            let style = if highlight_row(i) {
                Style::new().add_modifier(Modifier::REVERSED)
            } else {
                Style::new()
            };
            ListItem::new(Line::from(spans)).style(style)
        });
        let filtering = self.mode == Mode::Filter;
        let matches = format!("  {} found ", visible.len());
        let title = if filtering {
            // The filter being typed, as an input field in the title.
            Line::from(vec![
                FILTER_BADGE.black().on_yellow().bold(),
                " ".into(),
                self.filter.clone().yellow().bold(),
                matches.dark_gray(),
            ])
        } else if self.filter.is_empty() {
            Line::from(format!(" {} ", SECTIONS[self.section]))
        } else {
            Line::from(vec![
                " filter: ".yellow(),
                self.filter.clone().yellow().bold(),
                matches.dark_gray(),
                "(/ change, Esc clear) ".dark_gray(),
            ])
        };
        let border = match (filtering, self.focus) {
            (true, _) => Color::Yellow,
            (false, Focus::Keys) => Color::Cyan,
            (false, Focus::Sections) => Color::DarkGray,
        };
        let block = Block::bordered()
            .title(title)
            .border_style(Style::new().fg(border));
        let items: Vec<ListItem> = items.collect();
        let items = if items.is_empty() && !self.filter.is_empty() {
            vec![ListItem::new("  no key matches".dark_gray())]
        } else {
            items
        };
        let mut state = ListState::default();
        if self.focus == Focus::Keys || !self.filter.is_empty() {
            state.select(Some(self.selected));
        }
        let list = List::new(items).block(block);
        frame.render_stateful_widget(list, area, &mut state);
        if filtering {
            // After the badge and the space, at the end of the filter.
            let column = text_width(FILTER_BADGE) + 1 + text_width(&self.filter);
            let x = area
                .x
                .saturating_add(1)
                .saturating_add(u16::try_from(column).unwrap_or(u16::MAX));
            let last = area.right().saturating_sub(2).max(area.x);
            frame.set_cursor_position(Position::new(x.min(last), area.y));
        }
        if let Some((_, _, column)) = window {
            let row = self.selected.saturating_sub(state.offset());
            let x = text_x.saturating_add(u16::try_from(column).unwrap_or(u16::MAX));
            let y = area
                .y
                .saturating_add(1)
                .saturating_add(u16::try_from(row).unwrap_or(u16::MAX));
            let last = |start: u16, end: u16| end.saturating_sub(2).max(start);
            frame.set_cursor_position(Position::new(
                x.min(last(area.x, area.right())),
                y.min(last(area.y, area.bottom())),
            ));
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
            let base = self.base_of(&key).as_ref().map(schema::format);
            let source = match (self.override_of(&key), base) {
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
        // (key, what it does); an empty key is a plain note.
        let hints: &[(&str, &str)] = match self.mode {
            Mode::Normal => &[
                ("Enter", "edit"),
                ("Space", "toggle"),
                ("i", "edit text"),
                ("r", "reset"),
                ("/", "filter"),
                ("?", "help"),
                ("q", "quit"),
            ],
            Mode::Help => &[("", "any key closes the help")],
            Mode::Edit { .. } => &[
                ("Enter", "save"),
                ("Esc", "cancel"),
                ("Ctrl+U", "clear"),
                ("", "empty resets optional keys"),
            ],
            Mode::Filter => &[
                ("", "type to filter every section"),
                ("Enter", "done"),
                ("Esc", "clear"),
            ],
        };
        let mut spans = Vec::new();
        if self.mode == Mode::Filter {
            spans.extend([FILTER_BADGE.black().on_yellow().bold(), " ".into()]);
        }
        for (i, (key, what)) in hints.iter().enumerate() {
            if i > 0 {
                spans.push(" • ".dark_gray());
            }
            if !key.is_empty() {
                spans.extend([key.cyan().bold(), " ".into()]);
            }
            spans.push(what.dark_gray());
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }
}

/// The keys of the TUI, as `?` lists them: groups of (keys, what they do).
const BINDINGS: [(&str, &[(&str, &str)]); 2] = [
    (
        "Browsing",
        &[
            ("↑ ↓  j k", "move"),
            ("g G  Home End", "first, last"),
            ("Tab  ← →  h l", "switch between sections and keys"),
            ("Enter", "toggle or cycle, or edit anything else"),
            ("Space", "toggle a bool, cycle a choice"),
            ("i", "edit the value as text"),
            ("r", "reset the key to the config file"),
            ("R R", "reset the whole section"),
            ("/", "filter keys in every section"),
            ("Esc", "clear the filter, or quit"),
            ("?", "show this help"),
            ("q  Ctrl+C", "quit"),
        ],
    ),
    (
        "Editing",
        &[
            ("Enter", "save"),
            ("Esc  Ctrl+C", "cancel"),
            ("Ctrl+U", "clear"),
            ("← →  Home End", "move the cursor"),
        ],
    ),
];

/// The key binding table, centered over the rest.
fn draw_bindings(frame: &mut Frame) {
    let width = BINDINGS
        .iter()
        .flat_map(|(_, keys)| keys.iter())
        .map(|(keys, _)| text_width(keys))
        .max()
        .unwrap_or(0);
    let mut lines = Vec::new();
    for (group, keys) in BINDINGS {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.push(Line::from(group.bold()));
        for (keys, what) in keys {
            let pad = " ".repeat(width.saturating_sub(text_width(keys)));
            lines.push(Line::from(vec![
                format!("  {keys}{pad}  ").cyan(),
                Span::raw(*what),
            ]));
        }
    }
    let height = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX);
    let area = frame
        .area()
        .centered(Constraint::Length(60), Constraint::Length(height));
    let block = Block::bordered()
        .title(" keys ")
        .title_bottom(Line::from(" any key closes ").right_aligned())
        .border_style(Style::new().fg(Color::Cyan));
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// Marks the filter while it is typed, in the title and the footer.
const FILTER_BADGE: &str = " FILTER ";

/// A value, with a swatch for colors.
fn value_spans(value: Option<&toml::Value>) -> Vec<Span<'static>> {
    let Some(value) = value else {
        return vec!["(default)".dark_gray()];
    };
    let swatch = |s: &str| {
        s.parse::<Rgb>()
            .ok()
            // Not reversed with a highlighted row, or it shows the row's color.
            .map(|Rgb(r, g, b)| Span::raw("  ").bg(Color::Rgb(r, g, b)).not_reversed())
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

/// Columns `s` takes on screen.
fn text_width(s: &str) -> usize {
    s.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// The part of `buffer` to show in `room` columns so that the char at
/// `cursor` is visible: its byte range, and the cursor's column in it.
fn edit_window(buffer: &str, cursor: usize, room: usize) -> (usize, usize, usize) {
    let chars: Vec<(usize, char)> = buffer.char_indices().collect();
    let cursor = cursor.min(chars.len());
    let width = |range: &[(usize, char)]| {
        range
            .iter()
            .map(|&(_, c)| c.width().unwrap_or(0))
            .sum::<usize>()
    };
    // Scroll right just enough to fit everything up to the cursor.
    let mut first = 0;
    while first < cursor && width(&chars[first..cursor]) > room.saturating_sub(1) {
        first += 1;
    }
    let column = width(&chars[first..cursor]);
    let mut last = cursor;
    while last < chars.len() && width(&chars[first..=last]) <= room {
        last += 1;
    }
    let byte = |i: usize| chars.get(i).map_or(buffer.len(), |&(b, _)| b);
    (byte(first), byte(last), column)
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
        execute!(io::stdout(), EnableBracketedPaste)?;
        while !state.quit {
            terminal.draw(|frame| state.draw(frame))?;
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => state.handle(key),
                Event::Paste(text) => state.paste(&text),
                _ => {}
            }
        }
        Ok(())
    })();
    // Best effort, like the rest of restoring the terminal.
    let _ = execute!(io::stdout(), DisableBracketedPaste);
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
        settings::save(&layers.overrides_path, &layers.overrides).unwrap();
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
            settings::get_path(&state.effective, &size.path),
            Some(&toml::Value::Float(14.0))
        );
        assert_eq!(
            saved(&dir),
            Some(toml::from_str("[font]\nsize = 14.0").unwrap())
        );
        state.set(&size, None).unwrap();
        assert_eq!(
            settings::get_path(&state.effective, &size.path),
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
            settings::get_path(&state.layers.overrides, &blur.path),
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
    fn the_edit_window_follows_the_cursor() {
        // Everything fits.
        assert_eq!(edit_window("abc", 3, 10), (0, 3, 3));
        // Scrolled so the cursor at the end is in the last column.
        assert_eq!(edit_window("abcdefgh", 8, 4), (5, 8, 3));
        // At the start, the rest is cut off.
        assert_eq!(edit_window("abcdefgh", 0, 4), (0, 4, 0));
        // Wide characters count twice.
        assert_eq!(edit_window("日本語", 3, 5), (3, 9, 4));
        assert_eq!(edit_window("", 0, 0), (0, 0, 0));
    }

    #[test]
    fn section_reset_needs_a_second_press() {
        let (mut state, dir) = state("", "[font]\nsize = 20.0");
        press(&mut state, KeyCode::Char('R'));
        assert!(!state.layers.overrides.is_empty());
        // Anything else in between cancels it.
        press(&mut state, KeyCode::Char('j'));
        press(&mut state, KeyCode::Char('k'));
        press(&mut state, KeyCode::Char('R'));
        press(&mut state, KeyCode::Char('R'));
        assert!(state.layers.overrides.is_empty());
        assert_eq!(saved(&dir), None);
    }

    #[test]
    fn changes_made_elsewhere_are_kept() {
        let (mut state, dir) = state("", "");
        let size = key(&state, "font.size");
        state.set(&size, Some(toml::Value::Float(14.0))).unwrap();
        // Another jotty config sets a color meanwhile.
        std::fs::write(
            dir.join("overrides.toml"),
            "[font]\nsize = 14.0\n[colors]\nbackground = \"#000000\"",
        )
        .unwrap();
        state
            .set(
                &key(&state, "window.blur"),
                Some(toml::Value::Boolean(true)),
            )
            .unwrap();
        let saved = saved(&dir).unwrap();
        assert!(saved.contains_key("colors") && saved.contains_key("window"));
        // A broken file isn't overwritten.
        std::fs::write(dir.join("overrides.toml"), "junk =").unwrap();
        assert!(state.set(&size, None).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pasting_stays_on_one_line() {
        let (mut state, _dir) = state("", "");
        state.mode = Mode::Edit {
            buffer: "ab".into(),
            cursor: 1,
        };
        state.paste("x\ny");
        assert_eq!(
            state.mode,
            Mode::Edit {
                buffer: "axyb".into(),
                cursor: 3
            }
        );
        // Ctrl+C cancels, rather than typing a c.
        state.handle(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert_eq!(state.mode, Mode::Normal);
    }

    #[test]
    fn cancelling_an_edit_clears_its_error() {
        let (mut state, _dir) = state("", "");
        state.focus = Focus::Keys;
        state.mode = Mode::Edit {
            buffer: "x".into(),
            cursor: 1,
        };
        state.selected = state
            .visible()
            .iter()
            .position(|k| k.name() == "size")
            .unwrap();
        press(&mut state, KeyCode::Enter);
        assert!(matches!(state.message, Some((_, true))));
        press(&mut state, KeyCode::Esc);
        assert_eq!(state.message, None);
        // A result is shown until the next key.
        state.report(Ok("done".into()));
        press(&mut state, KeyCode::Char('j'));
        assert_eq!(state.message, None);
    }

    #[test]
    fn palette_colors_are_listed_and_set_one_by_one() {
        let (mut state, dir) = state("[colors]\npalette = [\"#010101\"]", "");
        let names: Vec<String> = state
            .keys
            .iter()
            .filter(|k| k.index.is_some())
            .map(|k| k.name())
            .collect();
        assert_eq!(names.len(), 16);
        assert_eq!(names[1], "palette[1] red");
        assert_eq!(names[10], "palette[10] bright green");
        let red = key(&state, "colors.palette[1] red");
        let black = key(&state, "colors.palette[0] black");
        assert_eq!(
            state.base_of(&black),
            Some(toml::Value::String("#010101".into()))
        );
        state
            .set(&red, Some(toml::Value::String("#ff0000".into())))
            .unwrap();
        assert_eq!(state.palette[1], Rgb(0xff, 0, 0));
        assert_eq!(state.palette[0], Rgb(1, 1, 1));
        assert!(state.override_of(&red).is_some());
        // Saved as all 16, but only red counts as overridden.
        let saved_palette = saved(&dir).unwrap()["colors"]["palette"]
            .as_array()
            .unwrap()
            .len();
        assert_eq!(saved_palette, 16);
        assert!(state.override_of(&black).is_none());
        // Resetting the only change drops the override.
        state.set(&red, None).unwrap();
        assert_eq!(saved(&dir), None);
        assert_eq!(state.palette[1], DEFAULT_PALETTE[1]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn space_toggles_and_i_edits() {
        let (mut state, dir) = state("", "");
        let blur = key(&state, "window.blur");
        state.focus = Focus::Keys;
        state.section = 1;
        state.selected = state.visible().iter().position(|k| **k == blur).unwrap();
        press(&mut state, KeyCode::Char(' '));
        assert_eq!(state.override_of(&blur), Some(toml::Value::Boolean(true)));
        // i edits even a bool as text.
        press(&mut state, KeyCode::Char('i'));
        assert_eq!(
            state.mode,
            Mode::Edit {
                buffer: "true".into(),
                cursor: 4
            }
        );
        press(&mut state, KeyCode::Esc);
        // Space on text says what to do instead, and changes nothing.
        state.selected = 0;
        press(&mut state, KeyCode::Char(' '));
        assert!(matches!(state.message, Some((_, true))));
        assert_eq!(state.mode, Mode::Normal);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn help_closes_on_any_key_without_acting() {
        let (mut state, _dir) = state("", "");
        press(&mut state, KeyCode::Char('?'));
        assert_eq!(state.mode, Mode::Help);
        press(&mut state, KeyCode::Char('q'));
        assert_eq!(state.mode, Mode::Normal);
        assert!(!state.quit);
    }

    #[test]
    fn palette_edits_start_from_the_saved_colors() {
        let (mut state, dir) = state("", "");
        // Another jotty config changes blue while this one is open.
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("overrides.toml"),
            "[colors]\npalette = [\"#000000\", \"#e06c75\", \"#98c379\", \"#e5c07b\", \"#0000ff\"]",
        )
        .unwrap();
        let red = key(&state, "colors.palette[1] red");
        state
            .set(&red, Some(toml::Value::String("#ff0000".into())))
            .unwrap();
        assert_eq!(state.palette[1], Rgb(0xff, 0, 0));
        assert_eq!(state.palette[4], Rgb(0, 0, 0xff));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn short_palette_overrides_mark_what_they_change() {
        // The list replaces the config file's, so colors past its end fall
        // back to the defaults, not to the config file.
        let base: Vec<String> = (0..16).map(|i| format!("\"#0000{i:02x}\"")).collect();
        let base = format!("[colors]\npalette = [{}]", base.join(", "));
        let (state, dir) = state(&base, "[colors]\npalette = [\"#000000\"]");
        let green = key(&state, "colors.palette[2] green");
        assert_eq!(state.palette[2], DEFAULT_PALETTE[2]);
        assert!(state.override_of(&green).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn conflicting_overrides_show_the_config_file() {
        // However they got there, overrides that don't apply over the
        // config file leave it in effect, with the reason shown.
        let (mut state, dir) = state("[font]\nsize = 10.0", "");
        state.layers.overrides = toml::from_str("[font]\nsize = \"big\"").unwrap();
        state.refresh();
        assert!(state.merge_error.is_some());
        let size = key(&state, "font.size");
        assert_eq!(state.effective_of(&size), Some(toml::Value::Float(10.0)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn leaving_the_keys_clears_the_filter() {
        let (mut state, _dir) = state("", "");
        state.focus = Focus::Keys;
        state.filter = "red".into();
        press(&mut state, KeyCode::Char('h'));
        assert_eq!(state.focus, Focus::Sections);
        assert!(state.filter.is_empty());
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
