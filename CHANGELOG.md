CHANGELOG
=========

## Unreleased

### Features

- Support reverse video (DECSCNM)
- Add `hints.alphabet`, the characters hint labels are made of
- Add `terminal.word_delimiters`, the characters that end a word for
    double-click selection
- Vi mode: `f`, `F`, `t`, `T`, `;` and `,`
- Hints label paths with a line number (`src/main.rs:12:5`) and open them
    in `terminal.editor` at that line
- Blinking text blinks (`terminal.text_blink`)
- Variable fonts use their named instances, so italics that live in the
    same file as the upright style show slanted
- Fonts without a bold or italic face get a made-up one instead of the
    regular face

### Changes

- Hints label hex hashes in uppercase

### Fixes

- Large kitty graphics images no longer freeze every window while they
    decode
- `jotty config` saves through a symlinked `overrides.toml` instead of
    replacing the link
- Backspace erases a whole non-ASCII character, not just its last byte, in
    programs that read a line at a time

### Performance

- Faster output of colored text, links and sixel images
- Faster output of non-ASCII text (accented Latin, CJK, symbols, box
    drawing), and faster drawing of characters not shown before
- Faster output of text with many thousands of different characters on
    the CPU renderer, whose glyph atlas now grows up to 16 MiB
- Scrollback takes less memory, and text scrolls faster
- Large output, such as `cat` of a big file, is up to 20% faster with
    either renderer: the pty is read on a thread of its own, and lines
    scrolling into the scrollback no longer allocate

## 2.3.0

### Features

- Add hints (Ctrl+Shift+E): label links, paths, hashes and IP addresses on
    the screen to copy, open or insert them
- Ctrl+click opens audio files and documents, and opens links on release
- Ctrl+drag files named in the output to other applications; Ctrl+right
    click marks several to drag at once
- Dropping files onto a window types their paths, quoted for the shell
- Programs can write the clipboard with OSC 52 (`terminal.osc52`)
- Notify when a long command finishes in an unfocused window
    (`window.command_notify_after`)
- Show a notice when something is copied (`window.copy_notice`)
- Copy or open a command's output in `terminal.editor`, with
    `terminal.output_click` or the `copy_last_output` and
    `open_last_output` actions
- Add `window.startup_mode`, `window.hide_pointer_while_typing`,
    `terminal.cursor_shape` and `terminal.cursor_blink`; blinking cursor
    styles blink

### Changes

- File kinds from the config win over the built-in ones for an extension
    they share

### Performance

- Windows share fontconfig's configuration, saving about 0.7 MiB each

## 2.2.0

### Features

- Ctrl+letter sends the control character on non-Latin keyboard layouts
- `{count}G` goes to that line in vi mode

### Changes

- Hyperlinks longer than 2 KiB are ignored; empty `shell`, `term` and
    `app_id` mean the default; an invalid `url.regex` is a config error

### Fixes

- Fixed held keys repeating one-shot actions (a new window per repeat)
- Fixed a bell per byte flooding the compositor with attention requests
- Fixed input-method deletions corrupting the search query
- Fixed images of the alternate screen staying in memory after leaving it
- Fixed long stalls on output with many distinct underline colors, links
    or clusters once their tables were full
- Fixed the vte parser taking quadratic time on invalid UTF-8
- Fixed notification bodies escaped for daemons that don't read markup

### Performance

- The glyph atlas grows as a screen needs it, instead of being cleared on
    every frame when a screen doesn't fit, and starts smaller

## 2.1.0

### Features

- Display sixel images
- Add `--single-instance` and `window.single_instance`: new windows open in
    the jotty already running, with the directory and environment of the
    command that asked for them
- Ctrl+Shift+N opens the new window in the same process
- Draw an Adwaita-style title bar, with the window title and buttons, on
    compositors without server-side decorations (GNOME)

## 2.0.0

### Features

- Add `jotty config`, a TUI to try settings live; its changes are saved to
    `$XDG_STATE_HOME/jotty/overrides.toml`, on top of the config file
- Add `jotty config export` (as TOML or `--nix`) and `jotty config reset`
- Show desktop notifications for OSC 9 and OSC 777
- Tell the input method about password prompts

### Changes

- Look for links only near the pointer

### Fixes

- Fixed `gg` ignoring counts, and word motions skipping empty lines in vi mode
- Fixed the renderer redrawing forever when the glyph atlas overflows
- Fixed crashes and hangs on malformed or oversized escape sequences and images
- Fixed several cursor, autowrap, insert and reset bugs
- Fixed legacy key encodings that differed from xterm
- Fixed VS15 being ignored in emoji sequences
- Fixed the config not reloading when given as a bare file name (`-c config.toml`)
- Fixed arguments that aren't UTF-8 being rejected

### Performance

- Faster output, especially for plain ASCII, and smaller cells in memory

## 1.2.0

### Features

- Add a home-manager module that installs jotty and writes its config
- Support the kitty keyboard protocol
- Select a command's output with Ctrl+double click
- Fit Nerd Font icons to their cell, or to two cells when a space follows

## 1.0.0

### Features

- Display images with the kitty graphics protocol
- Bind actions to the mouse wheel
- Add `font.size_step` for changing the font size
- Draw box drawing, braille, block elements and Symbols for Legacy Computing
    procedurally, so they join across cells

### Changes

- Prefer fallback fonts from the primary font's family

### Fixes

- Fixed symbols sticking out of their cell or off the baseline

## 0.1.0

### Features

- First release: Wayland native, CPU and GPU renderers, input methods with
    surrounding text, vi mode, scrollback search, hyperlinks, color emoji,
    reflow, fractional scaling, mouse reporting, OSC 133 shell integration
    and config reload
