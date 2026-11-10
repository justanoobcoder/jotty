<p align="center">
  <img src="extra/jotty.svg" alt="jotty logo" width="128">
</p>

# jotty

An opinionated terminal emulator for Wayland, written in Rust.

- **Wayland native.** It speaks Wayland directly through
  [smithay-client-toolkit](https://github.com/Smithay/client-toolkit). There
  is no X11 code path and no GTK, winit or SDL layer.
- **Input methods with surrounding text.** It implements
  `text-input-unstable-v3`, including surrounding text. Input methods such as
  fcitx5 or IBus can commit text, and delete text around the cursor.
- **Light on memory, GPU optional.** By default it draws on the CPU into
  shared memory, repainting only the rows that changed, in about 30+ MiB.
  With `renderer = "gpu"` it draws with OpenGL ES 3 instead (one draw call
  per frame), at the cost of the 120+ MiB a GPU driver takes. Either way,
  frames are paced by the compositor's frame callbacks.
- **Modern text.** Grapheme clusters, color emoji with their sequences,
  reflow on resize, fractional scaling, synchronized output, hyperlinks and
  clickable URLs, scrollback search, and the primary selection.
- **Images.** The kitty graphics protocol, so image previews in yazi,
  `kitten icat`, timg or chafa show real pixels, including through tmux
  with Unicode placeholders.
- **Every key distinct.** The kitty keyboard protocol, so programs that
  ask for it (Claude Code, Neovim, Helix, fish, yazi) can tell Shift+Enter,
  Ctrl+I and Escape apart from Enter, Tab and Alt sequences.
- **Notifications.** Programs can raise a desktop notification with
  `OSC 9` or `OSC 777` (Claude Code can, when a task needs you); clicking
  it brings the window back.
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

With Nix flakes:

```sh
nix run github:justanoobcoder/jotty/cachix               # try it
nix profile install github:justanoobcoder/jotty/cachix   # install into your profile
```

Builds are cached on [Cachix](https://justanoobcoder.cachix.org), so Nix
downloads jotty instead of compiling it. Add the cache with
`cachix use justanoobcoder`, or in `nix.conf`:

```
extra-substituters = https://justanoobcoder.cachix.org
extra-trusted-public-keys = justanoobcoder.cachix.org-1:D2/vJZSn8o8Fpsxzbjc9tHo2/D1ejBUqc4r71UvDVyk=
```

The `cachix` branch always points at the newest commit of `main` whose
build is already in the cache; following `main` itself can pull in a
commit CI is still building, which then compiles locally. Don't set
`inputs.nixpkgs.follows` on jotty: another nixpkgs changes the build, and
the cache no longer has it.

In a NixOS or home-manager configuration, add this repository as a flake
input and put its package in your packages:

```nix
{
  inputs.jotty.url = "github:justanoobcoder/jotty/cachix";
  # ...
  environment.systemPackages = [ inputs.jotty.packages.${pkgs.stdenv.hostPlatform.system}.default ];
}
```

With home-manager, the flake's module installs jotty and writes its
config from Nix. `settings` takes the options of
[docs/configuration.md](docs/configuration.md), and jotty reloads them on
every `home-manager switch`:

```nix
{
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

## License

MIT
