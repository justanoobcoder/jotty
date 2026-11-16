# Known issues and limitations

jotty is deliberately small. This page lists what it doesn't do (yet), so
nothing surprises you.

**Contents**

- [Text and fonts](#text-and-fonts)
- [Terminal emulation](#terminal-emulation)
- [Input method (text-input-v3)](#input-method-text-input-v3)
- [Wayland integration](#wayland-integration)
- [Rendering and performance](#rendering-and-performance)
- [`jotty config`](#jotty-config)
- [Platform](#platform)

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
- **Variable fonts** use the named instance fontconfig picks, with the
  weight and slant of the style set when it lacks them; other axes (width,
  optical size, ...) can't be configured.
- **No synthetic bold or italic.** If the family has no bold or italic face,
  the regular face is used.

## Terminal emulation

- **Reflow** applies to the normal screen and its history when the width
  changes. Full-screen programs (on the alternate screen) are cut off or
  padded instead; they redraw on resize. Reflowing a long history takes a
  moment on every resize step. When the cursor sits above a full screen of
  text, the rows below the screen that no longer fit are dropped, so that
  the cursor stays visible.
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
  [configuration](terminal.md#term-and-terminfo)).
- `TERM=jotty` is unknown on remote hosts until the entry is copied there.
- If a command's last line has no newline and the next prompt starts on
  it, that line isn't part of the selected output. Output marks are kept
  per row, so they are lost when the shell redraws the screen without
  emitting them again.
- Scrolling to a prompt goes by the top row of the view: going back
  scrolls to the first prompt above it, past the prompts already on the
  screen, and a prompt on the live screen can't be scrolled to the top, so
  going forward to it scrolls to the bottom. The marks don't pass through
  multiplexers (zellij, tmux). A command whose output is longer than
  `scrollback` loses its prompt: going back stops at the top of the
  history, in the middle of that output.
- The terminfo entry must stay under 4096 bytes compiled (unibilium, used
  by Neovim, rejects larger files). The Nix build checks this. To fit, it
  leaves out the names of Ctrl+Alt navigation keys (`kUP7`, ...); the keys
  still work, but ncurses programs don't recognize them by name. `Sync` is left
  out too; programs find synchronized output by asking (DECRQM, mode 2026).
- Synchronized output (mode 2026) holds drawing for at most 150 ms, in
  case a program never ends its update.
- Not implemented: `modifyOtherKeys`, grapheme cluster mode (2027) and
  DECCOLM. Unhandled sequences are ignored (logged at
  `debug` level).
- **Kitty graphics**: animation frames (`a=f`, `a=a`, `a=c`) and relative
  placements (`P`, `Q`) are not supported. Images are kept up to 320 MiB,
  1024 images and 4096 placements per screen; past that the oldest are
  evicted. While a large image decodes, the output after it waits, so its
  window shows nothing new until then. Images in the history move with
  the cursor's line when a width change reflows the text.
- **Sixel graphics**: images are at most 4096 pixels per side (the rest is
  cut off) and are drawn at their pixel size, whatever the font size. The
  pixel aspect ratio is taken as 1:1. Color registers are private to each
  image. Sixel display mode (DECSDM, mode 80) and the cursor-right-of-image
  mode (8452) are not supported, nor is changing the number of color
  registers or the image size with XTSMGRAPHICS. Cells a sixel image covers
  are cut out of it at the cell size it was placed with; after a font size
  change the cut is off. An image may allocate and paint 16384 pixels per
  byte it is sent in (a real image needs at most about 7000); what it draws
  past that is dropped, so a few bytes can't cost a full canvas. Still,
  heavily repeated sixels make a few megabytes of output slow to decode.
  An image cut short by CAN or SUB is shown as far as it got, where xterm
  drops it. Cutting images into pieces counts toward the 4096 placements,
  past which the oldest pieces go.
- **OSC 52** only writes the clipboard: queries (`?`) are never answered,
  and an empty payload doesn't clear it. A payload over 16 MiB of base64 is
  dropped. Only the clipboard (`c`) and the primary selection (`p`, `s`)
  exist; targets naming neither (empty, `q`, cut buffers) mean the
  clipboard. Compositors only take a selection with the serial of an input
  event the window received: a write that comes before the window was
  first focused waits for the focus (the last one for each selection, and
  none if the window closes first); one that comes while it is unfocused
  later is made with its last serial, which the compositor may refuse.
- **Long command notifications** time a command from when jotty reads
  its `OSC 133 ; C` mark to when it reads `D`, so output that waits to be
  read (a window still starting) shortens it. Of the commands that finish
  between two reads of the pty, only the longest can be notified. A shell
  without `OSC 133 ; B` and without fish's `cmdline_url` gives no command
  line, and the window title stands in for it. Read from the screen, the
  command line is the rest of the prompt's row, so it includes zsh's
  `RPROMPT` when one stays on screen (unless `TRANSIENT_RPROMPT` is set).
  A shell run inside another (`bash` in fish, `ssh` to a host whose shell
  sends marks) ends the outer command's timing at its own first prompt,
  so the outer one isn't notified.
- The **bell** only marks an unfocused window urgent; there is no sound or
  visual bell.
- **Notifications**: kitty's `OSC 99` is not supported, only `OSC 9` and
  `OSC 777`. Clicking a notification focuses the window only with a
  notification daemon that sends activation tokens.
  A burst of 4 is shown, then one every 2 seconds; the rest are dropped, as
  they are for 30 seconds after failing to reach the session bus.
- Hyperlinks (OSC 8) and grapheme clusters are stored in tables of 65535
  entries per terminal, and so are the distinct combinations of hyperlink,
  cluster and underline color (SGR 58) that cells carry. When one fills,
  entries no cell uses any more are dropped; only when 65535 are still on
  the screen or in the history are new ones dropped (the text stays, without
  that link, cluster tail or underline color). A link the program ends
  always ends, even if its underline color has to go with it.
- Limits against runaway output: a grapheme cluster keeps at most 32
  characters after its first one, a window title 256 characters, and an
  OSC string 16 KiB; the rest is dropped. `CSI n b` (REP) repeats at most
  a row's worth. Replies to queries are dropped while over 1 MiB of input
  waits for a program that isn't reading, and a paste over 64 MiB is
  refused.
- **Opening command output** (`output_click = "open"`, `open_last_output`)
  runs `terminal.editor` in a new jotty window, so a graphical editor
  leaves that window empty until it exits. The output is plain text,
  without its colors. Its file is removed when the window closes, so an
  editor that exits at once and leaves the file to a process already
  running (`code` or `subl` without `--wait`, a server or daemon mode)
  loses it; one that jotty never got to close (a crash or `kill -9`)
  stays in `$XDG_RUNTIME_DIR` until logout.
- Ctrl combined with Enter, Tab or Backspace can't be told apart from the
  plain key in some cases, as in classic xterm, unless the program turns on
  the kitty keyboard protocol.
- The kitty keyboard protocol reports no *base layout key* (the key's
  meaning on a US layout) among the alternate keys, only the shifted key.

## Input method (text-input-v3)

- **Surrounding text is an approximation** (see
  [architecture](../design/input.md#input-method-imers-apprs)). It is the
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

- **The windows of a process go down together.** Ctrl+Shift+N, and every
  window of a single instance, run in one process, so a crash closes all
  of them.
- `--single-instance` leaves its lock file in `$XDG_RUNTIME_DIR` (removing
  it would race with a jotty starting at the same time), and a jotty that
  starts while the serving one is stalled waits on the lock instead of
  opening a window of its own.
- With `--single-instance`, the jotty that hands its window over exits at
  once: it doesn't wait for the window, and its exit status isn't the
  program's. Run it without single instance to wait (`jotty -e cmd` in a
  script).
- A window from Ctrl+Shift+N gets jotty's own environment, the one the
  first window started with, not the shell's: variables exported in the
  shell since then (or by `nix develop` in it) are missing. Windows handed
  over by `--single-instance` do get the environment of the command that
  asked.
- The notification limit (a burst of 4, then one every 2 seconds) counts
  the notifications of all windows of a process together.
- **Client-side decorations come from sctk-adwaita.** On compositors without
  server-side decorations (GNOME/Mutter), the title bar imitates Adwaita
  rather than using GTK. Its light or dark theme is asked of the desktop
  once per process, and its button layout once per window, so later changes
  to either don't apply; it ignores jotty's color theme. It draws at the next
  integer scale, and keeps its borders and resize edges while tiled (only a
  maximized window loses them). Creating it runs `gsettings`, `fc-match` and
  `dbus-send` on the event loop's thread: each new window stalls all
  windows for tens of milliseconds, up to about 200 ms when the desktop
  portal is slow. Without these programs on `PATH`, the frame falls back to
  the light theme and the Cantarell font it embeds.
- **Fractional scaling** needs both `wp-fractional-scale-v1` and
  `wp-viewporter`; without them, jotty renders at the next integer scale.
- Block (rectangular) selection is only available in vi mode, not with the
  mouse. The selection is kept while output goes elsewhere, but cleared
  when its text changes or it scrolls out of the history.
- Vi mode has motions, visual modes, yank and search, but not vim's text
  objects (`iw`), marks or registers. `G` goes to the bottom of the
  screen.
- While a program has mouse reporting on, Ctrl+click, Ctrl+drag and
  Ctrl+right click go to the program; hold Shift as well to open a link or
  drag or mark files.
- Marks of files for dragging are dropped when their text changes, the
  window is resized or the alternate screen switches. Ctrl+right dragging
  over `ls -l` output may also mark files that other columns (a user name,
  a month) happen to name. At most 1000 files are marked.
- Marking and dragging files check them on the file system from the event
  loop: a sweep tries up to 10000 names. On a slow network mount, every
  window waits meanwhile.
- Relative file paths are resolved in the directory the foreground program
  is in now, not where the path was printed. After a `cd`, or over SSH,
  older output may point to the wrong place (or nowhere, and then isn't a
  link).
- **Hints** cover the first cells of their targets with the label. They
  only label the visible screen, as it was when they came up: targets that
  output prints later aren't labeled. Relative paths that don't start
  with `./` or `../` (`src/app.rs`, `Cargo.toml`) are only labeled when
  they name a file that exists in the current directory of the program in
  the foreground. Inside a multiplexer that program is the multiplexer, so
  they usually aren't labeled, and files from `ls` in a pane can't be
  opened (Ctrl+O looks for them in the multiplexer's directory). IPv6 addresses with a zone (`%eth0`), a prefix
  length or an IPv4 tail (`::ffff:1.2.3.4`) are only labeled in part.
  Targets that start above the top row or end below the bottom row (a
  long URL cut by the edge) aren't labeled. A path with a line number
  opens in the editor with `+LINE`, which some editors (Helix, VS Code)
  don't take, and its column is ignored. Hex hashes in mixed case aren't
  matched. Bringing the labels up checks every name on the screen on the
  file system, from the event loop (each once), as marking files does.
- No `idle-inhibit`.
- Dropped files are typed as paths, quoted for POSIX shells and fish. A
  `file://` URI's host is ignored; in a path, bytes that aren't UTF-8 are
  replaced, and control characters (a newline in a name) become `?`.
- A dragged file is offered as a `file://` URI only, not through the
  document portal (`application/vnd.portal.filetransfer`). Sandboxed
  applications, such as Flatpaks, may not be able to read it when it lies
  outside their sandbox.
- wlroots-based compositors don't send the keyboard modifiers again after a
  drag, so jotty asks for the keyboard again once its own drag ends (or the
  Ctrl of Ctrl+drag would stay held). Programs that ask for focus reports
  see the window lose and regain the focus then.
- The kitty graphics medium `t=s` removes the shared memory object it is
  given, whichever it is, as kitty does; `t=f` reads any regular file the
  user can read, and shows it as an image at most.
- OSC 8 hyperlinks open with any URI scheme on Ctrl+click, and the target
  isn't shown first. Only one that starts with `-`, which the launcher would
  take for an option, is refused.
- Ctrl with a letter of a non-Latin layout sends the letter, as in xterm,
  unless the program turns on the kitty keyboard protocol.
- Saving the config reloads it by watching the file's directory, which
  jotty creates at startup if it is missing. If it can't be created, or
  is removed or replaced while jotty runs (a symlink to the whole
  directory, as home-manager makes with `xdg.configFile."jotty".source`,
  being switched), changes aren't noticed. When the config file is a
  symlink, editing the file it points to in another directory isn't
  noticed either (replacing the symlink, as home-manager does, is). Press
  Ctrl+Shift+R, send `SIGUSR1` or restart jotty then.

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
- A glyph atlas that fills up doubles (up to 4096×4096 on the GPU, within
  the driver's texture size limit, and 2048×2048 on the CPU) and is
  repacked, then the frame is built again before it is drawn, which can
  cause a brief hitch with very large fonts or many scripts or emoji on
  screen. If the screen needs more than the largest atlas holds, the glyphs
  that don't fit are left out.
- A frame that fails to draw is retried every 200 ms; the window closes
  after 50 failures in a row (10 seconds).
- Windows of the same process share the font files, but each looks up
  its own faces and rasterizes glyphs into its own atlas.

## `jotty config`

- An override can only set a key, not unset one. If the config file sets
  `terminal.shell` or an optional color, the TUI can't bring back the
  default (`$SHELL`, or the derived color) over it. It can only remove its
  own override. Edit the config file for that.
- The TUI edits the `[files]` kinds that already exist. Add new kinds in the
  config file (or by hand in `overrides.toml`); after that they show up in
  the TUI.
- `jotty config export --merged` loses the config file's comments and
  ordering.
- Saving replaces `overrides.toml` with a new file, so a symlink there
  becomes a regular file.
- If `~/.local/state/jotty/` is deleted while jotty runs, open windows stop
  noticing changes to the overrides until they are restarted or reloaded
  (Ctrl+Shift+R, `SIGUSR1`).

## Platform

- Linux only: it uses `openpty`, `TIOCSCTTY`, `EIO` for detecting that the
  child exited, `/proc` for the shell's directory, and inotify to notice
  config changes. The BSDs would need small changes in `pty.rs`.
- Wayland only. There is no X11 support, by design.
