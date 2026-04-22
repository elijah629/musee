# musee

`musee` is a Rust CLI for organizing and repairing a FLAC music library.

## Commands

```bash
musee -s /mnt/nas/Music add /home/Music
musee -s /mnt/nas/Music add --apply /home/Music
musee -s /mnt/nas/Music repair
musee -s /mnt/nas/Music repair --apply
musee -s /mnt/nas/Music tag genre --all
musee -s /mnt/nas/Music tag genre --all --apply
musee tag genre /home/Music/Artist/Album
musee tag genre /home/Music/file.flac --apply --retag
```

Dry-run is default. `--apply` performs changes.

`tag genre` does a best-effort online genre lookup from MusicBrainz.
By default it skips files that already have a `GENRE` tag. Use `--retag` to replace existing genres.

## Install with flakes

Run directly:

```bash
nix run github:elijah629/musee -- --help
```

Add to a flake:

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    musee.url = "github:elijah629/musee";
  };

  outputs = { self, nixpkgs, musee, ... }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
    in
    {
      packages.${system}.default =
        musee.packages.${pkgs.stdenv.hostPlatform.system}.default;
    };
}
```

For NixOS or Home Manager, this direct form also works:

```nix
environment.systemPackages = [
  inputs.musee.packages.${pkgs.stdenv.hostPlatform.system}.default
];
```

Or install on NixOS:

```nix
{
  inputs.musee.url = "github:elijah629/musee";

  outputs = { self, nixpkgs, musee, ... }: {
    nixosConfigurations.my-host = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        musee.nixosModules.default
        {
          programs.musee.enable = true;
          # optional, explicit package selection:
          # programs.musee.package =
          #   inputs.musee.packages.${pkgs.stdenv.hostPlatform.system}.default;
        }
      ];
    };
  };
}
```

## Build locally

```bash
nix build .#musee
./result/bin/musee --help
```
