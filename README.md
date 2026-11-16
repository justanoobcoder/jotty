<p align="center">
  <img src="extra/jotty.svg" alt="Jotty logo" width="128">
</p>
<h1 align="center">Jotty</h1>
<p align="center">Jotty is an opinionated terminal emulator for Wayland</p>
<p align="center">
  <a href="https://github.com/justanoobcoder/jotty/actions/workflows/ci.yml"><img src="https://github.com/justanoobcoder/jotty/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="License: MIT"></a>
  <a href="https://github.com/justanoobcoder/jotty/releases/latest"><img src="https://img.shields.io/github/v/release/justanoobcoder/jotty" alt="Release"></a>
  <img src="https://img.shields.io/badge/Wayland-native-green" alt="Wayland native">
  <img src="https://img.shields.io/badge/rust-edition%202024-orange" alt="Rust 2024">
</p>

<img width="1920" height="1080" alt="screenshot_20261110_172458-full" src="https://github.com/user-attachments/assets/d199fad8-129d-4b5a-b09e-1b8973c09c07" />

# Features

- **Wayland native:** It speaks Wayland directly through
  [smithay-client-toolkit](https://github.com/Smithay/client-toolkit).
- **Input methods with surrounding text:** It implements
  `text-input-unstable-v3`, including surrounding text. Input methods such as
  fcitx5 or IBus can commit text, and delete text around the cursor.
- **CPU or GPU renderer:** By default it draws on the CPU, repainting only
  what changed, in about 30 MiB. With `renderer = "gpu"` it draws with
  OpenGL ES 3 instead. See [Renderer](docs/user/appearance.md#renderer).
- **Vi mode and search:** Move over the screen and scrollback with vim keys,
  select, yank and search without the mouse.
  See [Search and vi mode](docs/user/search-and-vi-mode.md).
- **Hints:** Ctrl+Shift+E labels the links, paths, git and Nix hashes and
  IP addresses on the screen. Type a label, then copy, open or insert the
  text. See [Hints](docs/user/hints.md).
- **Links and files:** Ctrl+click opens links, and images, videos and
  documents named in the output. Ctrl+drag files out into other
  applications, or drop files in to type their paths.
  See [Links and files](docs/user/links-and-files.md).
- **Shell integration:** Jump between prompts, and select, copy or open a
  command's output. See [Shell integration](docs/user/shell-integration.md).
- **Live config:** Saving the config file reloads every window, and
  `jotty config` opens a TUI to try options live.
  See [Configuration](docs/user/configuration.md).
- **Single instance:** With `--single-instance`, new windows open in the
  jotty already running. See [Single instance](docs/user/single-instance.md).

## Install

jotty is on the AUR (`jotty-bin`, `jotty-git`), has a Nix flake with a
binary cache and a home-manager module, and each
[release](https://github.com/justanoobcoder/jotty/releases) has `.deb`,
`.rpm` and tarball packages. See the
[installation guide](docs/user/installation.md).

```sh
paru -S jotty-bin                              # Arch
nix run github:justanoobcoder/jotty/cachix     # try it with Nix
```

## Configuration

jotty reads `~/.config/jotty/config.toml`. Every option has a default, so
the file only needs what you change, and saving it reloads every window.
[`docs/config.example.toml`](docs/config.example.toml) lists every option,
and the [configuration guide](docs/user/configuration.md) explains them.
The [default keybinds](docs/user/keybinds.md#default-keybinds) are in the
keybinds guide.

## Documentation

The documentation is also on the website,
<https://justanoobcoder.github.io/jotty/>.

- [User guide](docs/user/index.md): installing, configuring and using jotty
- [Known issues](docs/user/known-issues.md): limitations and missing features
- [Design notes](docs/design/README.md): how jotty works, for people changing it
- [Development](docs/development.md): building, testing, debugging
- [Changelog](CHANGELOG.md): what changed in each release

## License

MIT, see [LICENSE](LICENSE). The licenses of the crates compiled into jotty
are in [THIRD-PARTY-LICENSES](THIRD-PARTY-LICENSES).
