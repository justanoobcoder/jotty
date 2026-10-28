# Architecture

jotty is a single-threaded, event-driven program. One `calloop` event loop
waits on the Wayland socket and the PTY. The terminal state is plain data,
and the renderer turns it into quads once per frame, drawn with OpenGL ES or
on the CPU.

```
             ┌────────────── calloop event loop (app.rs) ───────────────┐
             │                                                          │
  keyboard ──┤ input.rs ── bytes ──► pty.rs ──► child process (shell)   │
  text-input ┤ ime.rs   ── bytes ──┘   │                                │
  pointer ───┤ selection.rs, links.rs  │ output                         │
             │                         ▼                                │
             │                terminal/ (vte parser → Screen)           │
             │                         │ dirty                          │
             │  frame callback ──► renderer/ build() ─┬► gl.rs ──► EGL  │
             │                                        └► software ► shm │
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
| `terminal/` | VT emulation: grid, scrollback, escape sequences, kitty graphics | no |
| `pty.rs` | Opens the pseudo-terminal, spawns the child, does non-blocking I/O | no |
| `terminfo.rs` | Picks `$TERM`: jotty's own entry when installed, else `xterm-256color` | no |
| `input.rs` | Key → bytes encoding (xterm and the kitty keyboard protocol), mouse reports, paste encoding | xkb keysyms only |
| `keybinds.rs` | Configurable key bindings for jotty's actions | xkb keysyms only |
| `ime.rs` | Input-method state machine and the surrounding-text approximation | no |
| `selection.rs` | Text selection in history-aware coordinates; words, lines, following output | no |
| `text.rs` | The text of a logical line with the cell of each character | no |
| `links.rs` | Hyperlinks (OSC 8) and URL, file path and regex detection under the pointer | no |
| `search.rs` | Finding text in the scrollback and on screen | no |
| `vi.rs` | Vi mode: cursor motions, visual modes, yanking | no |
| `watch.rs` | Reload triggers: inotify on the config's directory, a SIGUSR1 self-pipe | no |
| `font.rs` | Font matching (fontconfig), rasterization (ab_glyph), clusters, box drawing | no |
| `emoji.rs` | Color emoji: bitmap glyphs, emoji-font ligatures, scaling | no |
| `renderer/mod.rs` | Builds the vertex batch for a frame: colors, cursor, preedit, selection; the `Renderer` enum over both backends | no |
| `renderer/atlas.rs` | Glyph atlas: shelf packing and the on-demand glyph cache | no |
| `renderer/gl.rs` | EGL context, shaders, atlas texture, draw and swap | yes |
| `renderer/software.rs` | CPU drawing of the batch into `wl_shm` buffers | `wl_shm` only; drawing is unit-tested |
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
  status queries: DSR, DA, DECRQM, OSC 4/10/11/12 color queries, and
  DECRQSS for SGR, the scroll region and the cursor style), `take_title()`,
  `take_bell()` and `take_dirty()`.
- colors the program changed (OSC 4/10/11/12), which the renderer applies
  over the theme, and the configured defaults that queries report.
- tables of hyperlink targets (OSC 8) and grapheme cluster tails, which
  cells refer to by a 16-bit id.
- the scrollback view offset, and the prompt marker from OSC 133;B.

Each `Row` also keeps the last OSC 133 mark set on it (prompt, command line
or output start), which moves into the history with the row and is cleared
when the row is erased. `Screen::command_output` walks these marks to find
the output a Ctrl+double click selects.

A `Cell` (24 bytes) is a `char`, foreground and background colors
(default, 256-color index or RGB), attribute flags, an underline style
(single, double, curly, dotted, dashed; SGR `4:n`) with its own color
(SGR 58), a hyperlink id and a cluster id. A double-width character
occupies a `WIDE` cell followed by a `WIDE_SPACER` cell. Every write and
erase calls `split_wide()` so a wide character is never left
half-overwritten.

**Grapheme clusters.** A printed character that continues the cluster of
the last one (by the Unicode segmentation rules) joins it instead of
starting a new cell: its text is added to the first cell's cluster tail.
Zero-width characters (combining marks, ZWJ, variation selectors) take no
room. Characters with a width (the emoji after a ZWJ, skin tones, the
second flag letter) still take their `wcwidth` cells, as `CONTINUATION`
cells that only reserve room, so the cursor stays where programs expect.

**Reflow.** When the width changes, `Grid::reflow` joins soft-wrapped rows
of the primary screen and its history into lines, re-wraps them to the new
width (moving a wide character that would straddle a row boundary to the
next row), and maps the cursor, the saved cursor, the prompt marker and
shell-integration marks to their new positions. The alternate screen is
cut off or padded instead.

**Synchronized output.** While mode 2026 is set, the application holds
drawing until it is reset, or for at most 150 ms.

**Images (`terminal/graphics.rs`).** vte drops APC strings, so
`Terminal::feed` takes `ESC _ … ESC \` out of the byte stream first (also
when split across reads) and hands kitty graphics commands (`G…`) to the
screen. Each screen buffer has its own `Graphics`: images decoded to RGBA
(raw RGB/RGBA or PNG, optionally zlib-compressed, sent inline in chunks or
read from a file, a temporary file or POSIX shared memory) and placements.
A placement's row is counted from the grid's `scrolled` counter, so it
moves into the history with the text; region scrolls, inserted and deleted
lines, `ED 2`/`ED 3` and the alternate screen move or drop placements
explicitly. Virtual placements (`U=1`) are drawn wherever a cell holds
U+10EEEE: its foreground color gives the image id, its underline color the
placement, and combining diacritics the image row and column. The screen
also answers `CSI 14/16/18 t`, and the pty's window size carries the grid's
size in pixels, from which programs compute the cell size.

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
text. jotty's own key bindings (copy, paste, scrolling, zoom; see
`keybinds.rs`) are matched first, against both that keysym and the key's
symbol without modifiers. For the latter, `app.rs` compiles its own copy
of the keymap the compositor sends.
Everything else goes to `encode_key`, which produces xterm-compatible
sequences:

- cursor keys use `CSI` or `SS3` depending on application cursor mode, and
  `CSI 1;m X` when modifiers are held;
- `CSI n ~` for the editing keys and F5–F12;
- Alt adds an ESC prefix; Ctrl+key maps to control characters when xkb didn't
  already.

Key repeat is done client-side by SCTK's calloop timer.

**Kitty keyboard protocol.** A program can turn on the protocol's
progressive enhancements with `CSI > flags u` (and restore them with
`CSI < u`, set them with `CSI = flags ; mode u`, query them with
`CSI ? u`). Each screen keeps its own flags and a stack of 16 pushed ones,
so an editor on the alternate screen doesn't change the shell's. With any
flag set, `input::encode` uses `CSI number ; modifiers u` (or the
`CSI 1 ; modifiers A`/`~` forms of functional keys): Escape and keys with
Ctrl, Alt or Super are no longer ambiguous, so Shift+Enter, Ctrl+I and Tab
all differ. The other flags add repeat and release events (a release is
sent only for keys whose press went to the program), the shifted key,
every key as an escape code including modifier keys, and the key's text.
Plain Enter, Tab and Backspace keep their usual bytes unless every key is
reported.

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

Rendering has two stages. The first is the same for both backends; the
second draws with the GPU (`renderer = "gpu"`) or the CPU (`"cpu"`).

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
implementation; at run time `atlas::Atlas` provides one that rasterizes on
first use, places the bitmap in an atlas image (coverage for text, RGBA
for color emoji), and hands it to the backend to store (a texture upload,
or a copy into memory). Cells with a grapheme cluster ask for the whole
cluster; continuation cells draw no glyph. The frame also carries the
hovered link (underlined) and the search bar, drawn over the bottom row.

Images become quads too (mode 3), with the image's uid recorded in
`Batch::images`. By z-index they go under the cell backgrounds (below
−2³⁰), between backgrounds and text (below 0), or over the text, and are
clipped to the grid.

**2. Drawing** (`renderer::gl`) uploads the batch into one vertex buffer and
issues one `glDrawArrays`. A vertex has position, texture coordinate, color,
and a mode (solid, glyph, or color glyph). A single shader handles all
modes, writing premultiplied alpha. The EGL config has an alpha channel, so the
compositor blends the window: the clear color is the background at
`window.opacity`, and colored cell backgrounds use the same alpha when
`transparent_cell_backgrounds` is set. The atlas is a 2048×2048
single-channel (`R8`) texture sampled with `NEAREST`, since glyphs sit on
whole pixels, and color emoji go to a 1024×1024 `RGBA8` texture. When
either fills up, both are cleared and the frame is redrawn. Each image is
uploaded once as an `RGBA8` texture (sampled with `LINEAR`, as images are
scaled) and freed when the program deletes it; the draw call is split at
image quads to bind their texture.

**2b. Drawing on the CPU** (`renderer::software`, the default) paints the
same batch into an ARGB8888 `wl_shm` buffer: it clears to the background,
then fills each quad, blending glyph coverage from a 1024×1024 atlas in
memory, or color emoji from a 512×512 BGRA atlas allocated on first use.
Quads are axis-aligned and glyphs 1:1, so this is a plain blit with
premultiplied-alpha blending, and the result matches the GL output pixel
for pixel. Images are scaled by sampling the nearest pixel.

Only what changed is repainted. The window is divided into horizontal
bands: the padding above the grid, one band per text row, and the rest.
Each frame, every quad is hashed into each band it touches, which gives a
fingerprint per band that also covers glyphs overhanging from neighboring
rows. Buffers come from SCTK's `SlotPool`; a buffer the compositor still
holds is skipped, so usually two alternate. Each buffer remembers the
fingerprints of what it holds, so only bands that differ from it are
repainted; only bands that differ from the previous frame are damaged.

**Fonts.** fontconfig picks the regular, bold, italic and bold-italic faces,
and finds fallback faces per character (excluding color fonts), with the
results cached. A grapheme cluster is drawn as the single character it
composes to under NFC when there is one (`e` + U+0301 → `é`); otherwise
its combining marks are drawn over the first character, centered.

**Color emoji** (`emoji.rs`) come from the font fontconfig picks for
`emoji` with color, loaded on first use. Characters that are emoji by
default and sequences (VS16, ZWJ, flags, skin tones, keycaps) are mapped to
glyphs, and the font's ligature substitutions (GSUB lookup type 4) turn a
sequence into one glyph. Its PNG (CBDT) or BGRA (sbix) bitmap is decoded,
premultiplied, and scaled by area averaging to fit the cell slot. `ab_glyph` (built on `ttf-parser`) rasterizes TrueType, CFF
and CFF2 outlines; for variable fonts, the `wght` axis is set to 400 or 700.
Cell size comes from the primary face's advance width and line metrics.
Font files are memory-mapped rather than read, so a large fallback font
(Noto Sans CJK is over 30 MB) only takes memory for the parts in use.
Box-drawing lines and block elements are drawn procedurally to fill the cell
exactly, so they join without gaps.

**Frame pacing.** `eglSwapInterval(0)` keeps `eglSwapBuffers` from blocking.
Each draw first requests a `wl_surface.frame` callback, and new content only
marks the window dirty until that callback arrives. Output therefore costs at
most one frame per display refresh, and nothing is drawn while the window is
hidden. `eglSwapBuffers` (or the CPU renderer's attach) also commits the
surface.

## Window and scaling (`app.rs`)

The window is an `xdg_toplevel` that asks for server-side decorations. The
first configure sets the logical size; the grid size is
`(physical size − 2·padding) / cell size`. With `wp-fractional-scale-v1` and
`wp-viewporter`, the scale is the compositor's preferred fractional scale:
the buffer is `round(logical size × scale)` pixels and the viewport maps it
to the logical size. Otherwise integer scales come from SCTK's
`scale_factor_changed`, and jotty sets the buffer scale. Either way, it
re-rasterizes the font at `size × 96/72 × scale` pixels per em, resizes
the EGL window (or drops the shared-memory buffers of the old size), and
reports the new grid size to the child with `TIOCSWINSZ`.

When the bell rings while the window is unfocused, jotty requests an
`xdg-activation` token without an input serial and activates its own
surface with it; compositors show such a request as urgency rather than
moving focus.

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
without a round trip. The primary selection works the same way through
`primary-selection-unstable-v1`: finishing a selection offers it, and a
middle click pastes it.

**Selection and output.** Before reading child output, the app takes a
fingerprint of each selected row on the live screen and the grid's count
of rows scrolled off the top. Afterwards it moves the selection up by the
number of rows that scrolled, and keeps it only if every fingerprinted row
still holds the same text. Pasted text is wrapped in bracketed-paste markers when
the application asked for them, and ESC characters are removed so the paste
can't end bracketed mode early.

## Lifetimes and safety

The `unsafe` code is confined to:

- `pty.rs`: `openpty`, `fcntl`, `ioctl`, `kill`, and the `pre_exec` hook,
  which only makes async-signal-safe calls;
- `font.rs`: the fontconfig FFI calls, each a create/use/destroy sequence
  inside one function, and `mmap` of font files, which are never unmapped
  (so the `&'static` slices that faces borrow stay valid);
- `watch.rs`: `inotify_init1`/`inotify_add_watch`, and the SIGUSR1
  handler, which only writes a byte to a pipe the event loop reads;
- `renderer/gl.rs`: EGL and GL calls. The `Renderer` must not outlive the
  `wl_display` or the `wl_surface`. `App` declares `renderer` before `window`,
  so it is dropped first, and the event loop holding the connection outlives
  `App`.
