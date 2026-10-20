# Releasing

A pushed `vX.Y.Z` tag runs `.github/workflows/release.yml`: build, unit
tests, the .deb, .rpm and tarball, install tests, then the GitHub Release
(created if the tag has none) with the files attached. Everything before
the tag and the AUR files after it is by hand.

The website (`site/`) redeploys on its own when `README.md`,
`docs/screenshots/`, `Cargo.toml` or `site/` change on `main`, so the
version bump updates the version in its footer; a release needs nothing
else for it.

## Steps

1. **Bump the version** in `Cargo.toml` and `flake.nix` (both `version =
   "X.Y.Z"`), then run `cargo build` to update `Cargo.lock`. Done when
   `cargo build --locked` passes. The workflow stops if either file
   disagrees with the tag.
2. **Commit, tag and push**: `git tag vX.Y.Z`, then
   `git push origin main vX.Y.Z`.
3. **Watch the run** (`gh run watch`). Done when the release lists
   `vidicon_amd64.deb`, `vidicon.x86_64.rpm`,
   `vidicon-X.Y.Z-x86_64-linux.tar.gz` and `SHA256SUMS`. The README links
   use `releases/latest/download` with fixed names, so it needs no edit.
4. **Update the AUR packages**, now that the tag's archive exists:
   - `packaging/aur/vidicon/PKGBUILD`: `pkgver=X.Y.Z`, `pkgrel=1`, and
     `sha256sums` from
     `curl -sL https://github.com/emzbtw/vidicon/archive/vX.Y.Z.tar.gz | sha256sum`.
   - `packaging/aur/vidicon-git/PKGBUILD`: `pkgver=X.Y.Z.r0.g0000000`.
   - Regenerate each `.SRCINFO` in its package directory:
     `nix shell nixpkgs#pacman -c sh -c 'makepkg --config "$(dirname "$(command -v makepkg)")/../etc/makepkg.conf" --printsrcinfo' > .SRCINFO`
     (makepkg looks for `/etc/makepkg.conf` otherwise).

   Done when both `.SRCINFO` files name X.Y.Z. Commit them; once the
   packages are on the AUR, push each package directory there too.

## Install tests

`packaging/test/install-test.sh` installs a package in a clean container
and checks that linked and dlopened libraries resolve, `--help` works and
a launch fails only for want of a display. The workflow runs it on the
oldest supported release of each distribution plus its `latest` image.

- **Oldest releases** are the ones the README's Install section names.
  At end of life, move both the workflow matrix and the README up: Fedora 43
  around December 2026 (its repos move to the archive and `dnf` fails),
  Ubuntu 22.04 in April 2027, Debian 12 around 2028.
- **A `latest` image failing** with no code change usually means a new
  distribution release; read the job log before blaming the package.
- **The dlopened library list** in the script mirrors the deb and rpm
  dependencies in `Cargo.toml`. Change all three together.
- **Locally**, run it in rootless podman from nixpkgs (no container
  runtime is installed). There is no `/etc/containers`, so pass
  `--signature-policy` a policy.json of
  `{"default":[{"type":"insecureAcceptAnything"}]}` on pull, and keep
  `--root`/`--runroot` in a scratch directory. Clear that storage with
  `podman --root ... --runroot ... system reset --force`; a plain `rm`
  hits files owned by the container's user IDs.

## Known limits

- **v0.1.0 cannot be rebuilt from its tag**: the packaging metadata in
  `Cargo.toml` came after it, so the Package step fails with
  "Missing field: package.metadata". Its release files are good as they are.
- **cargo-generate-rpm must stay at 0.21.1 or newer.** 0.21.0 writes weak
  glibc version needs as `libc.so.6(GLIBC_2.x)[WEAK](64bit)`, which no
  Fedora package provides, so `dnf` refuses the .rpm.
