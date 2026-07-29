self:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.programs.rbld;
  tomlFormat = pkgs.formats.toml { };
in
{
  options.programs.rbld = {
    enable = lib.mkEnableOption "rbld";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = lib.literalExpression "rbld.packages.\${pkgs.stdenv.hostPlatform.system}.default";
      description = "The package providing the {command}`rbld` executable.";
    };

    settings = lib.mkOption {
      type = tomlFormat.type;
      default = { };
      example = lib.literalExpression ''
        {
          flake = "''${config.home.homeDirectory}/nix";
        }
      '';
      description = ''
        Configuration written as TOML to {file}`$XDG_CONFIG_HOME/rbld/config.toml`,
        which {command}`rbld` reads unless `--config` points it at another file.

        A command-line argument overrides whatever the config sets for it.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package ];

    # an empty config would only be a file for rbld to read nothing out of
    xdg.configFile."rbld/config.toml" = lib.mkIf (cfg.settings != { }) {
      source = tomlFormat.generate "rbld-config.toml" cfg.settings;
    };
  };
}
