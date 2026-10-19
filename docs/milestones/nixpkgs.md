# Milestone: nixpkgs

Parked 2026-10-20. Revisit once the package has matured: a few releases
out, the settings and database formats stable or migrating cleanly, and
GitHub issues being answered. No tickets yet. Below is a sketch, not
settled.

## Why

Every Nix user compiles Vidicon, test suite included, because no binary
cache holds the flake's package. In nixpkgs, Hydra builds it and
cache.nixos.org serves it. It also gives the shortest install
(`pkgs.vidicon`, `nix-shell -p vidicon`) and a listing on
search.nixos.org and Repology.

## Starting point

- The name is free: nixpkgs has no `vidicon` (checked 2026-10-20).
- nixpkgs already has IPTV players (`iptvnator`, `hypnotix`) and an IPTV
  proxy (`tuliprox`). Vidicon ships no channels and points to none
  (README Disclaimer).
- The flake's package already builds in the Nix sandbox with its tests;
  the only quirk is `cacert` for the tests' local servers.

## Proposed

- `pkgs/by-name/vi/vidicon/package.nix`: `rustPlatform.buildRustPackage`
  from the release tag with `fetchFromGitHub` and pinned hashes, in place
  of the flake's local `fileset`. The wrapper (runtime libraries on
  `LD_LIBRARY_PATH`, mpv and ffmpeg last on `PATH`, Inter on
  `SLINT_FONT_PATH`) and `postInstall` carry over from `flake.nix`.
- `meta`: description, homepage, `licenses.gpl3Plus`, `mainProgram`,
  `platforms.linux`, and the user as maintainer, added to
  `maintainers/maintainer-list.nix` in its own commit.
- `passthru.updateScript = nix-update-script { }`, so the update bot
  opens a pull request for each release.
- `vendor/femtovg` is a path dependency inside the source tree, so it
  needs nothing extra; check this when packaging.
- Aim to land before a NixOS release branches off (26.11 in late
  November 2026, or 27.05) so it reaches the stable channel too.
- Read nixpkgs' contributing guide at the time, including any rules on
  AI-generated contributions. They would apply to the `package.nix`
  submitted, which the user writes or reviews.

## Afterwards

- README NixOS section leads with `pkgs.vidicon`; the flake stays for
  following `main`.
- `docs/releasing.md`: the update bot's pull request to review after each
  release.
