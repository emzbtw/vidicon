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

- [ ] `nix build` puts the template under `result/share/vidicon/`, and
  Noctalia renders it from the profile path with `--list-templates`
  listing it.
- [ ] The deb, rpm and tarball from a release build hold it; the install
  test checks the deb and rpm.
- [ ] Both PKGBUILDs install it (`makepkg` in a container, or checked by
  reading if no Arch image is at hand).
- [ ] README and site updated; site builds.

## Out of scope

Noctalia's community catalog (a later step, `docs/milestones/themes.md`).
Changing the template itself.
