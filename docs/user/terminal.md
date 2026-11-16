# Terminal

Configure the shell jotty runs, the scrollback and the cursor.

```toml
[terminal]
shell = "/usr/bin/fish"
scrollback = 50000
cursor_shape = "beam"
cursor_blink = true
```

| Key | Default | Description |
| --- | --- | --- |
| `shell` | `$SHELL` | Program to run. `jotty -e command` runs another one. |
| `term` | `"auto"` | Value of `$TERM`; see [`TERM` and terminfo](#term-and-terminfo). |
| `scrollback` | `10000` | Lines of history. `0` keeps none. |
| `bold_is_bright` | `true` | Draw bold text in the first eight colors with their bright variants. |
| `cursor_shape` | `"block"` | `"block"`, `"beam"` or `"underline"`. |
| `cursor_blink` | `false` | Blink the cursor while the window is focused. |
| `output_click` | `"select"` | What Ctrl+double click on a command's output does; see [Shell integration](shell-integration.md#command-output). |
| `editor` | `[]` | Editor that opens a command's output; see [Shell integration](shell-integration.md#the-editor). |
| `osc52` | `true` | Let programs copy to the clipboard; see [below](#clipboard-from-programs). |

Programs such as Neovim may change the cursor shape while they run.

## `TERM` and terminfo

jotty sets `TERM=jotty` when its terminfo entry is installed, which every
package does, and `xterm-256color` otherwise. The entry tells programs what
jotty supports, such as curly and colored underlines.

Hosts you reach with `ssh` don't know `jotty`. Copy the entry to a host
once:

```sh
infocmp -x jotty | ssh host -- tic -x -
```

or set `term = "xterm-256color"`, which works everywhere.

## Clipboard from programs

Programs can copy to your clipboard with an escape sequence (OSC 52), even
over SSH or inside tmux and zellij. Neovim's `"+y` does this, for example.

- Programs can't read your clipboard this way.
- `osc52 = false` stops programs from copying too, so that a file you
  `cat` can't replace what you copied.
