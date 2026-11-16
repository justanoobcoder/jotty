# Configuration

jotty reads `~/.config/jotty/config.toml` (or `$XDG_CONFIG_HOME/jotty/config.toml`),
or the file given with `jotty --config PATH`. The file is optional: every
option has a default, so put in it only what you want to change.

[`config.example.toml`](config.example.toml) lists every option with its
default value and what it does. This page explains how they fit together,
with examples. For how to use jotty's features day to day, see
[Usage](usage.md).

With home-manager, `programs.jotty.settings` writes this file from Nix
(see [Install](../README.md#install)).

**Reloading.** Saving the file applies it to every open window, with no
restart. Ctrl+Shift+R and `pkill -USR1 jotty` reload it too. A few options
only apply to windows opened afterwards: `terminal.shell`, `terminal.term`,
`terminal.scrollback`, `window.renderer`, `window.columns`, `window.lines`,
`window.startup_mode`, `window.title` and `window.app_id`.

**Mistakes.** A misspelled option or an invalid value is an error. jotty
logs it and keeps the settings it had (or starts with the defaults), so a
typo never leaves you without a terminal. Only a file given with
`--config` stops jotty from starting.

**Contents**

- [Overrides and `jotty config`](#overrides-and-jotty-config)
- [`[font]`](#font)
- [`[window]`](#window)
- [`[terminal]`](#terminal)
- [`[colors]`](#colors)
- [`[keybinds]`](#keybinds)
- [`[url]`](#url)
- [`[hints]`](#hints)
- [`[files]`](#files)
- [Shell integration](#shell-integration)

## Overrides and `jotty config`

`jotty config` opens a TUI that lists every option with its current value
and lets you change it, with each change applied at once. It never touches
your config file: its changes go to `~/.local/state/jotty/overrides.toml`,
which takes precedence over the config file. That makes it handy with
home-manager, whose config file is read-only: try things in the TUI, then
move them into Nix.

In the TUI, `Enter` changes the selected option, `r` removes its override,
`/` filters, `?` lists every key and `q` quits. Overridden options are
marked `●`.

| Command | Does |
| --- | --- |
| `jotty config` | Change options in the TUI |
| `jotty config export` | Print the overrides as TOML |
| `jotty config export --merged` | Print the config file and the overrides merged into one |
| `jotty config export --nix` | Print the overrides as Nix, for `programs.jotty.settings` |
| `jotty config reset` | Remove all overrides |

To move your overrides into home-manager, paste the output of
`jotty config export --nix` into `programs.jotty.settings`, run
`home-manager switch`, then `jotty config reset`.

## `[font]`

```toml
[font]
family = "JetBrains Mono"   # any name from `fc-list : family`
size = 10.5                 # in points
fallback = ["Noto Sans CJK JP", "Symbols Nerd Font Mono"]
```

Characters the font lacks are taken from the `fallback` fonts, in order,
then from any installed font that has them. Ctrl+= and Ctrl+- change the
size by `size_step` points; Ctrl+0 goes back to `size`.

Box-drawing characters, block elements and braille are drawn by jotty
itself, so they line up whatever the font. Color emoji are drawn from your
emoji font.

## `[window]`

```toml
[window]
padding = 8                 # space around the text
opacity = 0.9
blur = true
startup_mode = "maximized"  # or "windowed", "fullscreen"
```

### Transparency

`opacity` makes the background see-through; text stays opaque. Colored
backgrounds that programs paint (vim and tmux themes, status bars) stay
opaque unless you also set `transparent_cell_backgrounds = true`. Without
it, a full-screen program whose theme paints every cell looks opaque.

`blur = true` blurs what is behind the window, which keeps text readable
over a busy wallpaper. It works on compositors that support it (the
`ext-background-effect-v1` protocol); elsewhere it does nothing.

### Renderer

`renderer = "cpu"` (the default) draws in software and uses little memory,
about 30 MiB. `renderer = "gpu"` draws with OpenGL, which is cheaper when
a large window redraws all the time, but the graphics driver it loads
brings jotty to 130 MiB or more. jotty falls back to the CPU when OpenGL
isn't available.

### Other window options

- `notifications`, `command_notify_after`: desktop notifications from
  programs and for long commands. See [Notifications](usage.md#notifications).
- `single_instance`: open new windows in the jotty already running. See
  [Single instance](usage.md#single-instance).
- `bell_urgent`: mark the window urgent when a program rings the bell while
  you're elsewhere.
- `copy_notice`: say "Copied 3 lines" in the corner after copying.
- `hide_pointer_while_typing`: hide the mouse pointer until it moves.

## `[terminal]`

```toml
[terminal]
shell = "/usr/bin/fish"     # default: $SHELL
scrollback = 50000          # lines of history
cursor_shape = "beam"       # "block", "beam" or "underline"
cursor_blink = true
```

Programs such as Neovim can change the cursor shape while they run.

**The editor.** `open_last_output` and `output_click = "open"` show a
command's output in an editor, in a new window. jotty uses `editor`, or
else `$VISUAL`, `$EDITOR`, then `less`. Pick an editor that runs in a
terminal and stays open until you close it:

```toml
[terminal]
editor = ["nvim", "-R"]
```

Set it in the config if you start jotty from an app launcher, which doesn't
see the variables your shell sets.

### `TERM` and terminfo

jotty sets `TERM=jotty` when its terminfo entry is installed (the packages
install it), and `xterm-256color` otherwise. The entry tells programs about
features such as curly and colored underlines.

Hosts you `ssh` into don't know `jotty`. Copy the entry to a host once:

```sh
infocmp -x jotty | ssh host -- tic -x -
```

or set `term = "xterm-256color"`, which works everywhere.

### Clipboard from programs (OSC 52)

Programs can copy to your clipboard with the OSC 52 escape sequence, even
over SSH or inside tmux and zellij: Neovim's `"+y`, for example. Programs
can't read the clipboard this way. Set `osc52 = false` to refuse copies
from programs as well.

## `[colors]`

```toml
[colors]
foreground = "#839496"
background = "#002b36"
cursor = "#93a1a1"
# The 16 ANSI colors: black, red, green, yellow, blue, magenta, cyan,
# white, then their bright variants.
palette = [
    "#073642", "#dc322f", "#859900", "#b58900",
    "#268bd2", "#d33682", "#2aa198", "#eee8d5",
    "#002b36", "#cb4b16", "#586e75", "#657b83",
    "#839496", "#6c71c4", "#93a1a1", "#fdf6e3",
]
```

Unset colors have sensible defaults: the cursor takes the text color, and
selected text has its colors swapped. Programs may change colors while they
run; `reset` brings yours back.

## `[keybinds]`

Each action takes a list of key combos. Listing an action replaces its
defaults, and an empty list unbinds it so the keys reach the program. The
[keybinds table](../README.md#keybinds) shows the defaults, and
[`config.example.toml`](config.example.toml) every action.

A combo is modifiers (`ctrl`, `alt`, `shift`, `super`) and a key joined by
`+`: a character (`c`, `0`), a key name (`page_up`, `insert`, `return`,
`F5`, `plus`) or a wheel direction (`wheel_up`, `wheel_down`).

```toml
[keybinds]
# Ctrl+C copies when text is selected, and interrupts the program otherwise.
copy_or_interrupt = ["ctrl+c"]
# Copy and paste with Ctrl+Insert / Shift+Insert, and leave Ctrl+Shift+V
# to programs.
copy = ["ctrl+insert"]
paste = ["shift+insert"]
# Scroll by a line with Shift+Up/Down, by a page with Shift and the wheel.
scroll_line_up = ["shift+up"]
scroll_line_down = ["shift+down"]
scroll_page_up = ["shift+page_up", "shift+wheel_up"]
scroll_page_down = ["shift+page_down", "shift+wheel_down"]
# Open a new window in the same directory with Shift+Enter.
new_window = ["shift+return"]
# Copy or open the last command's output (needs shell integration).
copy_last_output = ["ctrl+shift+o"]
open_last_output = ["ctrl+shift+u"]
```

## `[url]`

Ctrl+click opens links with `launcher` (`xdg-open` by default). `regex`
turns more text into links, using [Rust regex syntax](https://docs.rs/regex);
the launcher gets the matched text. For example, to open issue keys such as
`JOT-123` in a tracker:

```toml
[url]
launcher = ["sh", "-c", 'case "$0" in JOT-*) xdg-open "https://tracker.example/$0";; *) xdg-open "$0";; esac']
regex = ['\bJOT-\d+\b']
```

## `[hints]`

[Hints](usage.md#hints) (Ctrl+Shift+E) label links, paths, hashes and
addresses on the screen. `regex` labels more kinds of text, which you can
then copy or insert:

```toml
[hints]
regex = ['\b[A-Z]+-\d+\b']   # issue keys, such as JOT-123
```

## `[files]`

Names of existing files in the output are links too: Ctrl+click
`cat.png` in `ls` output to view the image. Files come in kinds, by
extension: `image`, `video`, `audio` and `document` are built in. Each kind
has a `launcher`; empty means `url.launcher`.

```toml
# Play videos in mpv and open documents in zathura.
[files.video]
launcher = ["mpv"]

[files.document]
launcher = ["zathura"]

# Add a kind of your own.
[files.sheet]
extensions = ["csv", "tsv"]
launcher = ["libreoffice", "--calc"]
```

A built-in kind keeps its extensions unless you set them;
`extensions = []` turns it off. A kind you add wins over the built-in ones
for an extension they share.

## Shell integration

A few features need your shell to tell jotty where its prompt and each
command's output are:

- scrolling from prompt to prompt (Ctrl+Shift+PageUp/PageDown),
- selecting a command's whole output with Ctrl+double click,
- copying or opening the last command's output,
- a [notification](usage.md#notifications) when a long command finishes,
- better input-method support on the command line.

**fish** 4.0 and later does this by itself. For **bash**, add to
`~/.bashrc`, after `PS1` is set:

```bash
PS1='\[\e]133;D;$?\a\e]133;A\a\]'"$PS1"'\[\e]133;B\a\]'
PS0='\e]133;C\a'
```

For **zsh**, add to `~/.zshrc`, after the prompt is set up:

```zsh
autoload -Uz add-zsh-hook
_jotty_prompt_start() { print -n "\e]133;D;$?\a\e]133;A\a" }
add-zsh-hook precmd _jotty_prompt_start
_jotty_output_start() { print -n '\e]133;C\a' }
add-zsh-hook preexec _jotty_output_start
PS1="$PS1"$'%{\e]133;B\a%}'
```

For **fish** before 4.0, add to `~/.config/fish/config.fish`:

```fish
functions -c fish_prompt __jotty_fish_prompt
function fish_prompt
    printf '\e]133;D;%d\a\e]133;A\a' $status
    __jotty_fish_prompt
    printf '\e]133;B\a'
end
function __jotty_output_start --on-event fish_preexec
    printf '\e]133;C\a'
end
```

These features don't work inside zellij or tmux, which don't pass the
marks on to jotty.
