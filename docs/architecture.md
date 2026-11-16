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

**Contents**

- [Modules](#modules)
- [Terminal emulation (`terminal/`)](#terminal-emulation-terminal)
- [PTY (`pty.rs`)](#pty-ptyrs)
- [Input (`input.rs`)](#input-inputrs)
- [Input method (`ime.rs`, `app.rs`)](#input-method-imers-apprs)
- [Rendering (`renderer/`, `font.rs`)](#rendering-renderer-fontrs)
- [Windows and single instance (`app.rs`, `instance.rs`)](#windows-and-single-instance-apprs-instancers)
- [Window and scaling (`app.rs`)](#window-and-scaling-apprs)
- [Clipboard (`app.rs`)](#clipboard-apprs)
- [Config and overrides (`config.rs`, `settings/`)](#config-and-overrides-configrs-settings)
- [Escape sequences for programs](#escape-sequences-for-programs)
- [Links, files and hints (`links.rs`, `hints.rs`)](#links-files-and-hints-linksrs-hintsrs)
- [Lifetimes and safety](#lifetimes-and-safety)

## Modules

| Module | Responsibility | Depends on Wayland/GL? |
| --- | --- | --- |
| `main.rs` | Logging, CLI, config loading, error reporting | no |
| `cli.rs` | Argument parsing | no |
| `color.rs` | `Rgb`, the color type the config, the terminal and the renderers share | no |
| `config.rs` | TOML config with defaults, validation and clamping; merges the overrides over the config file | no |
| `settings/` | `jotty config`: the overrides file, the TUI (ratatui), the key schema it edits, and the TOML → Nix export | no |
| `terminal/` | VT emulation: grid, scrollback, escape sequences, kitty and sixel graphics | no |
| `pty.rs` | Opens the pseudo-terminal, spawns the child, does non-blocking I/O | no |
| `terminfo.rs` | Picks `$TERM`: jotty's own entry when installed, else `xterm-256color` | no |
| `input.rs` | Key → bytes encoding (xterm and the kitty keyboard protocol), mouse reports, paste encoding | xkb keysyms only |
| `keybinds.rs` | Configurable key bindings for jotty's actions | xkb keysyms only |
| `ime.rs` | Input-method state machine and the surrounding-text approximation | no |
| `selection.rs` | Text selection in history-aware coordinates; words, lines, following output | no |
| `text.rs` | The text of a logical line with the cell of each character | no |
| `links.rs` | Hyperlinks (OSC 8) and URL, file path and regex detection under the pointer | no |
| `hints.rs` | Hints: the links, paths, hashes and addresses on the screen, their labels, and picking one by typing | no |
| `search.rs` | Finding text in the scrollback and on screen | no |
| `vi.rs` | Vi mode: cursor motions, visual modes, yanking | no |
| `notify.rs` | Desktop notifications: a minimal D-Bus client of the session bus for `org.freedesktop.Notifications`. It connects on a thread of its own; once connected it is non-blocking, with a bounded write queue | no |
| `watch.rs` | Reload triggers: inotify on the directories of the config and the overrides, a SIGUSR1 self-pipe | no |
| `instance.rs` | Single instance: the socket path, the lock that lets one jotty bind it, and the request a later jotty sends (directory, environment, command) | no |
| `font.rs` | Font matching (fontconfig), rasterization (ab_glyph), clusters, box drawing | no |
| `emoji.rs` | Color emoji: bitmap glyphs, emoji-font ligatures, scaling | no |
| `renderer/mod.rs` | Builds the vertex batch for a frame: colors, cursor, preedit, selection; the `Renderer` enum over both backends | no |
| `renderer/atlas.rs` | Glyph atlas: shelf packing and the on-demand glyph cache | no |
| `renderer/gl.rs` | EGL context, shaders, atlas texture, draw and swap | yes |
| `renderer/software.rs` | CPU drawing of the batch into `wl_shm` buffers | `wl_shm` only; drawing is unit-tested |
| `app.rs` | Wayland objects and handlers, the windows, the event loop, gluing everything | yes |

Everything except `renderer/gl.rs` and `app.rs` is unit-tested without a
compositor or GPU.

## Terminal emulation (`terminal/`)

`Terminal` owns a `vte::Parser` and a `Screen`. The parser splits the byte
stream into printable characters and control functions, and calls the
`vte::Perform` methods that `Screen` implements.

jotty uses its own fork of vte (`justanoobcoder/vte`),
which adds `Perform::print_ascii`: vte passes each run of printable ASCII in
one call instead of calling `print` per character. `Screen` writes such a run
to the grid a row at a time, which is most of what output is. Everything else
still comes through `print`, one character at a time. The fork also drops
upstream's `ansi` module (jotty has its own terminal), skips invalid UTF-8
in place rather than rescanning the buffer, executes C1 controls split
across reads, and lets CAN and SUB abandon an OSC string instead of
dispatching it truncated.

`Screen` holds:

- two `Grid`s, primary and alternate. Each grid has visible `Row`s, in a
  `VecDeque` so that scrolling the whole screen shifts none of them, plus a
  bounded `VecDeque` of history rows; the alternate grid keeps no history.
- the cursor: position, a template cell carrying the current attributes, the
  pending-wrap flag (set after writing the last column), and the G0/G1
  character sets.
- modes (application cursor, bracketed paste, autowrap, origin, insert, ...),
  the scroll region, and tab stops.
- outputs for the application to collect: `take_responses()` (replies to
  status queries: DSR, DA, DECRQM, OSC 4/10/11/12 color queries, and
  DECRQSS for SGR, the scroll region and the cursor style), `take_title()`,
  `take_bell()`, `take_notifications()` and `take_dirty()`.
- colors the program changed (OSC 4/10/11/12), which the renderer applies
  over the theme, and the configured defaults that queries report.
- tables of hyperlink targets (OSC 8), grapheme cluster tails, and the
  `Extra`s that combine them with an underline color, each by a 16-bit id.
  When a table fills up, `collect_strings` drops what no cell, template or
  saved cursor refers to and renumbers the rest.
- the scrollback view offset, and the prompt marker from OSC 133;B.

Each `Row` also keeps the last OSC 133 mark set on it (prompt, command line
or output start), which moves into the history with the row and is cleared
when the row is erased. `Screen::command_output` walks these marks to find
the output a Ctrl+double click selects, and `Screen::scroll_to_prompt` to
find the prompts it scrolls between. Since a row keeps only its last mark,
a one-row prompt reads as its command line (`B`, or `C` when the output
starts on the same row); such a row starts a prompt unless the marked row
above it holds that prompt's `A`.

A `Cell` is 16 bytes, since the grid and its history hold millions of them
and printing and scrolling are bound by writing them. It is a `char`,
foreground and background colors (default, 256-color index or RGB),
attribute flags that also hold the underline style (single, double, curly,
dotted, dashed; SGR `4:n`), and the id of its `Extra`: the hyperlink id,
cluster id and underline color (SGR 58), which few cells have. Id 0 is none
of them, so most cells never look the table up. A double-width character
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
screen. It takes out OSC 52 strings (`ESC ] 52 ; …`) the same way, since
clipboard text outgrows the 16 KiB vte keeps of an OSC string; as in vte,
BEL or any `ESC` ends them. Each screen buffer has its own `Graphics`: images decoded to RGBA
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

**Sixel (`terminal/sixel.rs`).** A sixel image (`DCS q … ST`) is decoded
as vte passes its bytes (`hook`, `put`, `unhook`) onto an RGBA canvas that
grows with it, and then placed at the cursor like a kitty image, flagged as
a sixel. The cursor ends on the image's last row, in the column it started
at, as in xterm and foot. Text written over a sixel image, and a sixel image
placed over another, cuts the cells it covers out of it: the placement is
replaced by up to four pieces that show parts of the image (`Placement::crop`).
Printing checks a flag first, so it costs next to nothing while no sixel image is on
screen. Primary device attributes (DA1) report sixel support (`4`), and
XTSMGRAPHICS answers the number of color registers and the largest size.

When a row scrolls off the top of the full-screen region on the primary
screen, it moves into history. If the user is looking at history at that
moment, the view offset grows by the same amount so the view stays put.

Once the history is full, the row that falls off its far end becomes the
new blank row at the bottom. That row is long out of the cache, so a `Row`
remembers how far it was written since it was last blank (`occupied`), and
blanking it again writes only that far. Code that writes cells goes
through `Row::cells_in` or `Row::cell_mut`, which extend that mark, or
`Row::cells_mut`, which gives up on it.

## PTY (`pty.rs`)

`openpty(3)` creates the pair. The child gets the slave as stdin, stdout and
stderr, becomes a session leader (`setsid`), and takes the pty as its
controlling terminal (`TIOCSCTTY`). It also gets `TERM`, `COLORTERM=truecolor`
and `TERM_PROGRAM=jotty`. The master is non-blocking:

- Reading happens when the fd is readable, up to 1 MiB per wake-up, so a
  flood of output can't starve input handling and drawing. `EIO` means the
  child and all its descendants closed the pty; the window then closes.
- Writing goes through a queue. If the pty is full (for example when pasting
  a large text into a program that isn't reading), the remainder waits and a
  temporary writable-fd source drains it, so the loop never blocks.

**`TERM` and terminfo (`terminfo.rs`).** The entry in
`extra/jotty.terminfo` is `xterm-256color` plus styled and colored
underlines (`Smulx`, `Setulc`, `Su`), `Tc`, cursor shape and color, and
`Ms` (OSC 52). With it, programs know what jotty supports without probing
at startup, which matters behind tools that relay the terminal and swallow
the probes (`devenv shell` does, so Neovim falls back to plain underlines
unless `TERM=jotty`). With `term = "auto"`, jotty looks for the compiled
entry next to its executable (`<prefix>/share/terminfo`, where the Nix
package and the dev shell put it), then in `$TERMINFO`, `~/.terminfo`,
`$TERMINFO_DIRS` and the system directories. If it finds one, the child
gets `TERM=jotty` and `TERMINFO` pointing at that directory; otherwise
`TERM=xterm-256color`. The child also gets `TERM_PROGRAM_VERSION`.
`tic -x -o ~/.terminfo extra/jotty.terminfo` installs the entry by hand.

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
  already. xkb only does so for ASCII symbols, so on a Cyrillic or Greek
  layout the key's base symbol (the Latin letter at that position) is used.

Combos (`keybinds.rs`) take the modifiers `ctrl`, `alt`, `shift` and
`super` (also `control`, `meta`, `logo`, `mod1`, `mod4`), which must match
exactly, ignoring Caps Lock and Num Lock. The key is a character or an xkb
keysym name, matched case-insensitively (`ctrl++` binds plus). A combo
matches the key's symbol without modifiers (`ctrl+shift+equal`) or the
symbol it produces, minus the Shift that produced it (`ctrl+plus` where `+`
is Shift+`=`); `ctrl+c` never matches Ctrl+Shift+C. Wheel combos run once
per notch (touchpads: once per line's worth), and take precedence over
scrolling and over programs that read the mouse. Unknown actions,
modifiers or keys are reported with their line. Middle-click paste is not
a binding.

Key repeat is done client-side by SCTK's calloop timer. A repeat runs a
bound action again only if it makes sense repeated (`Action::repeats`):
scrolling and font size, not opening a window or reloading the config.

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

The content type is purpose `terminal`, except at a password prompt: when the
pty has echo off but still edits lines (`sudo`, `passwd`, `ssh` set it so),
jotty sends purpose `password` with hints `hidden_text` and `sensitive_data`.
Input methods then pass keys through as typed. While the search bar or vi
mode is open, the purpose stays `terminal`: jotty takes those keys itself.
jotty checks the pty's settings after every frame, so it notices once the
prompt is drawn.

A terminal doesn't know what text is being edited; the shell does. The
surrounding text is therefore approximated as **the cursor's row**, starting
right after the prompt when the shell marked it with OSC 133;B, and trimmed of
trailing blank cells. Without the marker, the prompt is part of the
surrounding text. For an input method that recomposes the word being typed
that is harmless, because only the last word before the cursor matters.

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
`transparent_cell_backgrounds` is set. The atlas is a single-channel
(`R8`) texture sampled with `NEAREST`, since glyphs sit on whole pixels,
and color emoji go to an `RGBA8` texture; they start at 1024×1024 and
512×512 and double when a screen needs more, up to 4096×4096. When one
fills up (`atlas::Atlas`), it grows or, at its largest, is cleared, the
glyphs are repacked and `Glyphs::build` builds the frame once more before
anything is drawn. Each image is uploaded once as a premultiplied `RGBA8`
texture (sampled with `LINEAR`, as images are scaled) and freed when the
program deletes it; the draw call is split at image quads to bind their
texture.

**2b. Drawing on the CPU** (`renderer::software`, the default) paints the
same batch into an ARGB8888 `wl_shm` buffer: it clears to the background,
then fills each quad, blending glyph coverage from an atlas in memory
(1024×1024 to start with, up to 2048×2048), or color emoji from a BGRA
atlas (512×512 to start with) allocated on first use.
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
results cached. All windows use fontconfig's current configuration of the
process; a window opened or reloaded after fonts or configuration files
changed loads a new one in its place, so newly installed fonts are found.
A grapheme cluster is drawn as the single character it composes to under
NFC when there is one (`e` + U+0301 → `é`); otherwise its combining marks
are drawn over the first character, centered.

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

**Memory.** The CPU renderer loads no GPU driver, so jotty stays around
20–30 MiB; output that scrolls the whole screen repaints all of it, so on a
large high-DPI window constant fast scrolling costs more CPU than the GPU
renderer. Loading OpenGL loads the GPU driver into the process: about
100 MiB with the NVIDIA driver, plus as much again for Mesa when both are
installed, since EGL loads every installed vendor before picking one.
Setting `__EGL_VENDOR_LIBRARY_FILENAMES` to the NVIDIA vendor file
(`/run/opengl-driver/share/glvnd/egl_vendor.d/10_nvidia.json` on NixOS)
skips Mesa. If EGL or OpenGL ES 3 is missing, jotty logs a warning and uses
the CPU renderer.

Glyphs that stick out of the cell by more than a pixel (such as ♥ in some
fonts) are shrunk to fit. The fallback order for a missing character is
the regular face, the `fallback` families, a font fontconfig considers like
`family` (DejaVu Sans Mono rather than DejaVu Sans), then any installed
font that has it. Box drawing, block elements, braille, the triangles
`◢◣◤◥` and the Symbols for Legacy Computing are drawn procedurally.

**Frame pacing.** `eglSwapInterval(0)` keeps `eglSwapBuffers` from blocking.
Each draw first requests a `wl_surface.frame` callback, and new content only
marks the window dirty until that callback arrives. Output therefore costs at
most one frame per display refresh, and nothing is drawn while the window is
hidden. `eglSwapBuffers` (or the CPU renderer's attach) also commits the
surface.

## Windows and single instance (`app.rs`, `instance.rs`)

One process can have several windows. `App` holds what they share: the
Wayland globals, the seat's keyboard, pointer, clipboard and text input,
the config, the link patterns and the session bus. Each `TermWindow` holds
its surface, renderer, terminal, pty and the input state that belongs to
it (selection, search, vi mode, preedit). Events that name a surface
(configure, frame callbacks, pointer events, keyboard and text-input
`enter`) go to that surface's window; keys go to the window with keyboard
focus, and input method events to the one the input method entered. Loop
sources (the pty, timers, pastes) carry their window's id and remove
themselves once it is gone. What a window asks of the whole application
(a reload, a new window) is queued and done after the event, so no handler
holds a window while the list changes. A window closes when its child hangs
up or the compositor asks; jotty exits after the last one, with that
window's exit status. Each window loads its own fonts and glyph atlas.

**Single instance.** With `--single-instance`, `main.rs` calls
`instance::start` before connecting to Wayland. Under a lock file
(`flock`), so that two jottys starting at once take turns, it connects to
`$XDG_RUNTIME_DIR/jotty-<display>-<config>.sock`, where `<config>` is an
FNV-1a hash of the canonical `--config` path (or `default`). If a jotty
answers, this one sends a request: a magic, a version, and length-prefixed
byte strings for the directory, the environment and the command, so
nothing needs to be UTF-8. The running jotty reads it from a non-blocking
socket in its event loop, opens a window with that directory and
environment (taking `XDG_ACTIVATION_TOKEN` from it to activate the window),
and replies with a status byte. If no jotty answers (no socket, or one
whose owner died), this one removes the stale file, binds the socket and
serves. When it exits, it removes the socket only if the file is still
the one it created.

The later jotty exits with status 0 once the window is open, or prints the
error and exits with 1 if it can't be (no such program, say). The running
jotty exits when its last window closes, with that window's exit status,
and the next jotty to start becomes the new instance. A jotty of another
version doesn't join, and opens a window of its own.

## Window and scaling (`app.rs`)

The window is an `xdg_toplevel` that asks for server-side decorations. When
the compositor wants client-side ones instead (GNOME has no
`xdg-decoration`), jotty adds an `AdwaitaFrame` from sctk-adwaita: a title bar
(with the window title), borders and shadows drawn into synced subsurfaces placed around the window's surface,
at negative offsets, so the terminal surface and the renderers are
unchanged. The window geometry and minimum size include the frame, and the
configured size minus the frame is the terminal's. Pointer events on the
frame's surfaces (found through their parent surface) go to the frame,
which answers with a move, resize, window menu, minimize, maximize or
close. The frame draws, when dirty, just before the renderer commits the
window. The first configure sets the logical size; the grid size is
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

Notifications (OSC 9, OSC 777) go to the notification daemon through
`notify.rs`, a small D-Bus client: on the first notification it connects
to the session bus (SASL `EXTERNAL`), says `Hello`, subscribes to the
daemon's signals with `AddMatch`, and adds the socket to the event loop. It
then sends `Notify` with a `default` action and remembers the id the reply
carries, with the window that asked for it. When that notification is
clicked, the daemon sends `ActivationToken` and `ActionInvoked`, and jotty
activates that window's surface with the token, which moves focus because the token came from the click. A
daemon without activation tokens gets the urgency request instead. Signals
are accepted only from the bus name that answered the `Notify` call. If the
bus goes away, jotty connects again on the next notification.

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
middle click pastes it. Text a program sends with OSC 52 (`OSC 52 ; c ; <base64> ST`; `p` or `s`
for the primary selection, `pc` for both) takes the same
path, after the read of child output that carried it; of several in one
read, the last for each selection wins. Before the window's first keyboard focus it waits for
the focus, whose serial the compositor accepts.

**Selection and output.** Before reading child output, the app takes a
fingerprint of each selected row on the live screen and the grid's count
of rows scrolled off the top. Afterwards it moves the selection up by the
number of rows that scrolled, and keeps it only if every fingerprinted row
still holds the same text. Pasted text is wrapped in bracketed-paste markers when
the application asked for them, and ESC characters are removed so the paste
can't end bracketed mode early.

## Config and overrides (`config.rs`, `settings/`)

The config file and `~/.local/state/jotty/overrides.toml` (under
`$XDG_STATE_HOME`) are merged key by key, the overrides winning; lists are
replaced as a whole. jotty creates the state directory at startup so that
`watch.rs` can watch it. Reload triggers are a change to either file
(inotify on their directories), the `reload_config` binding and SIGUSR1.
An invalid default config file is logged and the defaults are used; an
invalid `--config` file stops jotty from starting. A reload that fails
keeps the current settings. A new `font.size` replaces any zoom. Values
are clamped where a range makes sense: `font.size` to 4–200,
`font.size_step` to 0.1–10.

If the overrides file is broken, or valid alone but not over the config
file, jotty logs it and ignores the overrides. `jotty config` refuses to
open a broken file, so it can't overwrite it; `jotty config reset` removes
it. In the TUI, `Space` only toggles or cycles, `i` edits any value as
text, `R` removes a section's overrides, and lists are typed
comma-separated or as a TOML array. The palette is listed as 16 named
colors; changing one saves the whole list, which the TUI drops again once
it matches the config file. Saving an empty value removes the override of
an optional key such as `colors.cursor`. `export --merged` has only the
keys set in either file, without the config file's comments.

## Escape sequences for programs

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

## Links, files and hints (`links.rs`, `hints.rs`)

**URLs** are recognized by their scheme (`https://`, `file://`, `ssh://`,
`git://`, `mailto:`, ...) and may wrap across rows. Trailing punctuation
and unmatched closing brackets are left out, so `(see https://x.org/a).`
opens `https://x.org/a`. `url.regex` patterns are matched against the line
under the pointer; an invalid one is logged and only the built-in URL
pattern is used. A link opens when the button is released, unless the
pointer moved.

**Files.** A path is absolute, starts with `~/`, or is relative to the
current directory of the foreground program; names `ls` quotes for spaces
work too, and only files that exist are links. Extensions match
case-insensitively. When an extension is in several kinds, kinds added
under new names win over the built-in ones, and otherwise the first kind
in alphabetical order wins; a built-in kind keeps its place even when
changed. Ctrl+drag takes the whitespace-delimited word under the pointer,
or the single-quoted name around it, dropping a trailing `ls -F` indicator
(`/`, `*`, `@`) if needed. Marks keep the path they had when made, even
after a `cd`; if a marked file is gone when dragged, nothing is dragged.
Dropped links are typed like paths, and dropped text is pasted as it is;
files dragged out of a window aren't taken back by it.

**Hints** label, in this order of precedence where targets overlap: links
(as Ctrl+click finds them), `hints.regex` matches, IPv4 addresses (with a
`/24` prefix length or a `:8080` port) and IPv6 addresses, Nix hashes (SRI
`sha256-…=` of the right length for md5, sha1, sha256 or sha512, or 52
characters of base 32), paths starting with `/`, `~/`, `./` or `../`, hex
hashes of 7 to 128 characters with both letters and digits (`1af659b`, not
`deadbeef`), and names of existing files. The newest output gets the
shortest labels. Whether a target names a file is checked when it is
opened. Copying a link gives its target (the URL of a hyperlink, the
absolute path of a file). Output keeps arriving while the labels are up:
targets move with their text, those whose text changes are dropped, and a
resize takes the labels down.

## Lifetimes and safety

The `unsafe` code is confined to:

- `pty.rs`: `openpty`, `fcntl`, `ioctl`, `kill`, the `pre_exec` hook,
  which only makes async-signal-safe calls, and `waitpid` on a thread for
  the child of a closed window, which nothing else waits for;
- `font.rs`: the fontconfig FFI calls, each a create/use/destroy sequence
  inside one function, and `mmap` of font files, which are never unmapped
  (so the `&'static` slices that faces borrow stay valid);
- `watch.rs`: `inotify_init1`/`inotify_add_watch`, and the SIGUSR1
  handler, which only writes a byte to a pipe the event loop reads;
- `renderer/gl.rs`: EGL and GL calls. The `Renderer` must not outlive the
  `wl_display` or the `wl_surface`. `TermWindow` declares `renderer` before
  `window`, so it is dropped first, and the event loop holding the
  connection outlives `App` and its windows. Each window has its own EGL
  context and surface on the connection's one EGL display: a renderer makes
  its context current before drawing or deleting GL objects, and only the
  last one to go terminates the display;
- `instance.rs`: `flock` on the lock file.
