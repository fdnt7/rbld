{ lib, ... }:
{
  programs = {
    nixfmt.enable = true;
    rustfmt.enable = true;
    shfmt = {
      enable = true;
      includes = [ ".envrc" ];
    };
    taplo.enable = true;
    typos.enable = true;
  };

  # exclude `--write-changes` from options so it doesn't automatically fix typos
  # because it could break code
  settings.formatter.typos.options = lib.mkForce [ "--force-exclude" ];
}
