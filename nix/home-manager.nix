# home-manager module: programs.jotty.
#
# The flake applies this to its own package, so `package` defaults to the
# jotty built from the same revision as the module.
{ self }:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.programs.jotty;
  tomlFormat = pkgs.formats.toml { };
in
{
  options.programs.jotty = {
    enable = lib.mkEnableOption "jotty, an opinionated Wayland-native terminal emulator";

    package = lib.mkOption {
      type = lib.types.nullOr lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = lib.literalExpression "jotty.packages.\${pkgs.stdenv.hostPlatform.system}.default";
      description = ''
        The jotty package to install. Set to `null` to only write the
        configuration, for a jotty installed some other way.
      '';
    };

    settings = lib.mkOption {
      inherit (tomlFormat) type;
      default = { };
      example = lib.literalExpression ''
        {
          font = {
            family = "JetBrains Mono";
            size = 12.0;
          };
          window = {
            opacity = 0.9;
            blur = true;
          };
          keybinds.copy_or_interrupt = [ "ctrl+c" ];
        }
      '';
      description = ''
        Configuration written to {file}`$XDG_CONFIG_HOME/jotty/config.toml`.
        See <https://github.com/justanoobcoder/jotty/blob/main/docs/configuration.md>
        for the options. jotty reloads the file when home-manager replaces it.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = lib.mkIf (cfg.package != null) [ cfg.package ];

    xdg.configFile."jotty/config.toml" = lib.mkIf (cfg.settings != { }) {
      source = tomlFormat.generate "jotty-config.toml" cfg.settings;
    };
  };
}
