# Architecture overview

jotty is an event-driven program. One `calloop` event loop waits on the
Wayland socket and the PTY; threads only do work that would stall it
(decoding large images, connecting to the session bus, waiting for
launched programs). The terminal state is plain data,
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
