# Design notes

The [user guide](../user/index.md) explains how to configure and use
jotty. These notes are for people changing it: how the pieces fit
together, the protocols and escape sequences involved, and the details
behind behavior the user guide only summarizes.

Start with the user guide when a detail affects configuration or normal
use. Use a design note when it explains a subsystem's internals, its
limits, or behavior a change could break.

- [Architecture overview](overview.md): the event loop, the modules, and
  where `unsafe` lives
- [Terminal and PTY](terminal.md): the grid, scrollback, reflow, images,
  the pseudo-terminal and terminfo
- [Input](input.md): keys, key bindings, the mouse and input methods
- [Rendering](rendering.md): building frames, the GPU and CPU renderers,
  fonts and emoji
- [Windows](windows.md): several windows per process, single instance,
  scaling, the bell and notifications
- [Clipboard](clipboard.md): copying, pasting and OSC 52
- [Config and overrides](config.md): loading, reloading and `jotty config`
- [Escape sequences for programs](escape-sequences.md): colors, the
  cursor, notifications, the clipboard and shell marks
- [Links, files and hints](links-and-hints.md): what counts as a link, a
  file or a hint target

See [Development](../development.md) for building, testing and debugging.
