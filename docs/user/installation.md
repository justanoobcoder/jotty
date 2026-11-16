# Installing jotty

jotty is packaged for Arch, NixOS, Debian, Fedora and their relatives, for
x86_64 and aarch64. Every package installs the terminfo entry, a desktop
entry and the icon, so app launchers list jotty.

## Arch Linux

jotty is on the AUR, for Arch and Arch-based distributions (Manjaro,
EndeavourOS, CachyOS, ...), as two packages:

| Package | What it is |
| --- | --- |
| [`jotty-bin`](https://aur.archlinux.org/packages/jotty-bin) | The release binaries |
| [`jotty-git`](https://aur.archlinux.org/packages/jotty-git) | Built from the latest commit on `main` |

```sh
paru -S jotty-bin    # or: yay -S jotty-bin
```

`jotty-bin` updates with each release. `jotty-git` follows `main`; to
rebuild it when there are new commits, upgrade with `paru -Syu --devel`
(or `yay -Syu --devel`).

## NixOS

To try jotty without installing it:

```sh
nix run github:justanoobcoder/jotty/cachix
```

To install it, add the flake to your `inputs`:

```nix
jotty = {
  url = "github:justanoobcoder/jotty";
  inputs.nixpkgs.follows = "nixpkgs";
};
```

Then add its package to your packages:

```nix
{ inputs, pkgs, ... }: {
  environment.systemPackages = [
    inputs.jotty.packages.${pkgs.stdenv.hostPlatform.system}.default
  ];
}
```

### Prebuilt binaries from Cachix

To download jotty instead of compiling it, add the Cachix cache to your
NixOS configuration:

```nix
nix.settings = {
  substituters = [ "https://justanoobcoder.cachix.org" ];
  trusted-public-keys = [ "justanoobcoder.cachix.org-1:D2/vJZSn8o8Fpsxzbjc9tHo2/D1ejBUqc4r71UvDVyk=" ];
};
```

and follow the `cachix` branch, without `inputs.nixpkgs.follows`:

```nix
jotty = {
  url = "github:justanoobcoder/jotty/cachix";
};
```

- The `cachix` branch points at the newest commit of `main` whose build is
  already in the cache.
- jotty must be built against its own pinned nixpkgs, which is what the
  cache holds; with `follows`, it compiles locally.
- The first rebuild after adding the cache still compiles jotty, because
  the new cache only takes effect once that configuration is active.

### home-manager

The flake's home-manager module installs jotty and writes its config from
Nix. `settings` takes the same options as the
[config file](configuration.md), and jotty reloads them on every
`home-manager switch`:

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
installed some other way. To try settings without a `home-manager switch`
each time, use [`jotty config`](configuration.md#trying-options-with-jotty-config).

## Debian, Fedora and others

Each [release](https://github.com/justanoobcoder/jotty/releases) has these
packages:

| Distribution | Package | Install with |
| --- | --- | --- |
| Debian, Ubuntu, Mint, Pop!_OS | `jotty_*.deb` | `sudo apt install ./jotty_*.deb` |
| Fedora, openSUSE, RHEL-likes | `jotty-*.rpm` | `sudo dnf install ./jotty-*.rpm` |
| Other (glibc 2.35+) | `jotty-*-linux.tar.gz` | Unpack anywhere and run `bin/jotty` |

`SHA256SUMS` lists their checksums.

## Building from source

Building needs Rust 1.88 or later, and the development files for wayland,
libxkbcommon, libEGL (libglvnd) and fontconfig:

```sh
cargo build --release
./target/release/jotty
```

Install the terminfo entry too, so programs know what jotty supports:

```sh
tic -x -o ~/.terminfo extra/jotty.terminfo
```

See [Development](../development.md) for working on jotty.
