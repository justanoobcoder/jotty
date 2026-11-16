# Keybinds

## Default keybinds

| Shortcut | Action |
| --- | --- |
| Ctrl+Shift+C | Copy the selection |
| Ctrl+Shift+V, Shift+Insert | Paste |
| Middle click | Paste the last selected text |
| Shift+PageUp / PageDown | Scroll the history by a page |
| Shift+Home / End | Jump to the top or bottom of the history |
| Ctrl+Shift+PageUp / PageDown | Scroll to the previous or next prompt (needs [shell integration](shell-integration.md)) |
| Ctrl+= / Ctrl+- / Ctrl+0 | Larger, smaller or default font size |
| Ctrl+Shift+F | [Search](search-and-vi-mode.md#search) the history |
| Ctrl+Shift+Space | [Vi mode](search-and-vi-mode.md#vi-mode) |
| Ctrl+Shift+E | [Hints](hints.md) |
| Ctrl+Shift+N | New window in the current directory |
| Ctrl+Shift+R | Reload the config |
| Ctrl+click | [Open a link or file](links-and-files.md) |
| Ctrl+drag | [Drag a file](links-and-files.md#dragging-files-out) into another application |
| Ctrl+double click | Select a [command's output](shell-integration.md#command-output) |

The mouse is covered in [Mouse and selection](mouse.md).

## Changing keybinds

Each action takes a list of key combos. Listing an action replaces its
defaults; an empty list unbinds it, so its keys reach the program. Actions
you don't list keep their defaults.

```toml
[keybinds]
copy = ["ctrl+insert"]
paste = ["shift+insert"]
new_window = ["shift+return"]
```

| Action | Default | Does |
| --- | --- | --- |
| `copy` | `ctrl+shift+c` | Copy the selection |
| `copy_or_interrupt` | none | Copy if text is selected, otherwise send the key to the program |
| `paste` | `ctrl+shift+v`, `shift+insert` | Paste the clipboard |
| `scroll_line_up`, `scroll_line_down` | none | Scroll the history by a line |
| `scroll_page_up`, `scroll_page_down` | `shift+page_up`, `shift+page_down` | Scroll the history by a page |
| `scroll_to_top`, `scroll_to_bottom` | `shift+home`, `shift+end` | Jump to the top or bottom of the history |
| `scroll_to_prev_prompt`, `scroll_to_next_prompt` | `ctrl+shift+page_up`, `ctrl+shift+page_down` | Scroll to the previous or next prompt |
| `font_larger` | `ctrl+plus`, `ctrl+equal`, `ctrl+kp_add`, `ctrl+wheel_up` | Larger font |
| `font_smaller` | `ctrl+minus`, `ctrl+kp_subtract`, `ctrl+wheel_down` | Smaller font |
| `font_reset` | `ctrl+0`, `ctrl+kp_0` | Default font size |
| `search` | `ctrl+shift+f` | Search the history |
| `vi_mode` | `ctrl+shift+space` | Turn vi mode on or off |
| `hints` | `ctrl+shift+e` | Show hints |
| `new_window` | `ctrl+shift+n` | New window in the current directory |
| `reload_config` | `ctrl+shift+r` | Reload the config |
| `copy_last_output` | none | Copy the last command's output |
| `open_last_output` | none | Open the last command's output in your editor |

## Key combos

A combo is modifiers and a key, joined by `+`:

- Modifiers: `ctrl`, `alt`, `shift`, `super`.
- Key: a character (`c`, `0`), or a key name such as `return`, `space`,
  `insert`, `page_up`, `F5`, `plus` or `kp_add` (keypad +).
- Or a direction of the mouse wheel: `wheel_up`, `wheel_down`,
  `wheel_left`, `wheel_right`.

Modifiers must match exactly, so `ctrl+c` doesn't match Ctrl+Shift+C.

## Examples

**Ctrl+C that copies.** Ctrl+C copies when text is selected, and
interrupts the program when nothing is. Copying clears the selection, so a
second Ctrl+C interrupts.

```toml
[keybinds]
copy_or_interrupt = ["ctrl+c"]
```

**Scroll with Shift.** Scroll by a line with Shift+Up and Shift+Down, and
by a page with Shift and the wheel:

```toml
[keybinds]
scroll_line_up = ["shift+up"]
scroll_line_down = ["shift+down"]
scroll_page_up = ["shift+page_up", "shift+wheel_up"]
scroll_page_down = ["shift+page_down", "shift+wheel_down"]
```

**A command's output.** Copy or open what the last command printed (needs
[shell integration](shell-integration.md)):

```toml
[keybinds]
copy_last_output = ["ctrl+shift+o"]
open_last_output = ["ctrl+shift+u"]
```
