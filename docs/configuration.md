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
highlights) stay opaque unless `transparent_cell_backgrounds = true`, even
when the color they set equals `colors.background`. To have Neovim show
the transparency without that option, let its theme leave the background
unset (most themes have a `transparent` setting). That
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
| `term` | string | `"auto"` | `$TERM` for the child. `"auto"` means `jotty` when its terminfo entry is installed, otherwise `xterm-256color`. |
| `scrollback` | integer | `10000` | Lines kept in history. `0` disables scrollback. |
| `bold_is_bright` | bool | `true` | Bold text in colors 0–7 uses colors 8–15. |

### `TERM` and terminfo

jotty ships a terminfo entry, [`extra/jotty.terminfo`](../extra/jotty.terminfo):
`xterm-256color` plus styled and colored underlines (`Smulx`, `Setulc`,
`Su`), `Tc`, and cursor shape and color. With it, programs know what jotty
supports without having to ask at startup. That matters behind anything
that relays the terminal: `devenv shell`, for example, swallows the
attribute probe Neovim uses, so Neovim falls back to plain underlines for
diagnostics unless `TERM=jotty`.

With `term = "auto"` (the default), jotty looks for the compiled entry next
to its executable (`<prefix>/share/terminfo`, where the Nix package and the
dev shell put it), then in `$TERMINFO`, `~/.terminfo`, `$TERMINFO_DIRS` and
the system directories. If it finds one, the child gets `TERM=jotty` and
`TERMINFO` pointing at that directory; otherwise `TERM=xterm-256color`.

To install the entry by hand (for example for a `cargo build` outside the
dev shell):

```sh
tic -x -o ~/.terminfo extra/jotty.terminfo
```

**SSH.** Remote hosts don't know `jotty`. Copy the entry over once:

```sh
infocmp -x jotty | ssh host -- tic -x -
```

or set `term = "xterm-256color"`, which works everywhere but makes programs
fall back to probing (or plain underlines).

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

## `[keybinds]`

Each action takes a list of key combos. Listing an action replaces its
default combos; an empty list unbinds it, so those keys reach the program.
Actions you don't list keep their defaults.

| Action | Default | Does |
| --- | --- | --- |
| `copy` | `ctrl+shift+c` | Copy the selection |
| `copy_or_interrupt` | unbound | Copy and clear the selection if there is one; otherwise pass the key to the program |
| `paste` | `ctrl+shift+v`, `shift+insert` | Paste the clipboard |
| `scroll_line_up` / `scroll_line_down` | unbound | Scroll the history by a line |
| `scroll_page_up` / `scroll_page_down` | `shift+page_up` / `shift+page_down` | Scroll the history by a screen |
| `scroll_to_top` / `scroll_to_bottom` | `shift+home` / `shift+end` | Jump to the top or bottom of the history |
| `font_larger` | `ctrl+plus`, `ctrl+equal`, `ctrl+kp_add` | Increase the font size by a point |
| `font_smaller` | `ctrl+minus`, `ctrl+kp_subtract` | Decrease it |
| `font_reset` | `ctrl+0`, `ctrl+kp_0` | Back to `font.size` |

A combo is modifiers and a key joined by `+`:

- Modifiers: `ctrl`, `alt`, `shift`, `super` (also `control`, `meta`,
  `logo`, `mod1`, `mod4`). They must match exactly; Caps Lock and Num Lock
  are ignored.
- Key: a single character (`c`, `0`, `ä`) or an xkb keysym name, matched
  case-insensitively (`page_up`, `insert`, `F5`, `plus`, `kp_add`,
  `return`). `ctrl++` binds the plus key.
- A combo matches the key's symbol without modifiers (`ctrl+shift+equal`)
  or the symbol it produces with them, minus the Shift that produced it
  (`ctrl+plus` on layouts where `+` is Shift+`=`). Letters are
  case-insensitive, and `ctrl+c` never matches Ctrl+Shift+C.

Example: copy and paste with Ctrl+Insert / Shift+Insert, free
Ctrl+Shift+V for the program, and scroll by line with Shift+Up/Down:

```toml
[keybinds]
copy = ["ctrl+insert"]
paste = ["shift+insert"]
scroll_line_up = ["shift+up"]
scroll_line_down = ["shift+down"]
```

**Ctrl+C that copies.** Bind `copy_or_interrupt` to `ctrl+c` and Ctrl+C
copies when text is selected, and interrupts the running program (sends
`^C`) when nothing is. Copying clears the selection, so pressing Ctrl+C
again interrupts:

```toml
[keybinds]
copy_or_interrupt = ["ctrl+c"]
```

Unknown actions, modifiers or keys are reported with their line when the
config is loaded. Middle-click paste isn't a key binding and can't be
changed.

When a program enables mouse reporting (Neovim, htop, tmux, mc, ...),
clicks, drags and the wheel go to it. Hold **Shift** to select text,
scroll the history, or paste with the middle button in jotty instead.

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
