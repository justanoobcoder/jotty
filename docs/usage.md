# Usage

How to use jotty day to day. The [keybinds table](../README.md#keybinds)
lists the default keys, and [Configuration](configuration.md) shows how to
change them.

**Contents**

- [Selecting and copying](#selecting-and-copying)
- [Links and files](#links-and-files)
- [Dragging files](#dragging-files)
- [Searching](#searching)
- [Vi mode](#vi-mode)
- [Hints](#hints)
- [Command output](#command-output)
- [Notifications](#notifications)
- [Single instance](#single-instance)

## Selecting and copying

- **Drag** to select. **Double click** selects a word (paths, URLs and
  options such as `--color=auto` count as one word), **triple click** a
  whole line. Keep the button down after a double or triple click and drag
  to extend by words or lines.
- **Shift+click** a character to mark it, then Shift+click another one:
  everything between them is selected, even across a long stretch of
  history. A plain click or Escape cancels the mark.
- **Ctrl+Shift+C** copies the selection, **Ctrl+Shift+V** pastes. Selecting
  also sets the primary selection, which a **middle click** pastes.

The selection stays put while output arrives, and follows its text as it
scrolls into the history.

When a program uses the mouse (Neovim, htop, tmux, ...), clicks and the
wheel go to it. Hold **Shift** to select text or scroll the history anyway.

## Links and files

Hold Ctrl over a link to underline it, and **Ctrl+click** to open it. Links
are URLs in the text, hyperlinks that programs print (`ls --hyperlink`,
`gcc`, `delta`), and names of image, video, audio and document files in the
output. Change what opens them in [`[url]`](configuration.md#url) and
[`[files]`](configuration.md#files).

## Dragging files

**Drag files out.** Ctrl+drag the name of a file or directory in the
output, as `ls` prints it, into another application: a file manager, a
browser upload or a chat. Relative names are found in your shell's current
directory.

To drag several at once, **Ctrl+right click** names to mark them, or
**Ctrl+right drag** over output to mark every file named in it. Then
Ctrl+drag one of the marked names. Escape clears the marks.

**Drop files in.** Drop files onto the window to type their paths at the
cursor, quoted for the shell. Nothing runs until you press Enter.

## Searching

**Ctrl+Shift+F** opens a search bar. Type to search the scrollback from the
bottom up; the match is selected. The search ignores case unless you type
an uppercase letter.

| Key | Does |
| --- | --- |
| Up, Ctrl+R | Previous (older) match |
| Down, Ctrl+S | Next (newer) match |
| Enter | Close the bar and keep the match selected |
| Escape | Close the bar |

## Vi mode

**Ctrl+Shift+Space** puts a cursor on the screen that you move with vim
keys, to select and copy text without the mouse. A label in the corner
shows the mode. Keys don't reach the program until you leave.

| Keys | Do |
| --- | --- |
| `h` `j` `k` `l`, arrows | Move; a count repeats (`5j`) |
| `w` `b` `e`, `W` `B` `E` | Next word, previous word, end of word |
| `0` `^` `$` | Start of the line, first non-blank, last non-blank |
| `gg` `G` | Top of the history, bottom of the screen |
| `H` `M` `L` | Top, middle, bottom of the view |
| Ctrl+U / Ctrl+D, Ctrl+B / Ctrl+F | Half a page, a page up or down |
| `v` `V` Ctrl+V | Select by character, line or block |
| `o` | Go to the other end of the selection |
| `y` | Copy the selection; `yy` copies the line |
| `/` `?` `n` `N` | Search down or up, next or previous match |
| Escape | Leave (also `i`, `a`, `q`) |

## Hints

**Ctrl+Shift+E** puts a short label over every link, path, git or Nix hash
and IP address on the screen, and over names of files that exist. Type a
label to pick one, then:

| Key | Does |
| --- | --- |
| Ctrl+C | Copy it |
| Ctrl+O | Open it, as Ctrl+click does |
| Ctrl+I | Type it at the cursor |
| Enter | Open it, or copy it if it can't be opened |
| Escape | Leave |

[`hints.regex`](configuration.md#hints) labels more kinds of text.

## Command output

With [shell integration](configuration.md#shell-integration):

- **Ctrl+double click** a command's output to select all of it. Set
  [`terminal.output_click`](configuration.md#terminal) to `"copy"` to also
  copy it, or to `"open"` to open it in your editor.
- **Ctrl+Shift+PageUp/PageDown** scrolls from prompt to prompt.
- `copy_last_output` and `open_last_output` copy or open what the last
  command printed. They are unbound by default; see
  [`[keybinds]`](configuration.md#keybinds).

## Notifications

While jotty's window isn't focused, it shows the desktop notifications
programs ask for. Clicking one brings you back to the window. To be told
when a command finishes:

```sh
make; printf '\e]777;notify;make;finished with status %d\a' $?
```

With [shell integration](configuration.md#shell-integration), jotty also
tells you when a command that ran for 10 seconds or more finishes while
you're elsewhere. Change the time with `window.command_notify_after`, or
set it to `0` to turn this off. `window.notifications = false` turns all
notifications off.

## Single instance

Start jotty with `--single-instance`, or set `window.single_instance = true`,
and each new jotty opens its window in the one already running instead of
starting another process. Windows open faster and share memory. A window
opened this way still starts in the directory and environment it was
launched from.

All windows of an instance close together if it crashes. Each config file
has its own instance, so `jotty -c other.toml --single-instance` opens a
separate one.
