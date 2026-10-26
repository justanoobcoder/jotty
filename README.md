# jotty

A minimal terminal emulator for Wayland, written in Rust.

- **Wayland native.** It speaks Wayland directly through
  [smithay-client-toolkit](https://github.com/Smithay/client-toolkit). There
  is no X11 code path and no GTK, winit or SDL layer.
- **Input methods with surrounding text.** It implements
  `text-input-unstable-v3`, including surrounding text. Input methods such as
  fcitx5 or IBus can show preedit text inline, commit text, and delete text
  around the cursor (useful for Vietnamese, Chinese, Japanese, Korean, and so
  on).
- **Light on memory, GPU optional.** By default it draws on the CPU into
  shared memory, repainting only the rows that changed, in about 20 MiB.
  With `renderer = "gpu"` it draws with OpenGL ES 3 instead (one draw call
  per frame), at the cost of the 100+ MiB a GPU driver takes. Either way,
  frames are paced by the compositor's frame callbacks.
- **Modern text.** Grapheme clusters, color emoji with their sequences,
  reflow on resize, fractional scaling, synchronized output, hyperlinks and
  clickable URLs, scrollback search, and the primary selection.
- **Minimal.** About 11k lines of Rust including tests, one TOML config
  file, no tabs or splits. Use your compositor or tmux for those.

## Install

With Nix flakes, from a checkout:

```sh
nix run .               # try it
nix profile install .   # install into your profile
```

In a NixOS or home-manager configuration, add this repository as a flake
input and put its package in your packages:

```nix
{
  inputs.jotty.url = "git+file:///path/to/jotty"; # or its git URL
  # ...
  environment.systemPackages = [ inputs.jotty.packages.${pkgs.system}.default ];
}
```

From source (needs Rust 1.88+, and the development files for wayland,
libxkbcommon, libEGL/libglvnd and fontconfig):

```sh
cargo build --release
./target/release/jotty
```

## Usage

```
jotty [-c CONFIG] [-e COMMAND [ARGS...]]
```

| Key / action           | Does                                         |
| ---------------------- | -------------------------------------------- |
| Ctrl+Shift+C           | Copy the selection to the clipboard           |
| Ctrl+Shift+V, Shift+Insert | Paste the clipboard                       |
| Middle click           | Paste the primary selection (the last selected text) |
| Left drag              | Select text (Shift+drag in programs that use the mouse) |
| Double click           | Select a word, or a command's whole output (opt-in, needs [shell integration](docs/configuration.md#shell-integration)) |
| Triple click           | Select a line (drag after a double or triple click to extend by words or lines) |
| Shift+PageUp/PageDown  | Scroll the history by a page                  |
| Shift+Home/End         | Jump to the top or bottom of the history      |
| Mouse wheel            | Scroll the history (arrow keys in full-screen programs; sent to programs that use the mouse) |
| Ctrl+= / Ctrl+- / Ctrl+0 | Larger / smaller / default font size        |
| Ctrl+Shift+F           | Search the scrollback                         |
| Ctrl+click             | Open a link, URL, image or video file (Ctrl+hover underlines it) |

These are the defaults; key bindings can be changed in the
[`[keybinds]`](docs/configuration.md#keybinds) section.

The configuration lives in `$XDG_CONFIG_HOME/jotty/config.toml`. See
[docs/configuration.md](docs/configuration.md) for every option.

For the best input-method experience, have your shell mark where its prompt
ends (OSC 133). See [Shell integration](docs/configuration.md#shell-integration).

## Documentation

- [Configuration](docs/configuration.md): options, colors, fonts, shell integration
- [Architecture](docs/architecture.md): how the pieces fit together
- [Development](docs/development.md): building, testing, debugging
- [Known issues](docs/known-issues.md): limitations and missing features

## License

MIT
