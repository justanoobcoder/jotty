# Configuration

jotty reads `$XDG_CONFIG_HOME/jotty/config.toml`, which falls back to
`~/.config/jotty/config.toml`. Use `jotty --config PATH` to read another file.

Every option has a default, so the file is optional and may contain only what
you change. Unknown keys and invalid values are errors:

- For a file given with `--config`, jotty refuses to start.
- For the default file, jotty logs the error and starts with the defaults, so
  a typo never leaves you without a terminal.

The config is read once at startup.

[`config.example.toml`](config.example.toml) lists every option with its
default value. A test checks that it matches the built-in defaults.

## `[font]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `family` | string | `"monospace"` | A fontconfig family name (`fc-list : family`) or a generic name. |
| `size` | float | `11.0` | Size in points at 96 DPI. It is multiplied by the output scale, and clamped to 4–200. |
| `fallback` | list of strings | `[]` | Families tried, in order, for characters the main font lacks. |

jotty asks fontconfig for the regular, bold, italic and bold-italic faces of
`family`. If a character is missing, it tries the regular face, then each
`fallback` family, then any installed font that has the character. Color
(bitmap emoji) fonts are skipped because jotty can't draw them; see
[known issues](known-issues.md).

Box-drawing characters (`─│┌┐└┘├┤┬┴┼` and their heavy and rounded forms) and
block elements (`█▀▄▌▐░▒▓▁…▇▏…▉`) are drawn by jotty itself. They fill the
cell exactly, whatever the font.

Example:

```toml
[font]
family = "JetBrains Mono"
size = 10.5
fallback = ["Noto Sans CJK JP", "Symbols Nerd Font Mono"]
```

## `[window]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `title` | string | `"jotty"` | Initial title. Programs can change it with OSC 0/2. |
| `app_id` | string | `"jotty"` | Wayland application ID, for compositor window rules. |
| `padding` | integer | `4` | Space around the grid, in logical pixels. |
| `opacity` | float | `1.0` | Background opacity, 0.0–1.0. Text, the cursor and the selection stay opaque. |
| `transparent_cell_backgrounds` | bool | `false` | Also apply `opacity` to cells with an explicit background color. |
| `blur` | bool | `false` | Blur what is behind the window, if the compositor supports `ext-background-effect-v1`. |
| `columns` | integer | `80` | Initial width in cells. Tiling compositors ignore it. |
| `lines` | integer | `24` | Initial height in cells. |

### Transparency

`opacity` makes the default background translucent. Cells whose background
an application set explicitly (status bars, vim or tmux themes, `ls`
highlights) stay opaque unless `transparent_cell_backgrounds = true`. That
option matters for full-screen programs whose theme paints every cell, since
otherwise the window looks completely opaque while they run.

```toml
[window]
opacity = 0.85
transparent_cell_backgrounds = true
blur = true
```

`blur` asks the compositor to blur what is behind the window, which keeps
text readable over busy wallpapers. It needs the `ext-background-effect-v1`
protocol; without it, jotty logs a warning and stays unblurred. When
`opacity` is 1.0, jotty marks the window opaque so the compositor can skip
drawing what is behind it.

## `[terminal]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `shell` | string | unset | Program to run. Unset means `$SHELL`, then `/bin/sh`. `-e` overrides it. |
| `term` | string | `"xterm-256color"` | `$TERM` for the child. |
| `scrollback` | integer | `10000` | Lines kept in history. `0` disables scrollback. |
| `bold_is_bright` | bool | `true` | Bold text in colors 0–7 uses colors 8–15. |

jotty does not ship its own terminfo entry. It implements the parts of
`xterm-256color` that matter in practice (see
[known issues](known-issues.md) for the gaps).

The child also gets `COLORTERM=truecolor`, `TERM_PROGRAM=jotty` and
`TERM_PROGRAM_VERSION`.

## `[colors]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `foreground` | `"#rrggbb"` | `"#dcdcdc"` | Default text color. |
| `background` | `"#rrggbb"` | `"#181818"` | Default background color. |
| `cursor` | `"#rrggbb"` | the foreground | Cursor color. Programs can change it at runtime (see below). |
| `cursor_text` | `"#rrggbb"` | the cell's background | Color of the character under a block cursor. |
| `selection_foreground` | `"#rrggbb"` | inverse | Text color of selected cells. Unset means the cell's background color. |
| `selection_background` | `"#rrggbb"` | inverse | Background of selected cells. Unset means the cell's text color. |
| `palette` | list of `"#rrggbb"` | built-in | The 16 ANSI colors in order: black, red, green, yellow, blue, magenta, cyan, white, then the bright variants. A shorter list overrides only the first entries. |

Programs can override the cursor color while they run with OSC 12
(`printf '\e]12;#ff8800\a'`, also `rgb:ff/88/00`), query it with
`OSC 12 ; ?`, and restore the configured color with OSC 112. Neovim and some
shell vi modes use this to show the current mode. A full reset (`reset`,
`tput reset`) also restores it.

To match foot's `cursor=<text> <cursor>`, set both colors:

```toml
[colors]
cursor = "#ebdbb2"       # foot's second value
cursor_text = "#282828"  # foot's first value
```

Colors 16–255 are the standard xterm 6×6×6 cube and 24-step grayscale ramp.

Example with the Solarized Dark palette:

```toml
[colors]
foreground = "#839496"
background = "#002b36"
palette = [
    "#073642", "#dc322f", "#859900", "#b58900",
    "#268bd2", "#d33682", "#2aa198", "#eee8d5",
    "#002b36", "#cb4b16", "#586e75", "#657b83",
    "#839496", "#6c71c4", "#93a1a1", "#fdf6e3",
]
```

## Key bindings

Key bindings are fixed:

| Keys | Action |
| --- | --- |
| Ctrl+Shift+C | Copy the selection |
| Ctrl+Shift+V, Shift+Insert, middle click | Paste the clipboard |
| Shift+PageUp / Shift+PageDown | Scroll history by a screen |
| Shift+Home / Shift+End | Top / bottom of the history |
| Ctrl+= (or Ctrl++), Ctrl+-, Ctrl+0 | Font larger, smaller, reset |

Any other key goes to the program. Typing, pasting or committing input-method
text scrolls back to the bottom.

## Shell integration

Input methods ask the application for the text around the cursor, so they can
correct or replace what was typed. A terminal doesn't really know which text
is being edited, so jotty reports the cursor's row. If the shell marks the end
of its prompt with the escape sequence `OSC 133 ; B`, jotty reports only what
comes after the prompt, which is your command line.

This is optional. Without it, the surrounding text also includes the prompt.
That still works with most input methods, since they only look at the last
few characters.

**bash** (`~/.bashrc`, after `PS1` is set):

```bash
PS1='\[\e]133;A\a\]'"$PS1"'\[\e]133;B\a\]'
```

**zsh** (`~/.zshrc`, after the prompt is set up):

```zsh
autoload -Uz add-zsh-hook
_jotty_prompt_start() { print -n '\e]133;A\a' }
add-zsh-hook precmd _jotty_prompt_start
PS1="$PS1"$'%{\e]133;B\a%}'
```

**fish** emits these markers itself in recent versions. If yours doesn't,
add this to `~/.config/fish/config.fish`:

```fish
functions -c fish_prompt __jotty_fish_prompt
function fish_prompt
    printf '\e]133;A\a'
    __jotty_fish_prompt
    printf '\e]133;B\a'
end
```

## Logging

jotty logs to stderr through `env_logger`. The default level is `warn`.

```sh
RUST_LOG=info jotty          # fonts loaded, GL version, scale changes
RUST_LOG=jotty=debug jotty   # also escape sequences jotty doesn't handle
```
