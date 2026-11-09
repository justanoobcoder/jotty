# Known issues and limitations

jotty is deliberately small. This page lists what it doesn't do (yet), so
nothing surprises you.

## Text and fonts

- **Cell widths follow `wcwidth`.** Grapheme clusters are kept together,
  but an emoji sequence still takes the cells that programs count for it:
  👍🏽 and 👨‍👩‍👧 are drawn as one emoji in their first two cells, followed
  by empty cells (2 + 2 and 2 + 2 + 2 columns). This keeps the cursor where
  shells and editors expect it. The exception is VS16 (`❤️`), which
  widens a narrow character to two cells, as starship and most terminals
  assume.
- **Combining marks without a precomposed form** are drawn over the base
  character, centered, without the font's mark positioning; stacked or
  unusual marks may not sit exactly right.
- **No shaping or ligatures** for text. Each character is drawn on its own,
  so Arabic, Indic scripts and programming ligatures don't render properly.
  (Emoji fonts' own ligatures, for emoji sequences, are applied.)
- **Color emoji need a bitmap font** (CBDT or sbix, such as Noto Color
  Emoji). Vector color fonts (COLR, SVG) aren't supported; with only those
  installed, emoji are monochrome. Which characters are emoji by default is
  approximated (wide characters in the emoji and symbol blocks).
- Search compares one character per cell: the one a cell's cluster composes
  into, so `é` matches however it was written, or else the cluster's first.
  Marks that don't compose are ignored.
- **Variable fonts** only get their `wght` axis set (400, or 700 for bold).
  Other axes keep their defaults, and italic is not synthesized.
- **No synthetic bold or italic.** If the family has no bold or italic face,
  the regular face is used.
- The font size changes in whole points (Ctrl+=/Ctrl+-).

## Terminal emulation

- **Reflow** applies to the normal screen and its history when the width
  changes. Full-screen programs (on the alternate screen) are cut off or
  padded instead; they redraw on resize. Reflowing a long history takes a
  moment on every resize step.
- Mouse reporting supports X10 (`?9`), normal (`?1000`), button-motion
  (`?1002`) and any-motion (`?1003`) tracking in the SGR (`?1006`) and
  legacy encodings. Not supported: the UTF-8 (`?1005`) and urxvt (`?1015`)
  encodings, highlight tracking (`?1001`), and pixel positions (`?1016`).
  In the legacy encoding, positions past column or row 223 can't be
  reported, so those events are dropped.
- With `TERM=xterm-256color` (when jotty's terminfo entry isn't installed,
  or set explicitly), Neovim detects curly and colored underlines by
  querying jotty (DECRQSS). That query doesn't survive relays such as
  `devenv shell` or tmux, so diagnostics fall back to plain underlines
  there. Install the entry (see
  [configuration](configuration.md#term-and-terminfo)).
- `TERM=jotty` is unknown on remote hosts until the entry is copied there.
- The word delimiters for double-click selection can't be configured.
- If a command's last line has no newline and the next prompt starts on
  it, that line isn't part of the selected output. Output marks are kept
  per row, so they are lost when the shell redraws the screen without
  emitting them again.
- The terminfo entry must stay under 4096 bytes compiled (unibilium, used
  by Neovim, rejects larger files). The Nix build checks this. To fit, it
  leaves out the names of Ctrl+Alt navigation keys (`kUP7`, ...); the keys
  still work, but ncurses programs don't recognize them by name. `Sync` is left
  out too; programs find synchronized output by asking (DECRQM, mode 2026).
- Synchronized output (mode 2026) holds drawing for at most 150 ms, in
  case a program never ends its update.
- Not implemented: `modifyOtherKeys`, OSC 52
  clipboard access, grapheme cluster mode (2027), sixel graphics,
  blinking text, DECCOLM, and DECSCNM. Unhandled sequences are ignored
  (logged at `debug` level).
- **Kitty graphics**: animation frames (`a=f`, `a=a`, `a=c`) and relative
  placements (`P`, `Q`) are not supported. Images are kept up to 320 MiB,
  1024 images and 4096 placements per screen; past that the oldest are
  evicted. Images are decoded on the event loop, so a very large one stalls
  the window while it decodes. Images in the history move with
  the cursor's line when a width change reflows the text.
- The **bell** only marks an unfocused window urgent; there is no sound or
  visual bell.
- **Notifications**: kitty's `OSC 99` is not supported, only `OSC 9` and
  `OSC 777`. Clicking a notification focuses the window only with a
  notification daemon that sends activation tokens.
  A burst of 4 is shown, then one every 2 seconds; the rest are dropped, as
  they are for 30 seconds after failing to reach the session bus.
- Hyperlinks (OSC 8) and grapheme clusters are stored in tables of 65535
  entries per terminal. When one fills, entries no cell uses any more are
  dropped; only when 65535 are still on the screen or in the history are
  new ones dropped (the text stays).
- Limits against runaway output: a grapheme cluster keeps at most 32
  characters after its first one, a window title 256 characters, and an
  OSC string 16 KiB; the rest is dropped. `CSI n b` (REP) repeats at most
  a row's worth. Replies to queries are dropped while over 1 MiB of input
  waits for a program that isn't reading, and a paste over 64 MiB is
  refused.
- Ctrl combined with Enter, Tab or Backspace can't be told apart from the
  plain key in some cases, as in classic xterm, unless the program turns on
  the kitty keyboard protocol.
- The kitty keyboard protocol reports no *base layout key* (the key's
  meaning on a US layout) among the alternate keys, only the shifted key.

## Input method (text-input-v3)

- **Surrounding text is an approximation** (see
  [architecture](architecture.md#input-method-imers-apprs)). It is the
  cursor's row, starting after the OSC 133 prompt marker if there is one.
  Multi-line commands only report their current row, and in full-screen
  programs (vim, tmux) it is whatever is on the cursor row.
- `delete_surrounding_text` is carried out by sending Backspace and Delete
  keys. That works with line editors (readline, zle, fish, vim insert mode)
  but not with programs that give those keys another meaning.
- The text selection isn't reported as the IME anchor; the anchor always
  equals the cursor.
- The preedit is clipped at the end of the line rather than wrapped.
- **Password prompts are only seen in jotty's own pty.** Inside a
  multiplexer (zellij, tmux) jotty's pty is the multiplexer's, which is
  always in raw mode, so a `sudo` prompt in a pane is not reported as a
  password field. The same goes for a prompt on the far side of `ssh`, or
  one from a full-screen program that reads passwords itself.
- **The password state is only re-checked after a frame.** jotty reads the
  pty's echo setting when it draws. A program that turns echo back on
  without printing anything leaves the input method in password mode (no
  composition) until the next frame; one that turns echo off after its
  prompt was already drawn isn't reported until something redraws, such as
  a key press or the cursor blinking. A shell prompt usually redraws at once,
  so in practice the gap is short.

## Wayland integration

- **No client-side decorations.** jotty asks for server-side decorations. On
  compositors that don't provide them (GNOME/Mutter), the window has no title
  bar or border.
- **Fractional scaling** needs both `wp-fractional-scale-v1` and
  `wp-viewporter`; without them, jotty renders at the next integer scale.
- Block (rectangular) selection is only available in vi mode, not with the
  mouse. The selection is kept while output goes elsewhere, but cleared
  when its text changes or it scrolls out of the history.
- Vi mode has motions, visual modes, yank and search, but not vim's text
  objects (`iw`), `f`/`t`, marks or registers. `G` goes to the bottom of the
  screen.
- While a program has mouse reporting on, Ctrl+click goes to the program;
  hold Shift as well to open a link.
- Relative file paths are resolved in the directory the foreground program
  is in now, not where the path was printed. After a `cd`, or over SSH,
  older output may point to the wrong place (or nowhere, and then isn't a
  link).
- No `idle-inhibit`. Drag-and-drop is not accepted.
- The kitty graphics medium `t=s` removes the shared memory object it is
  given, whichever it is, as kitty does; `t=f` reads any regular file the
  user can read, and shows it as an image at most.
- OSC 8 hyperlinks open with any URI scheme on Ctrl+click, and the target
  isn't shown first. Only one that starts with `-`, which the launcher would
  take for an option, is refused.
- Ctrl with a letter of a non-Latin layout sends the letter, as in xterm,
  unless the program turns on the kitty keyboard protocol.
- Saving the config reloads it by watching the file's directory. A directory
  that doesn't exist at startup, or is removed later, isn't watched; send
  `SIGUSR1` or restart then. When the
  config is a symlink, editing the file it points to in another directory
  isn't noticed (replacing the symlink, as home-manager does, is); press
  Ctrl+Shift+R or send `SIGUSR1` then.

## Rendering and performance

- Blur only uses `ext-background-effect-v1`. The older KDE
  (`org_kde_kwin_blur`) and Hyprland-specific mechanisms are not supported;
  on those compositors, use their window rules instead. The blur path has
  not been tested against a compositor that implements the protocol.
- With transparency, the input-method preedit is drawn over a translucent
  background, so text under it on the same row can show through faintly.
- Every frame rebuilds the whole screen's quads. The CPU renderer then
  repaints and damages only the rows that changed, so typing is cheap, but
  output that scrolls the whole screen repaints all of it; on a large
  high-DPI window, constant fast scrolling costs more CPU than the GPU
  renderer.
- The GPU renderer needs OpenGL ES 3.0 through EGL. Without it, jotty falls
  back to the CPU renderer.
- When a glyph atlas fills up (text: 2048×2048 on the GPU, 1024×1024 on
  the CPU; color emoji: 1024×1024 and 512×512), both are cleared and the
  frame is redrawn, which can cause a brief hitch with very large fonts or
  many scripts or emoji on screen. If the screen needs more than an atlas
  holds, the glyphs that don't fit are left out of that frame.
- A frame that fails to draw is retried every 200 ms; the window closes
  after 50 failures in a row (10 seconds).

## Platform

- Linux only: it uses `openpty`, `TIOCSCTTY`, `EIO` for detecting that the
  child exited, `/proc` for the shell's directory, and inotify to notice
  config changes. The BSDs would need small changes in `pty.rs`.
- Wayland only. There is no X11 support, by design.
