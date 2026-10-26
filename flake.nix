{
  description = "jotty - a minimal, Wayland-native, GPU-accelerated terminal emulator";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      # Libraries linked at build time and needed at run time.
      runtimeLibs =
        pkgs: with pkgs; [
          wayland
          libxkbcommon
          libGL
          fontconfig
        ];
    in
    {
      packages = forAllSystems (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "jotty";
          version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;
          src = pkgs.lib.cleanSource ./.;
          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = [
            pkgs.pkg-config
            pkgs.ncurses # tic
          ];
          buildInputs = runtimeLibs pkgs;

          # jotty finds the entry next to its executable and sets TERM=jotty.
          postInstall = ''
            mkdir -p $out/share/terminfo
            tic -x -o $out/share/terminfo extra/jotty.terminfo
            # unibilium (Neovim) refuses entries of 4096 bytes or more.
            size=$(stat -c %s $out/share/terminfo/j/jotty)
            if [ "$size" -ge 4096 ]; then
              echo "terminfo entry is $size bytes, must stay under 4096" >&2
              exit 1
            fi
          '';

          # libEGL is provided by libglvnd, which loads the driver from
          # /run/opengl-driver at run time; make sure the loader is found.
          postFixup = ''
            patchelf --add-rpath ${pkgs.lib.makeLibraryPath (runtimeLibs pkgs)} $out/bin/jotty
          '';

          meta = {
            description = "Minimal Wayland-native GPU-accelerated terminal emulator";
            license = pkgs.lib.licenses.mit;
            mainProgram = "jotty";
            platforms = pkgs.lib.platforms.linux;
          };
        };
      });

      apps = forAllSystems (pkgs: {
        default = {
          type = "app";
          program = "${self.packages.${pkgs.stdenv.hostPlatform.system}.default}/bin/jotty";
          meta.description = "Run the jotty terminal";
        };
      });

      checks = forAllSystems (
        pkgs:
        let
          package = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
        in
        {
          # Builds and runs the test suite.
          build = package;

          clippy = package.overrideAttrs (old: {
            pname = "jotty-clippy";
            nativeBuildInputs = old.nativeBuildInputs ++ [ pkgs.clippy ];
            buildPhase = "cargo clippy --all-targets --offline -- -D warnings";
            doCheck = false;
            installPhase = "touch $out";
            postFixup = "";
          });

          fmt = pkgs.runCommand "jotty-fmt" { nativeBuildInputs = [ pkgs.rustfmt ]; } ''
            rustfmt --check --edition 2024 ${./src}/main.rs
            touch $out
          '';
        }
      );

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            clippy
            rustfmt
            rust-analyzer
            pkg-config
            ncurses
          ];
          buildInputs = runtimeLibs pkgs;
          # Compile the terminfo entry next to target/{debug,release}/jotty,
          # where jotty looks for it, so `cargo run` uses TERM=jotty.
          shellHook = ''
            mkdir -p target/share/terminfo
            tic -x -o "$PWD/target/share/terminfo" extra/jotty.terminfo 2>/dev/null || true
          '';
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (runtimeLibs pkgs);
          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
        };
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt);
    };
}
