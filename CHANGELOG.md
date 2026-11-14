CHANGELOG
=========

## Unreleased

- Ctrl+click opens audio files and documents (PDF, EPUB, office files),
    with the new built-in `files.audio` and `files.document` kinds
- File kinds added in the config under new names win over the built-in
    kinds for an extension they share

## 2.2.0

- The glyph atlas grows as a screen needs it, instead of being cleared on
    every frame when a screen doesn't fit, and starts smaller
- Ctrl+letter sends the control character on non-Latin keyboard layouts
- `{count}G` goes to that line in vi mode
- Hyperlinks longer than 2 KiB are ignored; empty `shell`, `term` and
    `app_id` mean the default; an invalid `url.regex` is a config error
- Fixed held keys repeating one-shot actions (a new window per repeat)
- Fixed a bell per byte flooding the compositor with attention requests
- Fixed input-method deletions corrupting the search query
- Fixed images of the alternate screen staying in memory after leaving it
- Fixed long stalls on output with many distinct underline colors, links
    or clusters once their tables were full
- Fixed the vte parser taking quadratic time on invalid UTF-8
- Fixed notification bodies escaped for daemons that don't read markup

## 2.1.0

- Display sixel images
- Add `--single-instance` and `window.single_instance`: new windows open in
    the jotty already running, with the directory and environment of the
    command that asked for them
- Ctrl+Shift+N opens the new window in the same process
- Draw an Adwaita-style title bar, with the window title and buttons, on
    compositors without server-side decorations (GNOME)

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
