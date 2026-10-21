self:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.programs.rbld;
in
{
  options.programs.rbld = {
    enable = lib.mkEnableOption ''
      rbld. It works on the flake given as its last argument, or else the
      one {option}`programs.nh.flake` names, as nh does
    '';

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = lib.literalExpression "rbld.packages.\${pkgs.stdenv.hostPlatform.system}.default";
      description = "The package providing the {command}`rbld` executable.";
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package ];
  };
}
