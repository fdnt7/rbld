{
  description = "rbld";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # The dev shell, `nix fmt` and the formatting check, shared with my
    # other projects; ./treefmt.nix adds to its formatters.
    devshell = {
      url = "github:fdnt7/devshell";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      devshell,
    }:
    devshell.lib.mkOutputs { inherit self nixpkgs; } (pkgs: {
      packages = with pkgs; [
        cargo
        clippy
        rust-analyzer
        rustc
        rustfmt
      ];
      # Where rust-analyzer finds the standard library's source
      env.RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
      treefmt = ./treefmt.nix;
    })
    // {
      homeModules = rec {
        rbld = import ./home-manager.nix self;
        default = rbld;
      };

      packages = nixpkgs.lib.genAttrs [ "x86_64-linux" ] (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          cargoToml = nixpkgs.lib.importTOML ./Cargo.toml;
        in
        {
          default = pkgs.rustPlatform.buildRustPackage {
            inherit (cargoToml.package) version;
            pname = cargoToml.package.name;
            src = self;
            cargoLock.lockFile = ./Cargo.lock;
          };
        }
      );
    };
}
