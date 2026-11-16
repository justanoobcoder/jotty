# Input

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
