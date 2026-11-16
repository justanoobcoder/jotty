# Appearance

Configure jotty's font, colors and window.

## Font

```toml
[font]
family = "JetBrains Mono"
size = 10.5
size_step = 1.0
fallback = ["Noto Sans CJK JP", "Symbols Nerd Font Mono"]
```

| Key | Default | Description |
| --- | --- | --- |
| `family` | `"monospace"` | Font family, as `fc-list : family` lists it, or a generic name such as `"monospace"`. |
| `size` | `11.0` | Size in points. |
| `size_step` | `1.0` | Points that Ctrl+= and Ctrl+- add or remove. |
| `fallback` | `[]` | Fonts tried, in order, for characters the main font lacks. |

When no font has a character, jotty takes it from any installed font that
has it. Box-drawing characters, block elements and braille are drawn by
jotty itself, so they line up whatever the font. Emoji are drawn in color
from your emoji font.

Ctrl+= and Ctrl+- change the size of a window's font, and Ctrl+0 goes back
to `size`.

## Colors

```toml
[colors]
foreground = "#839496"
background = "#002b36"
cursor = "#93a1a1"
palette = [
    "#073642", "#dc322f", "#859900", "#b58900",
    "#268bd2", "#d33682", "#2aa198", "#eee8d5",
    "#002b36", "#cb4b16", "#586e75", "#657b83",
    "#839496", "#6c71c4", "#93a1a1", "#fdf6e3",
]
```

Colors use `#rrggbb`.

| Key | Default | Description |
| --- | --- | --- |
| `foreground` | `"#dcdcdc"` | Text. |
| `background` | `"#181818"` | Background. |
| `cursor` | the text color | Cursor. |
| `cursor_text` | the background | Text under a block cursor. |
| `selection_foreground` | swapped colors | Selected text. |
| `selection_background` | swapped colors | Background of selected text. |
| `mark_foreground` | the background | Files marked for [dragging](links-and-files.md#dragging-files-out). |
| `mark_background` | palette blue | Background of marked files. |
| `palette` | built in | The 16 ANSI colors: black, red, green, yellow, blue, magenta, cyan, white, then their bright variants. A shorter list changes only the first ones. |

Programs such as Neovim may change colors while they run; `reset` brings
yours back. `terminal.bold_is_bright` draws bold text in the bright
colors (see [Terminal](terminal.md)).

## Window

```toml
[window]
padding = 8
opacity = 0.9
blur = true
startup_mode = "maximized"
```

| Key | Default | Description |
| --- | --- | --- |
| `padding` | `4` | Space around the text, in pixels. |
| `opacity` | `1.0` | Background opacity, from `0.0` to `1.0`. Text stays opaque. |
| `transparent_cell_backgrounds` | `false` | Make colored backgrounds that programs paint transparent too. |
| `blur` | `false` | Blur what is behind the window. |
| `columns`, `lines` | `80`, `24` | Size of new windows, in characters. |
| `startup_mode` | `"windowed"` | `"windowed"`, `"maximized"` or `"fullscreen"`. |
| `title` | `"jotty"` | Window title, until a program sets one. |
| `app_id` | `"jotty"` | Application ID, for your compositor's window rules. |
| `renderer` | `"cpu"` | `"cpu"` or `"gpu"`; see [Renderer](#renderer). |
| `hide_pointer_while_typing` | `false` | Hide the mouse pointer while you type, until it moves. |

Tiling compositors decide the window's size themselves, and may ignore
`columns`, `lines` and `startup_mode`.

### Transparency

`opacity` makes the background see-through. Colored backgrounds that
programs paint, such as vim and tmux themes or status bars, stay opaque
unless `transparent_cell_backgrounds = true`. Without it, a program whose
theme paints the whole screen looks opaque.

`blur` keeps text readable over a busy wallpaper. It works on compositors
that support the `ext-background-effect-v1` protocol, and does nothing
elsewhere.

### Renderer

| Renderer | Memory | Best for |
| --- | --- | --- |
| `"cpu"` (default) | about 30 MiB | Most use: typing, output, editors |
| `"gpu"` | 130 MiB or more | Large windows that redraw all the time |

The GPU renderer draws with OpenGL; loading the graphics driver is what
costs the memory. jotty falls back to the CPU renderer when OpenGL isn't
available. A change applies to new windows.
