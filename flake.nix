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
        # what rbld runs besides nh, nix and git, which belong to the system
        dix
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

            nativeBuildInputs = [
              pkgs.installShellFiles
              pkgs.makeWrapper
            ];
            # the completions come of rbld's own commands, so it writes them,
            # before it is wrapped
            #
            # nh, nix and git are the system's own, and rbld uses whichever
            # the system has; dix is rbld's alone
            postInstall = ''
              installShellCompletion --cmd rbld \
                --bash <($out/bin/rbld completions bash) \
                --fish <($out/bin/rbld completions fish) \
                --zsh <($out/bin/rbld completions zsh)
              wrapProgram $out/bin/rbld --prefix PATH : ${nixpkgs.lib.makeBinPath [ pkgs.dix ]}
            '';
          };
        }
      );
    };
}
