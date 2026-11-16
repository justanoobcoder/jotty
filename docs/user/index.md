# jotty

jotty is a terminal emulator for Wayland. It is fast and light by default,
works well with input methods, and lets you do most things from the
keyboard: vi mode, hints and search.

## Start here

1. [Install jotty](installation.md).
2. Learn the [default keybinds](keybinds.md#default-keybinds).
3. [Set up shell integration](shell-integration.md) for the features that
   need your shell's help.
4. [Create a config file](configuration.md) and change what you like:
   [fonts and colors](appearance.md), [keybinds](keybinds.md), the
   [terminal](terminal.md).

jotty reloads the config when you save it, so changes show up at once.
`jotty config` opens a TUI to try options live.

## Features

- [Mouse and selection](mouse.md): word, line and range selection, the
  primary selection
- [Links and files](links-and-files.md): Ctrl+click links and files, drag
  files out, drop files in
- [Hints](hints.md): pick links, paths and hashes on screen from the
  keyboard
- [Search and vi mode](search-and-vi-mode.md): find text and select it
  without the mouse
- [Shell integration](shell-integration.md): jump between prompts, select
  or open a command's output
- [Notifications](notifications.md): from programs, and when long commands
  finish
- [Single instance](single-instance.md): open new windows in the jotty
  already running
- Transparency, blur, and a [CPU or GPU renderer](appearance.md#renderer)

[Known issues](known-issues.md) lists limitations and missing features.
