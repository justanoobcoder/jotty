CHANGELOG
=========

## Unreleased

## 2.4.0

### Features

- Programs can switch the whole screen to reverse video, swapping the text and background colors
- Hints (Ctrl+Shift+E) also label file paths followed by a line number, such as `src/main.rs:12:5`, and open them in your editor (`terminal.editor`) at that line
- Hints also label hex hashes written in uppercase, such as `1AF659B`
- New `hints.alphabet` option: choose which letters hint labels are made from, for example only the home row
- New `terminal.word_delimiters` option: choose which characters end a word when you double-click to select it
- Vi mode: jump to a character on the line with `f`, `F`, `t` and `T`, and repeat the jump with `;` and `,`
- Text that programs ask to blink now blinks; turn it off with `terminal.text_blink = false`
- Italic text is now slanted with variable fonts that keep the italic style in the same file as the regular one
- Fonts with no bold or italic style get one made from the regular style, so bold and italic text still stands out
- Better support for complex scripts: Arabic letters join up, Indic letters combine into syllables, and accents sit where the font puts them
- New `font.ligatures` option: draw the font's ligatures, such as `->` or `!=`, as one symbol (off by default)
- Emoji made of several characters (families, skin tones), flags and Indic syllables take two cells, in programs that ask for it

### Fixes

- Showing a large image (kitty graphics) no longer freezes every jotty window while the image loads
- `jotty config` keeps a symlinked `overrides.toml` (for example one from a dotfiles repo) as a link when it saves, instead of replacing it with a plain file
- Backspace deletes a whole accented or non-Latin character in programs that read a line at a time, such as `cat`, instead of leaving part of it behind
- Inserting a wide emoji in the middle of a line no longer overwrites the character after it

### Performance

- Colored text, links and sixel images show up faster
- Accented letters, CJK, symbols and box-drawing characters show up faster, and so do characters drawn for the first time
- Text with thousands of different characters shows up faster with the CPU renderer, whose glyph cache can now grow to 16 MiB
- Large output, such as `cat` of a big file or many short lines, is up to 20% faster with both renderers
- Scrollback uses less memory, and scrolling is faster

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
