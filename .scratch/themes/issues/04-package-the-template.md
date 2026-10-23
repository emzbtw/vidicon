# 04: Ship the Noctalia template in the packages

Status: open. Blocked by: 03.

## Goal

An installed Vidicon carries its Noctalia template, so setup is the
`[theme.templates.user.vidicon]` lines in Noctalia's config and nothing
to download.

## Context

`contrib/noctalia/vidicon.toml` and the README's Themes section (03).
The packages install `assets/vidicon.desktop` and `assets/vidicon.svg`
in four places, which this ticket extends the same way:
`Cargo.toml` (`[package.metadata.deb]` and
`[package.metadata.generate-rpm]` assets), `flake.nix` (`postInstall`,
and the source filter, which lists `./assets` but not `./contrib`),
`packaging/aur/vidicon/PKGBUILD` and `vidicon-git/PKGBUILD`, and the
tarball step in `.github/workflows/release.yml`. `docs/releasing.md`
for the release and its install tests.

## Scope

- Install the template as `share/vidicon/noctalia/vidicon.toml`
  (proposed; settle here): `/usr/share/...` for deb, rpm and AUR,
  `$out/share/...` for Nix, and in the tarball beside the desktop file.
- README Themes: `input_path` points at the installed file
  (`/usr/share/vidicon/noctalia/vidicon.toml`; on NixOS the profile
  path, `/run/current-system/sw/share/...` or
  `~/.nix-profile/share/...`), with downloading `contrib/` kept for
  builds from source. Check Noctalia reads the template through the
  profile's symlink into the store.
- The install test checks the file is there.

## Acceptance

- [x] `nix build` puts the template under `result/share/vidicon/`, and
  Noctalia renders it from the profile path with `--list-templates`
  listing it. (2026-10-23: `nix profile add --profile` into a scratch
  profile; Noctalia 5.3 listed it under User templates and rendered it
  through the profile's symlink into the store.)
- [ ] The deb, rpm and tarball from a release build hold it; the install
  test checks the deb and rpm. (2026-10-23: a local cargo-deb and
  cargo-generate-rpm 0.21.1 build both hold it, and the install test
  checks for it. The tarball and the install test run only in the
  release workflow: tick this after the next release run.)
- [x] Both PKGBUILDs install it (`makepkg` in a container, or checked by
  reading if no Arch image is at hand). (Checked by reading; `git
  archive` includes `contrib/`. `vidicon` builds only from the next tag:
  v0.2.0 has no `contrib/`.)
- [x] README and site updated; site builds.
- [x] Review findings fixed. (Kept: the tarball names the file
  `noctalia-template.toml`, the name the README has users save it as,
  since a bare `vidicon.toml` at its root reads as Vidicon's config.)

## Out of scope

Noctalia's community catalog (a later step, `docs/milestones/themes.md`).
Changing the template itself.
