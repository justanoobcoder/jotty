# Known issues and limitations

jotty is deliberately small. This page lists what it doesn't do (yet), so
nothing surprises you.

## Text and fonts

- **No combining characters or grapheme clusters.** Zero-width code points
  are dropped: combining accents (`e` + U+0301), ZWJ emoji sequences, and
  variation selectors other than VS16. VS16 (U+FE0F, emoji presentation)
  widens the preceding character to two cells when it has an emoji form,
  so prompts such as starship's stay aligned. Precomposed text, including all Vietnamese letters as
  input methods produce them, displays correctly.
- **No shaping or ligatures.** Each character is drawn on its own, so Arabic,
  Indic scripts and programming ligatures don't render properly.
- **No color emoji.** Bitmap and color fonts (CBDT/COLR, such as Noto Color
  Emoji) aren't supported and are skipped during fallback. Emoji-presentation
  characters are drawn with a monochrome glyph, enlarged to fill their two
  cells. Emoji appear as
  monochrome glyphs if a font like Noto Emoji or Symbola is installed;
  otherwise they are missing.
- **Variable fonts** only get their `wght` axis set (400, or 700 for bold).
  Other axes keep their defaults, and italic is not synthesized.
- **No synthetic bold or italic.** If the family has no bold or italic face,
  the regular face is used.
- The font size changes in whole points (Ctrl+=/Ctrl+-).

## Terminal emulation

- **No reflow on resize.** Making the window narrower cuts off long lines
  instead of re-wrapping them, and making it wider doesn't un-wrap them.
- **No mouse reporting** (modes 1000/1002/1003/1006). Mouse clicks in vim,
  htop or tmux are not sent to them. The wheel sends arrow keys on the
  alternate screen, which scrolls most pagers and editors.
- Not implemented: synchronized output (mode 2026), the kitty keyboard
  protocol and `modifyOtherKeys`, OSC 8 hyperlinks, OSC 52 clipboard access,
  OSC 4/10/11 color queries, sixel/kitty graphics, blinking text, DECCOLM, and
  DECSCNM. Unhandled sequences are ignored (logged at `debug` level).
- The **bell** is ignored.
- Ctrl combined with Enter, Tab or Backspace can't be told apart from the
  plain key in some cases, as in classic xterm.

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

## Wayland integration

- **No client-side decorations.** jotty asks for server-side decorations. On
  compositors that don't provide them (GNOME/Mutter), the window has no title
  bar or border.
- **Integer scaling only.** Fractional scales (`wp_fractional_scale_v1`)
  aren't supported. At 1.25× or 1.5×, jotty renders at the next integer scale
  and the compositor scales it down, which looks slightly soft.
- **No primary selection.** Middle click pastes the regular clipboard, and
  selecting text doesn't copy it automatically.
- Selection is cell-linear only: no double-click words, triple-click lines or
  block selection. It is cleared whenever new output arrives.
- No `xdg-activation` (urgency) or `idle-inhibit`.
- Pointer: only the text cursor shape. Drag-and-drop is not accepted.

## Rendering and performance

- Blur only uses `ext-background-effect-v1`. The older KDE
  (`org_kde_kwin_blur`) and Hyprland-specific mechanisms are not supported;
  on those compositors, use their window rules instead. The blur path has
  not been tested against a compositor that implements the protocol.
- With transparency, the input-method preedit is drawn over a translucent
  background, so text under it on the same row can show through faintly.
- Opacity can't be changed at runtime.

- Every frame rebuilds and uploads the vertex buffer for the whole screen
  (no damage tracking). This is cheap for normal sizes, but a huge window
  with a tiny font does more work than necessary.
- Requires OpenGL ES 3.0 through EGL; there is no software fallback inside
  jotty (Mesa's llvmpipe works).
- When the 2048×2048 glyph atlas fills up, it is cleared and the frame is
  redrawn, which can cause a brief hitch with very large fonts or many
  scripts on screen.

## Platform

- Linux only: it uses `openpty`, `TIOCSCTTY`, and `EIO` for detecting that
  the child exited. The BSDs would need small changes in `pty.rs`.
- Wayland only. There is no X11 support, by design.
