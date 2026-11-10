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

# Features

- **Wayland native.** It speaks Wayland directly through
  [smithay-client-toolkit](https://github.com/Smithay/client-toolkit).
- **Input methods with surrounding text.** It implements
  `text-input-unstable-v3`, including surrounding text. Input methods such as
  fcitx5 or IBus can commit text, and delete text around the cursor.
- **Support both CPU and GPU renderer.** By default it draws on the CPU into
  shared memory, repainting only the rows that changed, in about 30+ MiB.
  With `renderer = "gpu"` it draws with OpenGL ES 3 instead (one draw call
  per frame), at the cost of the 130+ MiB a GPU driver takes.
- **Vi mode.** Move over the screen and scrollback with vim keys, select
  by character, line or block, yank, and search, all without the mouse.

## Install

Each [release](https://github.com/justanoobcoder/jotty/releases) has
packages for x86_64 and aarch64 (the Arch package is x86_64 only):

| Distribution | Package | Install with |
| --- | --- | --- |
| Debian, Ubuntu, Mint, Pop!_OS | `jotty_*.deb` | `sudo apt install ./jotty_*.deb` |
| Fedora, openSUSE, RHEL-likes | `jotty-*.rpm` | `sudo dnf install ./jotty-*.rpm` |
| Arch, Manjaro, EndeavourOS | `jotty-*.pkg.tar.zst` | `sudo pacman -U jotty-*.pkg.tar.zst` |
| Other (glibc 2.35+) | `jotty-*-linux.tar.gz` | unpack anywhere and run `bin/jotty` |

`SHA256SUMS` lists their checksums. The packages install the terminfo
entry, a desktop entry and the icon, so app launchers list jotty.

From source (needs Rust 1.88+, and the development files for wayland,
libxkbcommon, libEGL/libglvnd and fontconfig):

```sh
cargo build --release
./target/release/jotty
```

## Install on NixOS

Add the flake to your `inputs`:

```nix
jotty = {
  url = "github:justanoobcoder/jotty";
  inputs.nixpkgs.follows = "nixpkgs";
};
```

Then put its package in your packages:

```nix
{ inputs, pkgs, ... }: {
  environment.systemPackages = [
    inputs.jotty.packages.${pkgs.stdenv.hostPlatform.system}.default
  ];
}
```

To try it without installing: `nix run github:justanoobcoder/jotty/cachix`.

### Prebuilt binaries from Cachix

To skip compiling jotty, add the Cachix cache to your NixOS configuration:

```nix
nix.settings = {
  substituters = [ "https://justanoobcoder.cachix.org" ];
  trusted-public-keys = [ "justanoobcoder.cachix.org-1:D2/vJZSn8o8Fpsxzbjc9tHo2/D1ejBUqc4r71UvDVyk=" ];
};
```

and follow the `cachix` branch in `flake.nix`:

```nix
jotty = {
  url = "github:justanoobcoder/jotty/cachix"; # drop inputs.nixpkgs.follows = "nixpkgs";
};
```

The `cachix` branch points at the newest commit of `main` whose build is
already in the cache. Without `follows`, jotty is built against its own
pinned nixpkgs, which is what the cache holds; another nixpkgs changes the
build and it compiles locally.

The first rebuild after adding the cache still compiles jotty locally,
because the new substituter only takes effect once that configuration is
active. Later rebuilds download it.

### home-manager

The flake's home-manager module installs jotty and writes its config from
Nix. `settings` takes the options of
[docs/configuration.md](docs/configuration.md), and jotty reloads them on
every `home-manager switch`:

```nix
{ inputs, ... }: {
  imports = [ inputs.jotty.homeManagerModules.default ];

  programs.jotty = {
    enable = true;
    settings = {
      font = {
        family = "JetBrains Mono";
        size = 12.0;
      };
      window.opacity = 0.9;
      keybinds.copy_or_interrupt = [ "ctrl+c" ];
    };
  };
}
```

Set `programs.jotty.package = null` to manage only the config of a jotty
installed some other way.

To try settings without a `home-manager switch` each time, run
`jotty config`. Its changes go to `~/.local/state/jotty/overrides.toml`,
which takes precedence over the config from Nix. Once you are happy, paste
the output of `jotty config export --nix` into `settings` and run
`jotty config reset`. See [overrides](docs/configuration.md#overrides-and-jotty-config).

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
| Double click           | Select a word |
| Ctrl+double click      | Select a command's whole output (needs [shell integration](docs/configuration.md#shell-integration)) |
| Triple click           | Select a line (drag after a double or triple click to extend by words or lines) |
| Shift+click, Shift+click | Select from the first clicked character to the second (Escape or a plain click cancels) |
| Shift+PageUp/PageDown  | Scroll the history by a page                  |
| Shift+Home/End         | Jump to the top or bottom of the history      |
| Mouse wheel            | Scroll the history (arrow keys in full-screen programs; sent to programs that use the mouse) |
| Ctrl+= / Ctrl+- / Ctrl+0 | Larger / smaller / default font size        |
| Ctrl+Shift+F           | Search the scrollback                         |
| Ctrl+Shift+Space       | Vi mode: move with hjkl/w/b/gg/G, select with v/V/Ctrl+V, yank with y, search with / ? n N |
| Ctrl+Shift+N           | New window in the current directory           |
| Ctrl+Shift+R           | Reload the config (saving the file also reloads it) |
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
- [Changelog](CHANGELOG.md): what changed in each release

## License

MIT, see [LICENSE](LICENSE). The licenses of the crates compiled into jotty
are in [THIRD-PARTY-LICENSES](THIRD-PARTY-LICENSES).
