# Escape sequences for programs

**Colors.** Programs can set and query colors while they run. *color* is
`#rrggbb`, `#rgb` or `rgb:rr/gg/bb`, and `?` queries the current value
(Neovim does with OSC 11 to pick a dark or light background). A full reset
restores the configured colors. Colors 16–255 are the xterm 6×6×6 cube and
24-step grayscale ramp.

| Sequence | Color | Reset with |
| --- | --- | --- |
| OSC 4 ; *n* ; *color* | palette entry *n* (0–255) | OSC 104 |
| OSC 10 ; *color* | default foreground | OSC 110 |
| OSC 11 ; *color* | default background (also the padding) | OSC 111 |
| OSC 12 ; *color* | cursor | OSC 112 |

**Cursor.** `CSI Ps SP q` (DECSCUSR) sets the cursor style; `CSI 0 SP q`
and a reset bring back the configured one. A blinking cursor toggles every
500 ms while the window is focused, and typing shows it at once.

**Notifications.** `OSC 9 ; message ST` (iTerm2) uses the window title as
the summary; `OSC 777 ; notify ; title ; body ST` (rxvt-unicode) gives both.
`OSC 9` followed by a number (`OSC 9 ; 4 ; …`, progress bars) is ignored.
A program can show 4 notifications at once, then one every 2 seconds; more
are dropped. A click focuses the window when the daemon sends an
activation token (version 1.2 of the notification specification);
otherwise the window is only marked urgent. For long commands, the summary
is the command line (sent by fish 4, otherwise read from the screen after
the `OSC 133 ; B` mark) and the body says how long it took and whether it
failed.

**Clipboard.** OSC 52 reads (`OSC 52 ; c ; ?`) are always refused, since
any program, or a file shown with `cat`, could read the clipboard. With
`osc52 = false` writes are dropped too; the terminfo entry still
advertises `Ms`.

**Shell marks (OSC 133).** `A` marks the start of the prompt, `B` its end,
`C` the start of a command's output, and `D ; status` its end. Selecting a
command's output needs `A` (where it ends) and `C`, or `B` for a one-row
command line (where it starts). Scrolling between prompts needs `A` or
`B`; with only `B`, a prompt of several rows is shown from its command
line. Long-command notifications need `C` and `D`. The text reported to
input methods starts after `B`.

**The editor.** Command output opened in `terminal.editor` is written as
plain text to a file in `$XDG_RUNTIME_DIR` that only the user can read, and
removed when the editor's window closes. That window closes when the
editor exits, so an editor that hands the file to a running process and
exits (`code` or `subl` without `--wait`) finds it gone. `$VISUAL` and
`$EDITOR` are split at spaces.
