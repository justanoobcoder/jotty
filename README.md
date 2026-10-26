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
- **GPU rendering.** It draws with OpenGL ES 3 on an EGL surface: one draw
  call per frame, glyphs cached in a texture atlas, and frames paced by the
  compositor's frame callbacks.
- **Minimal.** About 6k lines of Rust, one TOML config file, no tabs or
  splits. Use your compositor or tmux for those.

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
| Ctrl+Shift+V, Shift+Insert, middle click | Paste the clipboard        |
| Left drag              | Select text                                   |
| Shift+PageUp/PageDown  | Scroll the history by a page                  |
| Shift+Home/End         | Jump to the top or bottom of the history      |
| Mouse wheel            | Scroll the history (arrow keys in full-screen programs) |
| Ctrl+= / Ctrl+- / Ctrl+0 | Larger / smaller / default font size        |

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
