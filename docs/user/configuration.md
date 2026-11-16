# Configuration

## The config file

jotty reads `~/.config/jotty/config.toml` (or
`$XDG_CONFIG_HOME/jotty/config.toml`). Use `jotty --config PATH` to read
another file.

The file is optional: every option has a default, so it only needs what
you change. [`config.example.toml`](../config.example.toml) lists every
option with its default value. Start from it:

```sh
mkdir -p ~/.config/jotty
cp /usr/share/doc/jotty/config.example.toml ~/.config/jotty/config.toml
```

then delete the lines you don't change. The `.deb`, `.rpm` and Arch
packages install the example there; otherwise download it from the link
above. With home-manager, use `programs.jotty.settings` instead (see
[Installing](installation.md#home-manager)).

Each page of this guide shows the options of one topic:

| Section | Page |
| --- | --- |
| `[font]`, `[colors]`, the look of `[window]` | [Appearance](appearance.md) |
| `[terminal]` | [Terminal](terminal.md) |
| `[keybinds]` | [Keybinds](keybinds.md) |
| `[url]`, `[files]` | [Links and files](links-and-files.md) |
| `[hints]` | [Hints](hints.md) |
| Notifications in `[window]` | [Notifications](notifications.md) |

## Reloading

Saving the file applies it to every open window, with no restart. These
reload it too:

- Ctrl+Shift+R (`reload_config`)
- `pkill -USR1 jotty`, for example from a theme switcher

A few options only apply to windows opened afterwards:
`terminal.shell`, `terminal.term`, `terminal.scrollback`,
`window.renderer`, `window.columns`, `window.lines`,
`window.startup_mode`, `window.title` and `window.app_id`.
`window.single_instance` applies when jotty next starts.

## Mistakes

A misspelled option or an invalid value is an error:

- When you save a file with an error, jotty logs it and keeps the settings
  it had.
- When jotty starts with an error in the default config file, it uses the
  defaults, so a typo never leaves you without a terminal.
- A file given with `--config` that has an error stops jotty from
  starting.

## Trying options with `jotty config`

`jotty config` opens a TUI that lists every option with its current
value. Changes apply to open windows at once.

It never touches your config file. Its changes go to
`~/.local/state/jotty/overrides.toml`, which takes precedence over the
config file. That makes it handy with home-manager, whose config file is
read-only: try things in the TUI, then move them into Nix.

| Key | Does |
| --- | --- |
| Enter | Change the selected option |
| `r` | Remove its override |
| `/` | Filter options |
| `?` | Show every key |
| `q` | Quit |

Overridden options are marked `●`.

| Command | Does |
| --- | --- |
| `jotty config export` | Print the overrides as TOML |
| `jotty config export --merged` | Print the config file and the overrides merged |
| `jotty config export --nix` | Print the overrides as Nix, for `programs.jotty.settings` |
| `jotty config reset` | Remove all overrides |

To move your overrides into home-manager:

1. Paste the output of `jotty config export --nix` into
   `programs.jotty.settings`.
2. Run `home-manager switch`.
3. Run `jotty config reset`.
