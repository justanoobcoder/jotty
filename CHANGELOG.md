CHANGELOG
=========

## Unreleased

## 2.0.0

- Add `jotty config`, a TUI to try settings live; its changes are saved to
    `$XDG_STATE_HOME/jotty/overrides.toml`, on top of the config file
- Add `jotty config export` (as TOML or `--nix`) and `jotty config reset`
- Show desktop notifications for OSC 9 and OSC 777
- Tell the input method about password prompts
- Faster output, especially for plain ASCII, and smaller cells in memory
- Look for links only near the pointer
- Fixed `gg` ignoring counts, and word motions skipping empty lines in vi mode
- Fixed the renderer redrawing forever when the glyph atlas overflows
- Fixed crashes and hangs on malformed or oversized escape sequences and images
- Fixed several cursor, autowrap, insert and reset bugs
- Fixed legacy key encodings that differed from xterm
- Fixed VS15 being ignored in emoji sequences
- Fixed the config not reloading when given as a bare file name (`-c config.toml`)
- Fixed arguments that aren't UTF-8 being rejected

## 1.2.0

- Add a home-manager module that installs jotty and writes its config
- Support the kitty keyboard protocol
- Select a command's output with Ctrl+double click
- Fit Nerd Font icons to their cell, or to two cells when a space follows

## 1.0.0

- Display images with the kitty graphics protocol
- Bind actions to the mouse wheel
- Add `font.size_step` for changing the font size
- Draw box drawing, braille, block elements and Symbols for Legacy Computing
    procedurally, so they join across cells
- Prefer fallback fonts from the primary font's family
- Fixed symbols sticking out of their cell or off the baseline

## 0.1.0

- First release: Wayland native, CPU and GPU renderers, input methods with
    surrounding text, vi mode, scrollback search, hyperlinks, color emoji,
    reflow, fractional scaling, mouse reporting, OSC 133 shell integration
    and config reload
