# Windows

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
window's exit status. Each window loads its own fonts and glyph atlas;
the font files' mappings and fontconfig's configuration are shared.

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
