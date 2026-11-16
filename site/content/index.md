---
hide:
  - navigation
  - toc
---

<div class="jotty-hero" markdown>

<img class="jotty-logo" src="assets/logo.svg" alt="jotty logo">

# jotty

<p class="jotty-tagline">An opinionated terminal emulator for Wayland</p>

[Install](user/installation.md){ .md-button .md-button--primary }
[Features](features.md){ .md-button }
[User guide](user/index.md){ .md-button }

</div>

<img class="jotty-shot" alt="jotty running in a Wayland session" src="https://github.com/user-attachments/assets/d199fad8-129d-4b5a-b09e-1b8973c09c07">

<div class="grid cards" markdown>

-   :material-monitor:{ .lg .middle } **Wayland native**

    ---

    Speaks Wayland directly, with no X11 layer. Input methods get
    surrounding text, so they can edit around the cursor.

-   :material-memory:{ .lg .middle } **CPU or GPU**

    ---

    Draws on the CPU by default, repainting only the rows that changed, in
    about 30 MiB. `renderer = "gpu"` switches to OpenGL ES.

-   :material-keyboard:{ .lg .middle } **Vi mode and hints**

    ---

    Move over the scrollback with vim keys, or label the links, paths and
    hashes on screen and pick one by typing its label.

-   :material-file-move:{ .lg .middle } **Drag files in and out**

    ---

    Ctrl+drag a file named in `ls` output into another application, or drop
    files onto the window to type their paths.

-   :material-reload:{ .lg .middle } **Live config**

    ---

    Saving the config reloads every window. `jotty config` opens a TUI to
    try options live, even on a read-only home-manager config.

-   :simple-nixos:{ .lg .middle } **Packaged**

    ---

    AUR, a Nix flake with a binary cache and a home-manager module, and
    `.deb`, `.rpm` and tarball releases.

</div>

## Quick start

=== "Arch"

    ```sh
    paru -S jotty-bin
    ```

=== "Nix"

    ```sh
    nix run github:justanoobcoder/jotty/cachix
    ```

=== "Debian / Ubuntu"

    Download `jotty_*.deb` from the
    [latest release](https://github.com/justanoobcoder/jotty/releases/latest), then:

    ```sh
    sudo apt install ./jotty_*.deb
    ```

=== "Fedora / openSUSE"

    Download `jotty-*.rpm` from the
    [latest release](https://github.com/justanoobcoder/jotty/releases/latest), then:

    ```sh
    sudo dnf install ./jotty-*.rpm
    ```

Every option has a default, so jotty needs no config file. To change
something, create `~/.config/jotty/config.toml` with only what you change:

```toml
[font]
family = "JetBrains Mono"
size = 12.0

[window]
opacity = 0.9
blur = true
```

See [Installing](user/installation.md) for every way to install jotty,
and the [user guide](user/index.md) for every option.
