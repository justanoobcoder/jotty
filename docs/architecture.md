# Architecture

jotty is a single-threaded, event-driven program. One `calloop` event loop
waits on the Wayland socket and the PTY. The terminal state is plain data,
and the renderer turns it into vertices once per frame.

```
             ┌────────────── calloop event loop (app.rs) ───────────────┐
             │                                                          │
  keyboard ──┤ input.rs ── bytes ──► pty.rs ──► child process (shell)   │
  text-input ┤ ime.rs   ── bytes ──┘   │                                │
  pointer ───┤ selection.rs            │ output                         │
             │                         ▼                                │
             │                terminal/ (vte parser → Screen)           │
             │                         │ dirty                          │
             │  frame callback ──► renderer/ build() ──► gl.rs ──► EGL  │
             │                         ▲                                │
             │                     font.rs (fontconfig + ab_glyph)      │
             └──────────────────────────────────────────────────────────┘
```

## Modules

| Module | Responsibility | Depends on Wayland/GL? |
| --- | --- | --- |
| `main.rs` | Logging, CLI, config loading, error reporting | no |
| `cli.rs` | Argument parsing | no |
| `config.rs` | TOML config with defaults, validation and clamping | no |
| `terminal/` | VT emulation: grid, scrollback, escape sequences | no |
| `pty.rs` | Opens the pseudo-terminal, spawns the child, does non-blocking I/O | no |
| `terminfo.rs` | Picks `$TERM`: jotty's own entry when installed, else `xterm-256color` | no |
| `input.rs` | Key → bytes encoding, shortcuts, paste encoding | xkb keysyms only |
| `ime.rs` | Input-method state machine and the surrounding-text approximation | no |
| `selection.rs` | Text selection in history-aware coordinates | no |
| `font.rs` | Font matching (fontconfig), rasterization (ab_glyph), box drawing | no |
| `renderer/mod.rs` | Builds the vertex batch for a frame: colors, cursor, preedit, selection | no |
| `renderer/atlas.rs` | Shelf packing for the glyph texture | no |
| `renderer/gl.rs` | EGL context, shaders, atlas texture, draw and swap | yes |
| `app.rs` | Wayland objects and handlers, the event loop, gluing everything | yes |

Everything except `renderer/gl.rs` and `app.rs` is unit-tested without a
compositor or GPU.

## Terminal emulation (`terminal/`)

`Terminal` owns a `vte::Parser` and a `Screen`. The parser splits the byte
stream into printable characters and control functions, and calls the
`vte::Perform` methods that `Screen` implements.

`Screen` holds:

- two `Grid`s, primary and alternate. Each grid has visible `Row`s plus a
  bounded `VecDeque` of history rows; the alternate grid keeps no history.
- the cursor: position, a template cell carrying the current attributes, the
  pending-wrap flag (set after writing the last column), and the G0/G1
  character sets.
- modes (application cursor, bracketed paste, autowrap, origin, insert, ...),
  the scroll region, and tab stops.
- outputs for the application to collect: `take_responses()` (replies to
  status queries: DSR, DA, OSC 12, and DECRQSS for SGR, the scroll region
  and the cursor style), `take_title()`, `take_bell()` and `take_dirty()`.
- the scrollback view offset, and the prompt marker from OSC 133;B.

A `Cell` is a `char`, foreground and background colors (default, 256-color
index or RGB), attribute flags, and an underline style (single, double,
curly, dotted, dashed; SGR `4:n`) with its own color (SGR 58). A double-width character occupies a `WIDE`
cell followed by a `WIDE_SPACER` cell. Every write and erase calls
`split_wide()` so a wide character is never left half-overwritten.

When a row scrolls off the top of the full-screen region on the primary
screen, it moves into history. If the user is looking at history at that
moment, the view offset grows by the same amount so the view stays put.

## PTY (`pty.rs`)

`openpty(3)` creates the pair. The child gets the slave as stdin, stdout and
stderr, becomes a session leader (`setsid`), and takes the pty as its
controlling terminal (`TIOCSCTTY`). It also gets `TERM`, `COLORTERM=truecolor`
and `TERM_PROGRAM=jotty`. The master is non-blocking:

- Reading happens when the fd is readable, up to 1 MiB per wake-up, so a
  flood of output can't starve input handling and drawing. `EIO` means the
  child and all its descendants closed the pty; jotty then exits.
- Writing goes through a queue. If the pty is full (for example when pasting
  a large text into a program that isn't reading), the remainder waits and a
  temporary writable-fd source drains it, so the loop never blocks.

## Input (`input.rs`)

SCTK delivers keys already translated by xkbcommon: a keysym plus the UTF-8
text. jotty's own shortcuts (copy, paste, scrolling, zoom) are matched first.
Everything else goes to `encode_key`, which produces xterm-compatible
sequences:

- cursor keys use `CSI` or `SS3` depending on application cursor mode, and
  `CSI 1;m X` when modifiers are held;
- `CSI n ~` for the editing keys and F5–F12;
- Alt adds an ESC prefix; Ctrl+key maps to control characters when xkb didn't
  already.

Key repeat is done client-side by SCTK's calloop timer.

Pointer events go through the same split. When the program has enabled
mouse reporting (the terminal tracks DECSET 9/1000/1002/1003 and 1006) and
Shift isn't held, `app.rs` maps the pointer position to a cell and
`encode_mouse` turns the event into an SGR (`CSI < b;x;y M/m`) or legacy
(`CSI M` + three bytes) report. Motion is reported once per cell, the
wheel once per notch (buttons 64–67), and a release always follows a
reported press. Otherwise the pointer selects text and scrolls the history.

## Input method (`ime.rs`, `app.rs`)

jotty binds `zwp_text_input_manager_v3` and creates one `zwp_text_input_v3`
per seat at startup. On `enter`, it enables the text input with content
purpose `terminal`. On `leave`, it disables it.

The protocol is double-buffered. `preedit_string`, `commit_string` and
`delete_surrounding_text` events go into `ImeState` as pending state, and
`done` applies them in the order the protocol requires:

1. **Delete surrounding text.** The byte lengths are converted to character
   counts within the surrounding text that was last sent, then sent as that
   many Backspace (`DEL`) and Delete (`CSI 3~`) key presses. The shell's line
   editor does the deleting.
2. **Commit.** The UTF-8 text goes straight to the pty.
3. **Preedit.** It replaces the current preedit, which the renderer draws
   underlined at the cursor, with a beam cursor at the IME's caret.

After every frame, jotty recomputes the surrounding text and cursor
rectangle. If either changed, it sends them (`set_surrounding_text`,
`set_text_change_cause`, `set_cursor_rectangle`, `commit`).

A terminal doesn't know what text is being edited; the shell does. The
surrounding text is therefore approximated as **the cursor's row**, starting
right after the prompt when the shell marked it with OSC 133;B, and trimmed of
trailing blank cells. Without the marker, the prompt is part of the
surrounding text. For a dead-key or Telex-style IME (such as Vietnamese
Unikey or Bamboo) that is harmless, because only the last word before the
cursor matters.

## Rendering (`renderer/`, `font.rs`)

Rendering has two stages, and only the second touches the GPU.

**1. Building the frame** (`renderer::build`) walks the visible rows (from
history when scrolled back) and emits quads into a `Batch`: cell backgrounds
that differ from the default background, glyphs, underline and strikethrough,
the cursor (block, beam, underline, or a hollow box when unfocused), and the
preedit. Backgrounds go first, so a glyph that overhangs into the next cell is
never covered. Underlines other than plain ones are bitmaps from
`font::underline_glyph`, cached in the atlas like glyphs. Their patterns
repeat once per cell, so a curly underline is a continuous wave. Colors are resolved here: palette, 256-color cube, grayscale
ramp, bold-is-bright, dim, inverse, hidden, selection and block cursor.

Glyphs come through the `GlyphCache` trait. Tests pass in a fake
implementation; at run time `gl.rs` provides one that rasterizes on first use
and uploads into the atlas.

**2. Drawing** (`renderer::gl`) uploads the batch into one vertex buffer and
issues one `glDrawArrays`. A vertex has position, texture coordinate, color,
and a mode (solid or glyph). A single shader handles both modes, writing
premultiplied alpha. The EGL config has an alpha channel, so the
compositor blends the window: the clear color is the background at
`window.opacity`, and colored cell backgrounds use the same alpha when
`transparent_cell_backgrounds` is set. The atlas is a 2048×2048
single-channel (`R8`) texture sampled with `NEAREST`, since glyphs sit on
whole pixels. When it fills up, it is cleared and the frame is redrawn.

**Fonts.** fontconfig picks the regular, bold, italic and bold-italic faces,
and finds fallback faces per character (excluding color fonts), with the
results cached. `ab_glyph` (built on `ttf-parser`) rasterizes TrueType, CFF
and CFF2 outlines; for variable fonts, the `wght` axis is set to 400 or 700.
Cell size comes from the primary face's advance width and line metrics.
Box-drawing lines and block elements are drawn procedurally to fill the cell
exactly, so they join without gaps.

**Frame pacing.** `eglSwapInterval(0)` keeps `eglSwapBuffers` from blocking.
Each draw first requests a `wl_surface.frame` callback, and new content only
marks the window dirty until that callback arrives. Output therefore costs at
most one frame per display refresh, and nothing is drawn while the window is
hidden. `eglSwapBuffers` also commits the surface.

## Window and scaling (`app.rs`)

The window is an `xdg_toplevel` that asks for server-side decorations. The
first configure sets the logical size; the grid size is
`(physical size − 2·padding) / cell size`. Integer output scales come from
SCTK's `scale_factor_changed`. jotty sets the buffer scale, re-rasterizes the
font at `size × 96/72 × scale` pixels per em, resizes the EGL window, and
reports the new grid size to the child with `TIOCSWINSZ`.

For transparency, `app.rs` also sets a whole-surface opaque region when the
opacity is 1.0. When `window.blur` is set, it asks
`ext_background_effect_manager_v1` for a blur region once the compositor has
advertised that capability. Both regions are larger than any window and
clipped by the compositor, so resizing never has to update them.

## Clipboard (`app.rs`)

Copying creates a `wl_data_source` for `text/plain;charset=utf-8`,
`UTF8_STRING` and `text/plain`. When another client asks for the data, a
short-lived thread writes it to the pipe, so a slow reader can't block the
event loop. Pasting reads the current selection offer through a calloop
source on the pipe. When jotty owns the selection itself, it pastes directly
without a round trip. Pasted text is wrapped in bracketed-paste markers when
the application asked for them, and ESC characters are removed so the paste
can't end bracketed mode early.

## Lifetimes and safety

The `unsafe` code is confined to:

- `pty.rs`: `openpty`, `fcntl`, `ioctl`, `kill`, and the `pre_exec` hook,
  which only makes async-signal-safe calls;
- `font.rs`: the fontconfig FFI calls, each a create/use/destroy sequence
  inside one function;
- `renderer/gl.rs`: EGL and GL calls. The `Renderer` must not outlive the
  `wl_display` or the `wl_surface`. `App` declares `renderer` before `window`,
  so it is dropped first, and the event loop holding the connection outlives
  `App`.
