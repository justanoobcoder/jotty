# Development

## Environment

The flake's dev shell provides the Rust toolchain (cargo, rustc, clippy,
rustfmt, rust-analyzer), pkg-config, and the native libraries: wayland,
libxkbcommon, libGL (libglvnd) and fontconfig.

```sh
nix develop          # or: direnv allow  (the repo has an .envrc)
cargo build
cargo run -- -e htop
```

Without Nix, install the development packages for those four libraries and a
Rust toolchain of version 1.88 or newer.

The dev shell also compiles `extra/jotty.terminfo` into
`target/share/terminfo`, next to `target/{debug,release}/jotty`, so
`cargo run` uses `TERM=jotty`. Without the dev shell, jotty falls back to
`xterm-256color` unless the entry is installed elsewhere.

## Checks

Run these before committing. `nix flake check` runs all three in the
sandbox.

```sh
cargo test                                    # unit tests
cargo clippy --all-targets -- -D warnings     # lints
cargo fmt --check                             # formatting

nix flake check     # builds the package (with tests), clippy, rustfmt
nix fmt             # formats flake.nix
```

## Continuous integration

`.github/workflows/ci.yml` runs on pushes to `main` and on pull requests:

| Job | Runs |
| --- | --- |
| Format | `cargo fmt --check` and `nix fmt -- --check flake.nix` |
| Clippy | `cargo clippy --all-targets -- -D warnings` |
| Build and test | `cargo build --release` and `cargo test` on Ubuntu, with fonts installed so the font and emoji tests run |
| Minimum Rust version | `cargo build` with Rust 1.88, the `rust-version` in `Cargo.toml` |
| Nix flake check | `nix flake check`: the package with its tests, clippy and rustfmt in the sandbox, and the terminfo size check |
| Move the cachix branch | on `main` only, after every other job passes: points `cachix` at the commit |

Warnings fail every job. When raising `rust-version`, update the
toolchain in the minimum-version job too.

Builds of `main` upload the Nix build to the `justanoobcoder` Cachix cache
(token in the repository secret `CACHIX_AUTH_TOKEN`; pull requests only
pull). Once every job has passed, the `publish` job moves the `cachix`
branch to that commit. Users follow `cachix`, so it only ever points at a
cached build: never push to it by hand. A newer push to `main` cancels a
running build, so only the latest of a burst gets cached. The free cache
holds 5 GB; when full, Cachix drops the least recently used paths, which
only costs a rebuild to someone pinned to an old commit.

Pushes that touch only Markdown, `LICENSE`, `.gitignore`, `packaging/`,
`scripts/` or the release workflow skip CI (`paths-ignore`), so `cachix`
lags behind them until the next build, which is harmless: the package's
`src` holds only `Cargo.toml`, `Cargo.lock`, `src/`, `extra/` and
`docs/config.example.toml`. A file the build reads elsewhere must join that
fileset. The home-manager module is outside it too, but its changes do run
CI, or users on `cachix` would never get them.

## Releasing

1. Set `version` in `Cargo.toml` (and run `cargo build` to update
   `Cargo.lock`), commit, and push.
2. Tag the commit and push the tag:

   ```sh
   git tag v0.2.0
   git push origin v0.2.0
   ```

`.github/workflows/release.yml` then checks that the tag matches the
version, builds on Ubuntu 22.04 (for an old enough glibc) for x86_64 and
aarch64, and publishes a GitHub release with:

| File | For |
| --- | --- |
| `jotty_VERSION-1_ARCH.deb` | Debian, Ubuntu and derivatives (`cargo-deb`; dependencies detected from the binary) |
| `jotty-VERSION-1.ARCH.rpm` | Fedora, openSUSE, RHEL-likes (`cargo-generate-rpm`) |
| `jotty-VERSION-1-x86_64.pkg.tar.zst` | Arch Linux, built with `makepkg` from `packaging/arch/PKGBUILD` in an Arch container |
| `jotty-VERSION-ARCH-linux.tar.gz` | Anything else: `bin/jotty` with `share/terminfo`, the desktop entry and docs |
| `SHA256SUMS` | Checksums of all of the above |

A tag with a `-` (`v0.2.0-rc.1`) makes a pre-release. The package contents
are listed in `Cargo.toml` (`[package.metadata.deb]` and
`[package.metadata.generate-rpm]`) and in the PKGBUILD. The PKGBUILD also
works for the AUR: it downloads the tagged source from GitHub.

## Tests

The tests don't need a compositor, GPU or display:

| Area | What is tested |
| --- | --- |
| `terminal/tests.rs` | Escape sequences against expected grid contents: movement, erase, insert/delete, scroll regions, SGR, alternate screen, wide characters, grapheme clusters, charsets, reports (DECRQM, color queries), hyperlinks, resize and reflow, scrollback view, OSC 133, notifications (OSC 9, OSC 777), and a garbage-input fuzz test |
| `terminal/grid.rs` | Scrolling, history bounds, resize |
| `pty.rs` | Spawning `/bin/sh`, environment, window size, resize, input, controlling terminal, exit status |
| `input.rs` | Key encoding in all modes, shortcuts, paste encoding |
| `ime.rs` | Surrounding text from rows (including wide and multibyte characters), byte limits, delete-to-keys, event ordering |
| `renderer/mod.rs` | Color resolution and program color overrides, cursor styles, decorations, selection and preedit drawing, clusters, through a fake glyph cache |
| `renderer/atlas.rs` | Packing without overlap |
| `renderer/software.rs` | Pixel output of the CPU renderer: fills, translucency, glyph coverage, color glyphs, clipping, partial repaints and damage bands |
| `font.rs` | Cell metrics, box drawing, mark overlays, and loading the system monospace font (skipped when no fonts are installed, as in the Nix sandbox) |
| `emoji.rs` | Which text is drawn in color, scaling, and flag ligatures in the system emoji font (skipped without one) |
| `notify.rs` | D-Bus marshalling of `Notify`, parsing replies and signals in both byte orders, which clicks focus the window, bus addresses |
| `watch.rs` | Noticing a config file replaced by rename, ignoring other files, parsing inotify events |
| `links.rs`, `search.rs` | URL and hyperlink detection, trimming, wrapped lines, configured patterns; finding matches in both directions |
| `config.rs`, `cli.rs`, `selection.rs` | Parsing, validation, selection text, words, lines, following output |

The Wayland and GL glue (`app.rs`, `renderer/gl.rs`) is tested by running
jotty, as described next.

## Running jotty headless

`scripts/headless.sh` starts a headless sway with software rendering, runs
jotty in it, optionally drives it, and saves a screenshot. Nothing appears on
your screen, and it works over SSH or in CI.

```sh
cargo build
nix shell nixpkgs#sway nixpkgs#grim nixpkgs#wtype -c \
  scripts/headless.sh shot.png -- ./target/debug/jotty -e sh -c 'ls --color=always; sleep 5'
```

To type into it, write an actions script and pass it with `-a`:

```sh
cat > /tmp/actions.sh <<'EOF'
wtype -k Shift_L -s 200 'echo hello' -k Return -s 300
swaymsg output HEADLESS-1 scale 2
EOF
nix shell nixpkgs#sway nixpkgs#grim nixpkgs#wtype -c \
  scripts/headless.sh -a /tmp/actions.sh shot.png -- ./target/debug/jotty
```

Put the whole input sequence in a single `wtype` call (use `-s` for pauses).
Each `wtype` process creates and removes a virtual keyboard, and the keys it
sends right after the seat regains a keyboard can be lost.

The input method can be exercised the same way. sway relays
`text-input-v3` to an `input-method-v2` client, so a small client that sends
`set_preedit_string`, `commit_string` and `delete_surrounding_text` (and
prints the `surrounding_text` events it receives) tests the whole IME path.
Note that sway only sends `text_input.enter` while an input method is
connected.

## Debugging

- `RUST_LOG=debug` logs every escape sequence jotty doesn't handle.
- `WAYLAND_DEBUG=1` traces all Wayland messages. Combine it with `grep
  text_input` to debug the input method.
- To test the CPU renderer, set `renderer = "cpu"` in `[window]`. To test
  the GL renderer on Mesa's software rasterizer instead (on NixOS with an
  NVIDIA GPU), set `LIBGL_ALWAYS_SOFTWARE=1` and
  `__EGL_VENDOR_LIBRARY_FILENAMES=/run/opengl-driver/share/glvnd/egl_vendor.d/50_mesa.json`.
- To see what jotty receives from a program, record its output with
  `script -q /tmp/log` in another terminal, or run `cat -v` inside jotty.

## Code layout and conventions

See [architecture.md](architecture.md) for the module map. Some conventions:

- Keep Wayland and GL types out of the logic modules (`terminal`, `ime`,
  `input`, `selection`, `renderer::build`), so they stay unit-testable.
- When adding an escape sequence, add a test in `terminal/tests.rs` that
  feeds the bytes and checks the resulting grid.
- `unsafe` stays in `pty.rs`, `font.rs` (fontconfig, `mmap`), `watch.rs`
  (inotify, signals) and `renderer/gl.rs`,
  with a `SAFETY` comment where the reason isn't obvious.
- Imports come in three groups: std, other crates, then the crate's own.
  The nightly rustfmt options `imports_granularity = "Module"`,
  `group_imports = "StdExternalCrate"` and `hex_literal_case = "Lower"`
  keep them so; stable rustfmt can run them once with
  `RUSTC_BOOTSTRAP=1 cargo fmt -- --config imports_granularity=Module,group_imports=StdExternalCrate,hex_literal_case=Lower`.
- Commit one change at a time, and make sure tests, clippy and rustfmt pass
  first.
