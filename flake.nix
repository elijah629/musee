{
  description = "musee - Music library organizer and repair CLI";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];

      forAllSystems = f:
        nixpkgs.lib.genAttrs systems (system:
          f {
            inherit system;
            pkgs = import nixpkgs { inherit system; };
          });
    in
    {
      packages = forAllSystems ({ pkgs, ... }: rec {
        musee = pkgs.rustPlatform.buildRustPackage {
          pname = "musee";
          version = "0.1.0";

          src = pkgs.lib.cleanSourceWith {
            src = ./.;
            filter = path: type:
              let
                base = builtins.baseNameOf path;
              in
              !(base == "target" || base == "result" || base == ".git");
          };

          cargoLock = {
            lockFile = ./Cargo.lock;
          };

          meta = with pkgs.lib; {
            description = "Music library organizer and repair CLI";
            mainProgram = "musee";
            platforms = platforms.linux;
          };
        };

        default = musee;
      });

      apps = forAllSystems ({ pkgs, system }: rec {
        musee = {
          type = "app";
          program = "${self.packages.${system}.musee}/bin/musee";
        };

        default = musee;
      });

      overlays.default = final: prev: {
        musee = self.packages.${prev.system}.musee;
      };

      nixosModules.default = { config, lib, pkgs, ... }:
        let
          cfg = config.programs.musee;
        in
        {
          options.programs.musee = {
            enable = lib.mkEnableOption "musee CLI";

            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
              defaultText = lib.literalExpression "inputs.musee.packages.${pkgs.stdenv.hostPlatform.system}.default";
              description = "The musee package to install.";
            };
          };

          config = lib.mkIf cfg.enable {
            environment.systemPackages = [ cfg.package ];
          };
        };
    };
}
