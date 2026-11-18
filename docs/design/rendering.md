# Rendering (`renderer/`, `font.rs`)

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
(1024×1024 to start with, up to 4096×4096), or color emoji from a BGRA
atlas (512×512 to start with, up to 2048×2048) allocated on first use.
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
and CFF2 outlines. For a variable font, the axis coordinates of the named
instance fontconfig picked (the high 16 bits of its index, read from the
`fvar` table, which `ttf-parser` doesn't expose) are set, as FreeType
would; where the instance lacks the style's weight or slant, `wght` is set
to 400 or 700 and `ital` to its maximum or else `slnt` to its minimum.
When fontconfig matched a face of another style, it asks for `embolden`
and a slanting `matrix` (its `90-synthetic.conf`); unless the face's own
axes give the weight or slant, the glyph's bitmap is then widened by a
24th of the size (as FreeType emboldens) and its rows shifted by 0.2 pixel
per pixel above the baseline. Icons are left as they are.
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
