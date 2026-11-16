# Search and vi mode

## Search

Press Ctrl+Shift+F and type to search the history, from the bottom up. The
match is selected and scrolled into view. The search ignores case unless
you type an uppercase letter.

| Key | Does |
| --- | --- |
| Up, Ctrl+R | Previous (older) match |
| Down, Ctrl+S | Next (newer) match |
| Enter | Close the search and keep the match selected |
| Escape | Close the search |

## Vi mode

Vi mode lets you move over the screen and history with vim keys, and
select and copy text without the mouse.

1. Press Ctrl+Shift+Space. A cursor appears where the terminal's cursor
   is, and a label in the corner shows the mode.
2. Move to the text, press `v` and move to select it.
3. Press `y` to copy it.
4. Press Escape to leave.

Keys don't reach the program while vi mode is on.

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
| `/` `?` | Search down or up |
| `n` `N` | Next or previous match |
| Escape | Leave (also `i`, `a`, `q`) |
