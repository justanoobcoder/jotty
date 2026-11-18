# Terminal and PTY

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
  History rows are stored without the blank cells at their end (see below),
  so a row's `width` can be larger than the cells it holds; readers treat
  the missing cells as blank.
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

**Decoding images off the event loop.** Reading, decompressing and decoding
a large image takes long enough to stall every window, so a transmission
whose payload is over 64 KiB, or that names a file or shared memory, is not
finished on the spot: `Terminal::feed_until_decode` stops right after it
and returns a `graphics::Decode`, and `TermWindow::feed` runs it on a
thread, pausing that window's pty source and keeping the output read after
it. When the `Decoded` result comes back over a channel,
`Terminal::finish_decode` stores and places the image, as if it had been
decoded at once, and the waiting output is fed. What follows an image may
depend on where it left the cursor, so nothing after it is processed
first. A thread that dies without a result (a panic in a decoder) closes
the channel, and the transmission fails with `Decode::failed` instead of
leaving the window waiting; jobs are numbered so that a late result can't
resume a later pause. Small inline images, the common case for many small icons, are
decoded on the spot.

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
screen, a copy of it moves into history. If the user is looking at history
at that moment, the view offset grows by the same amount so the view stays
put.

The copy (`Row::trimmed`) leaves out the `Cell::default()` cells at the end
of the row, which most rows mostly are, so the history takes memory for what
was written rather than for its full width. Blanks with a background color
are kept, and so is all of a soft-wrapped row, whose trailing blanks are
text that reflow joins. The row itself becomes the new blank row at the
bottom while it is still in the cache. A `Row` remembers how far it was
written since it was last blank (`occupied`), so copying and blanking it
touch only that far. Code that writes cells goes through `Row::cells_in` or
`Row::cell_mut`, which extend that mark, or `Row::cells_mut`, which gives up
on it.

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
