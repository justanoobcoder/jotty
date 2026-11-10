# Configuration

jotty reads `$XDG_CONFIG_HOME/jotty/config.toml`, which falls back to
`~/.config/jotty/config.toml`. Use `jotty --config PATH` to read another file.

With home-manager, `programs.jotty.settings` writes this file from Nix
(see the [README](../README.md#install)).

Every option has a default, so the file is optional and may contain only what
you change. Unknown keys and invalid values are errors:

- For a file given with `--config`, jotty refuses to start.
- For the default file, jotty logs the error and starts with the defaults, so
  a typo never leaves you without a terminal.

**Reloading.** jotty reloads the config when you save the file (or
[`jotty config`](#overrides-and-jotty-config) changes the overrides), when you
press Ctrl+Shift+R (`reload_config`), and when it receives `SIGUSR1`
(`pkill -USR1 jotty` reloads every window, handy for theme switchers). If
the new file has an error, jotty logs it and keeps the current settings.
Fonts, colors, opacity, blur, padding, key bindings, `[url]`, `[files]`
and the other window and terminal options apply right away. A new
`font.size` replaces any zooming done with Ctrl+=/Ctrl+-. A few options
only apply to new windows, which jotty logs when they change:
`terminal.shell`, `terminal.term`, `terminal.scrollback`,
`window.renderer`, `window.columns`, `window.lines`, `window.title` and
`window.app_id`.

[`config.example.toml`](config.example.toml) lists every option with its
default value. A test checks that it matches the built-in defaults.

## Overrides and `jotty config`

`jotty config` opens a TUI that lists every option with its current value
and lets you change it. It never writes your config file. Its changes go to
`$XDG_STATE_HOME/jotty/overrides.toml` instead, which defaults to
`~/.local/state/jotty/overrides.toml`. This is meant for configs managed by
home-manager, which are read-only and slow to change: try things in the TUI,
then move them into Nix once they are settled.

- **Precedence.** The overrides are merged over the config file key by key.
  A key set in the overrides wins, and the rest come from the config file
  or the defaults. Lists are replaced as a whole, not appended to. With
  `--config PATH`, the overrides apply over that file too.
- **Live.** Each change is checked and then saved at once, and running
  windows reload it, like a change to the config file. jotty creates
  `~/.local/state/jotty/` at startup so that it can watch the file.
- **Errors.** The TUI refuses values the config would refuse. If the
  overrides file is broken anyway, for example after a hand edit, jotty logs
  the error and ignores the overrides. The config file still applies.
  `jotty config` won't open such a file, so that it can't overwrite it.

In the TUI, `Enter` toggles a bool, cycles a choice, or edits anything else.
`Space` only toggles or cycles, and `i` edits any value as text. `?` shows
every key.
Lists are typed comma-separated, or as a TOML array (`["a,b", "c"]`) when an
item contains a comma. `r` removes the override of a key, `R` removes those
of a whole section, `/` filters keys across sections, and `q` quits. An
overridden key is marked `●`. The palette is listed as its 16 colors by name
(`palette[9]  bright red`). Changing one saves the whole list, which the TUI
drops again once it matches the config file. For optional keys such as `colors.cursor`,
saving an empty value removes the override.

Without the TUI:

| Command | Prints |
| --- | --- |
| `jotty config export` | The overrides, as TOML |
| `jotty config export --merged` | The config file merged with the overrides, as one TOML file |
| `jotty config export --nix` | The overrides as a Nix attrset for `programs.jotty.settings` |
| `jotty config export --merged --nix` | Both merged, as Nix |
| `jotty config reset` | Nothing; removes all overrides |

`--merged` only has the keys set in either file, not every default, and it
drops the config file's comments. To move overrides into home-manager, paste
`jotty config export --nix` into `programs.jotty.settings` (or `--merged --nix`
to replace it), run `home-manager switch`, then `jotty config reset`.

## `[font]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `family` | string | `"monospace"` | A fontconfig family name (`fc-list : family`) or a generic name. |
| `size` | float | `11.0` | Size in points at 96 DPI. It is multiplied by the output scale, and clamped to 4–200. |
| `size_step` | float | `1.0` | Points that `font_larger` and `font_smaller` add or remove, per key press or wheel notch. Clamped to 0.1–10. |
| `fallback` | list of strings | `[]` | Families tried, in order, for characters the main font lacks. |

jotty asks fontconfig for the regular, bold, italic and bold-italic faces of
`family`. If a character is missing, it tries the regular face, then each
`fallback` family, then a font fontconfig considers like `family` (DejaVu
Sans Mono rather than DejaVu Sans for a monospace font), then any installed
font that has the character. Symbols that stick out of the cell by more
than a pixel, such as ♥ in some fonts, are shrunk to fit. Color
(bitmap emoji) fonts are skipped because jotty can't draw them; see
[known issues](known-issues.md).

Box-drawing characters (`─│┌┐└┘├┤┬┴┼` with their heavy, double, dashed,
rounded and diagonal forms), block elements (`█▀▄▌▐░▒▓▁…▇▏…▉▖…▟`), braille
(`⣿`), the black triangles `◢◣◤◥` and the Symbols for Legacy Computing
(sextants, wedges, checker boards, and the circle and diagonal pieces) are
drawn by jotty itself. They fill the cell exactly, whatever the font, so
they join across cells.

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
| `renderer` | string | `"cpu"` | `"cpu"` draws in software; `"gpu"` draws with OpenGL ES. See [Renderer](#renderer). |
| `bell_urgent` | bool | `true` | When the bell rings while the window isn't focused, ask the compositor to mark it urgent (`xdg-activation-v1`). |
| `notifications` | bool | `true` | Show the desktop notifications programs ask for (OSC 9, OSC 777) while the window isn't focused. See [Notifications](#notifications). |
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

### Notifications

Programs can ask for a desktop notification with either of these escape
sequences:

| Sequence | Origin | Notification |
| --- | --- | --- |
| `OSC 9 ; message ST` | iTerm2 | the window title as the summary, the message as the body |
| `OSC 777 ; notify ; title ; body ST` | rxvt-unicode | the title and the body |

jotty shows them only while its window isn't focused, through the
notification daemon (`org.freedesktop.Notifications` on the session bus).
Clicking a notification focuses the window again, when the daemon sends an
activation token with the click (version 1.2 of the notification
specification); otherwise the window is only marked urgent. `OSC 9`
followed by a number (`OSC 9 ; 4 ; …`, ConEmu's progress bar and similar)
is not a notification and is ignored.

A program can show 4 notifications at once, then one every 2 seconds; more are
dropped.

For example, to be told when a long command finishes:

```sh
make; printf '\e]777;notify;make;finished with status %d\a' $?
```

In Claude Code, choose the `ghostty` notification channel (`/config`, then
*Notifications*), which sends `OSC 777`; `iterm2` sends `OSC 9`.

### Renderer

`renderer = "cpu"` (the default) draws into shared-memory buffers on the
CPU, as foot does. No GPU driver is loaded, so jotty uses about 20 MiB. It
repaints only the rows that changed and tells the compositor which ones, so
typing and most output cost little. Output that scrolls the whole screen
repaints all of it, so on a large high-DPI window, constant fast scrolling
uses more CPU than the GPU renderer.

`renderer = "gpu"` draws with OpenGL ES 3 through EGL: each frame is one
draw call, whatever the window size. Loading OpenGL loads the GPU driver
into the process, which costs about 100 MiB of memory with the NVIDIA
driver, plus as much again for Mesa if both are installed (EGL loads every
installed driver before picking one).

If the GPU renderer can't start (no EGL or OpenGL ES 3), jotty logs a
warning and uses the CPU renderer.

To keep the GPU renderer but skip Mesa on an NVIDIA system, start jotty with
`__EGL_VENDOR_LIBRARY_FILENAMES=/run/opengl-driver/share/glvnd/egl_vendor.d/10_nvidia.json`
(NixOS path).

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

Programs can override colors while they run, and query them:

| Sequence | Color | Reset with |
| --- | --- | --- |
| OSC 4 ; *n* ; *color* | palette entry *n* (0–255) | OSC 104 (all, or listed entries) |
| OSC 10 ; *color* | default foreground | OSC 110 |
| OSC 11 ; *color* | default background (also the padding) | OSC 111 |
| OSC 12 ; *color* | cursor | OSC 112 |

*color* is `#rrggbb`, `#rgb` or `rgb:rr/gg/bb`; `?` instead queries the
current value, as Neovim does with OSC 11 to pick `background=dark` or
`light`. For example, `printf '\e]12;#ff8800\a'` sets the cursor color,
as Neovim and some shell vi modes do to show the current mode. A full
reset (`reset`, `tput reset`) restores the configured colors.

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
| `font_larger` | `ctrl+plus`, `ctrl+equal`, `ctrl+kp_add`, `ctrl+wheel_up` | Increase the font size by `font.size_step` |
| `font_smaller` | `ctrl+minus`, `ctrl+kp_subtract`, `ctrl+wheel_down` | Decrease it |
| `font_reset` | `ctrl+0`, `ctrl+kp_0` | Back to `font.size` |
| `search` | `ctrl+shift+f` | Search the scrollback (see [Searching](#searching)) |
| `reload_config` | `ctrl+shift+r` | Read the config file again |
| `vi_mode` | `ctrl+shift+space` | Turn vi mode on or off (see [Vi mode](#vi-mode)) |
| `new_window` | `ctrl+shift+n` | Open another jotty in the current directory of the shell (or whatever runs in the terminal), with the same `--config` file |

A combo is modifiers and a key joined by `+`:

- Modifiers: `ctrl`, `alt`, `shift`, `super` (also `control`, `meta`,
  `logo`, `mod1`, `mod4`). They must match exactly; Caps Lock and Num Lock
  are ignored.
- Key: a single character (`c`, `0`, `ä`) or an xkb keysym name, matched
  case-insensitively (`page_up`, `insert`, `F5`, `plus`, `kp_add`,
  `return`). `ctrl++` binds the plus key.
- Or a direction of the mouse wheel: `wheel_up`, `wheel_down`,
  `wheel_left`, `wheel_right`. The action runs once per notch (touchpads:
  once per line's worth of scrolling). A bound wheel combo takes
  precedence over scrolling and over programs that read the mouse; with
  other modifiers, or none, the wheel scrolls or goes to the program as
  usual.
- A combo matches the key's symbol without modifiers (`ctrl+shift+equal`)
  or the symbol it produces with them, minus the Shift that produced it
  (`ctrl+plus` on layouts where `+` is Shift+`=`). Letters are
  case-insensitive, and `ctrl+c` never matches Ctrl+Shift+C.

Example: copy and paste with Ctrl+Insert / Shift+Insert, free
Ctrl+Shift+V for the program, scroll by line with Shift+Up/Down, and by a
screen with Shift and the wheel:

```toml
[keybinds]
copy = ["ctrl+insert"]
paste = ["shift+insert"]
scroll_line_up = ["shift+up"]
scroll_line_down = ["shift+down"]
scroll_page_up = ["shift+page_up", "shift+wheel_up"]
scroll_page_down = ["shift+page_down", "shift+wheel_down"]
```

**A new window in the same directory.** `new_window` starts another jotty
where you are: in the current directory of the program running in the
terminal, usually your shell after its last `cd`. It is bound to
Ctrl+Shift+N; to use Shift+Enter instead (which then no longer reaches
programs that use it, such as some chat CLIs and Neovim mappings):

```toml
[keybinds]
new_window = ["shift+return"]
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

### Searching

The `search` binding (Ctrl+Shift+F) opens a search bar over the bottom row.
Type to search the scrollback and the screen, from the bottom up; the
match is selected and scrolled into view. The search ignores case unless
the query has an uppercase letter, and finds text that wraps across rows.

| Key | Does |
| --- | --- |
| Up, Ctrl+R | Previous (older) match |
| Down, Ctrl+S | Next (newer) match |
| Backspace | Delete the last character of the query |
| Enter | Close the bar and keep the match selected (and as the primary selection) |
| Escape | Close the bar and clear the selection |

### Vi mode

`vi_mode` (Ctrl+Shift+Space) puts a block cursor on the screen, at the
terminal's cursor, to move around the screen and scrollback with vim keys,
select text and yank it to the clipboard. A label in the top-right corner
shows the mode (`VI`, `VISUAL`, `V-LINE`, `V-BLOCK`). Keys don't reach the
program while vi mode is on; output keeps arriving, and the cursor stays on
its text as it scrolls.

| Keys | Do |
| --- | --- |
| `h` `j` `k` `l`, arrows | Move by a character or line; a count repeats (`5j`) |
| `w` `b` `e`, `W` `B` `E` | Next word, previous word, end of word (`W`: words separated only by blanks) |
| `0` `^` `$` | Start of the line, first non-blank, last non-blank |
| `gg` `G` | Top of the scrollback, bottom of the screen |
| `H` `M` `L` | Top, middle, bottom of the view |
| Ctrl+U / Ctrl+D, Ctrl+B / Ctrl+F | Half a page, a page up or down |
| `v` `V` Ctrl+V | Visual mode by character, line, or block; the same key again ends it |
| `o` | In visual mode: go to the other end of the selection |
| `y` | Yank the selection to the clipboard; `yy` (`3yy`) yanks lines |
| `/` `?` | Search down or up from the cursor; Enter jumps to the match |
| `n` `N` | Next match of the last search, in its direction or the other way (wrapping around) |
| Escape | Leave visual mode, then vi mode (also `i`, `I`, `a`, `A`, `q`, Ctrl+C) |

Key bindings still work in vi mode, so Ctrl+Shift+C copies the visual
selection too.

### Mouse

**Double click** selects the word under the pointer. A word is a run of
characters other than whitespace and `` "'`()[]{}<>|,; ``, so paths, URLs
and options such as `--color=auto` are selected whole. Words continue
across soft-wrapped lines. **Triple click** selects the whole line,
including the rows a long line wraps onto.

Keep the button down after a double or triple click and drag to extend the
selection by whole words or whole lines. The clicked word or line always
stays selected.

**Shift+click to select a range.** Shift+click a character to mark it (it
is highlighted), then Shift+click another one: everything from the first
to the second is selected, in either direction, even across a long stretch
of history. A plain click or Escape cancels the mark. Shift+dragging still
selects as usual.

**Selecting a command's output.** Ctrl+double-clicking anywhere in a
command's output selects all of it, from the line after the command to the
line before the next prompt, without trailing blank lines. Copy it as
usual. This needs the shell to mark its prompts and commands (see
[Shell integration](#shell-integration)); on prompts and command lines, a
Ctrl+double click selects a word.

**Primary selection.** Selecting text also makes it the primary
selection, and a middle click pastes the primary selection, as is usual on
Linux. Compositors without `primary-selection-unstable-v1` get the
clipboard instead. The selection stays while output arrives elsewhere,
following its text as it scrolls into the history.

**Links.** Hold Ctrl over a link to underline it, and Ctrl+click to open
it with `url.launcher`. Links are hyperlinks that programs set (OSC 8, as
`ls --hyperlink`, `gcc` and `delta` emit), URLs in the text, which may
wrap across rows, and paths of image and video files; see [`[url]`](#url)
and [`[files]`](#files) to change launchers or add kinds.

When a program enables mouse reporting (Neovim, htop, tmux, mc, ...),
clicks, drags and the wheel go to it. Hold **Shift** to select text,
scroll the history, paste with the middle button, or (with Ctrl) open
links in jotty instead.

Any other key goes to the program. Typing, pasting or committing input-method
text scrolls back to the bottom.

## `[url]`

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `launcher` | list of strings | `["xdg-open"]` | Command that opens URLs, regex matches, and files whose kind has no launcher; the link is added as the last argument. |
| `regex` | list of strings | `[]` | Regular expressions for more kinds of links, besides URLs. |

URLs are recognized by their scheme (`https://`, `file://`, `ssh://`,
`git://`, `mailto:`, ...). Trailing punctuation and unmatched closing
brackets are not part of them, so `(see https://x.org/a).` opens
`https://x.org/a`.

`regex` patterns use [Rust regex syntax](https://docs.rs/regex) and are
matched against the line under the pointer; the matched text goes to the
launcher as is. If a pattern is invalid, jotty logs the error and uses only
the built-in URL pattern. The launcher
gets every kind of link, so for matches that aren't URLs, use a small
script that turns them into one. For example, to open issue numbers:

```toml
[url]
launcher = ["sh", "-c", 'case "$0" in JOT-*) xdg-open "https://tracker.example/$0";; *) xdg-open "$0";; esac']
regex = ['\bJOT-\d+\b']
```

## `[files]`

Paths of files are links too, so Ctrl+clicking `cat.png` in `ls` output
shows the image and `clip.mp4` plays the video. Files come in kinds, each
a table `[files.<name>]` with its own extensions and launcher:

| Key | Type | Description |
| --- | --- | --- |
| `extensions` | list of strings | File name extensions, matched case-insensitively. `[]` turns the kind off. |
| `launcher` | list of strings | Command that opens these files, with the absolute path added as the last argument. Empty (the default) means `url.launcher`. |

Two kinds are built in:

- `image`: png, jpg, jpeg, gif, webp, avif, jxl, heic, heif, bmp, tif,
  tiff, svg, ico
- `video`: mp4, mkv, webm, mov, avi, m4v, wmv, flv, mpg, mpeg, ogv, 3gp

A kind in the config changes only the keys it sets, so this keeps the
default extensions and opens videos in mpv and images in imv, while URLs
still go to `xdg-open`:

```toml
[files.video]
launcher = ["mpv"]

[files.image]
launcher = ["imv"]
```

Add other kinds with new names:

```toml
[files.document]
extensions = ["pdf", "epub"]
launcher = ["zathura"]
```

A path can be absolute, start with `~/`, or be relative to the current
directory of the program running in the terminal (usually the shell).
Names that `ls` quotes because of spaces (`'my clip.mp4'`) work too. Only
files that exist are links. When an extension is in several kinds, the
first kind in alphabetical order wins.

## Shell integration

Shells can mark where their prompt starts (`OSC 133 ; A`), where the prompt
ends and the command line starts (`B`), and where the command's output
starts (`C`). jotty uses these marks for two things: the text it reports to
input methods, and selecting a command's output with a Ctrl+double click.

Input methods ask the application for the text around the cursor, so they can
correct or replace what was typed. A terminal doesn't really know which text
is being edited, so jotty reports the cursor's row. If the shell marks the end
of its prompt with the escape sequence `OSC 133 ; B`, jotty reports only what
comes after the prompt, which is your command line.

This is optional. Without it, the surrounding text also includes the prompt.
That still works with most input methods, since they only look at the last
few characters.

To select a command's output, jotty needs `A`, to know where the output
ends, and `C` (or `B`, if the command fits on one line) to know where it
starts.

**bash** (`~/.bashrc`, after `PS1` is set):

```bash
PS1='\[\e]133;A\a\]'"$PS1"'\[\e]133;B\a\]'
PS0='\e]133;C\a'
```

**zsh** (`~/.zshrc`, after the prompt is set up):

```zsh
autoload -Uz add-zsh-hook
_jotty_prompt_start() { print -n '\e]133;A\a' }
add-zsh-hook precmd _jotty_prompt_start
_jotty_output_start() { print -n '\e]133;C\a' }
add-zsh-hook preexec _jotty_output_start
PS1="$PS1"$'%{\e]133;B\a%}'
```

**fish** emits all of these marks itself since version 4.0. For older
versions, add this to `~/.config/fish/config.fish`:

```fish
functions -c fish_prompt __jotty_fish_prompt
function fish_prompt
    printf '\e]133;A\a'
    __jotty_fish_prompt
    printf '\e]133;B\a'
end
function __jotty_output_start --on-event fish_preexec
    printf '\e]133;C\a'
end
```

## Logging

jotty logs to stderr through `env_logger`. The default level is `warn`.

```sh
RUST_LOG=info jotty          # fonts loaded, GL version, scale changes
RUST_LOG=jotty=debug jotty   # also escape sequences jotty doesn't handle
```
