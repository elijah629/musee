# musee

`musee` is a Rust CLI for organizing and repairing a FLAC music library.

## Commands

```bash
musee -s /mnt/nas/Music add /home/Music
musee -s /mnt/nas/Music add --apply /home/Music
musee -s /mnt/nas/Music add --encoding sonos-flac /home/Music
musee -s /mnt/nas/Music add --encoding sonos-flac --apply /home/Music
musee -s /mnt/nas/Music add --unreleased --encoding sonos-flac --apply /home/Kanye-Unreleased
musee -s /mnt/nas/Music dedupe
musee -s /mnt/nas/Music dedupe --apply
musee -s /mnt/nas/Music repair
musee -s /mnt/nas/Music repair --apply
musee -s /mnt/nas/Music tag genre --all
musee -s /mnt/nas/Music tag genre --all --apply
musee tag genre /home/Music/Artist/Album
musee tag genre /home/Music/file.flac --apply --retag
```

Dry-run is default. `--apply` performs changes.

`add --encoding sonos-flac` accepts common audio formats and transcodes each input
before it enters the library. The resulting FLAC is 16-bit mono/stereo at the
nearest of 44.1 or 48 kHz, uses 4096-sample blocks, includes minimum and maximum
frame sizes in STREAMINFO, stays below the 32 KB maximum frame size, and has a
100-point seek table. The original source is removed only after the validated
FLAC has been placed successfully. This profile requires `ffmpeg`, `ffprobe`, and
`metaflac`; they are included automatically in the Nix package.

`add --unreleased` forces imported tracks into `Ye/Unreleased`, writes missing
artist, album, and title tags, and uses the filename as title when none exists.
Use `--unreleased-artist <NAME>` to override the default artist (`Ye`). It can be
combined with `--encoding sonos-flac` for untagged MP3, WAV, M4A, or other inputs.

`dedupe` finds identical decoded FLAC audio using the PCM MD5 stored in
STREAMINFO, independent of tags and compression settings. It prefers the track's
canonical library path (then lexical order), removes duplicate track sidecars,
and cleans known album art/NFO files when an entire duplicate album becomes
redundant. Files without a valid PCM MD5 are reported as unverified and are never
removed.

`repair` extracts embedded `LYRIC`, `LYRICS`, `SYNCEDLYRICS`, and
`UNSYNCEDLYRICS` values into UTF-8 `.lrc` sidecars and removes those tags from the
FLAC files. Multiple lyric values are preserved, and existing `.lrc` content is
kept and merged before embedded tags are removed.

`tag genre` does a best-effort online album-genre lookup from MusicBrainz.
It chooses one genre per album, then writes that genre across the album's FLAC tracks.
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
