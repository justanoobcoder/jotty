# Mouse and selection

## Selecting text

| Mouse | Selects |
| --- | --- |
| Drag | The text you drag over |
| Double click | A word; paths, URLs and options such as `--color=auto` count as one |
| Triple click | A whole line |
| Double or triple click, then drag | Whole words or lines |
| Shift+click, then Shift+click again | Everything between the two clicks |

Shift+click works across a long stretch of history without dragging. A
plain click or Escape cancels the first mark.

The selection stays put while output arrives, and follows its text as it
scrolls into the history.

## Copying and pasting

- Ctrl+Shift+C copies the selection, and Ctrl+Shift+V pastes it.
- Selecting text also sets the primary selection, which a middle click
  pastes.
- A notice in the corner says what was copied ("Copied 3 lines"). Set
  `window.copy_notice = false` to hide it.

To copy with Ctrl+C, see [Ctrl+C that copies](keybinds.md#examples).

## Programs that use the mouse

When a program uses the mouse (Neovim, htop, tmux, ...), clicks and the
wheel go to it. Hold Shift to select text, scroll the history or paste
with the middle button anyway.

## Other mouse actions

- Ctrl+click [opens links and files](links-and-files.md).
- Ctrl+drag [drags files](links-and-files.md#dragging-files-out) into
  other applications.
- Ctrl+double click selects a
  [command's output](shell-integration.md#command-output).
