# Release rokast

Goal: a GitHub Release `vX.Y.Z` with `rokast-X.Y.Z-x86_64-linux.tar.gz` (source + prebuilt desktop app),
`rokast-ui-x86_64-linux`, `rokast_X.Y.Z_amd64.deb`, `rokast-X.Y.Z-1.x86_64.rpm`, `SHA256SUMS` and `SHA256SUMS.sig`;
then the AUR packages updated.

## Steps

1. Pick the version (semver: fix = patch, feature = minor). Add a `## X.Y.Z` section at the top of `CHANGELOG.md`
   (user-facing changes, short bullets; rename `## Unreleased` and drop its reminder line if there is one). Commit and push it to main.
2. `scripts/release X.Y.Z`. It refuses unless on main, clean, in sync with origin, tag new, CHANGELOG section present;
   runs the Rust tests (workspace + desktop app); writes `VERSION` and `ui/Cargo.toml`; commits "Release vX.Y.Z" if anything changed; tags; pushes.
3. The tag push runs `.github/workflows/release.yml` (Ubuntu 22.04): re-checks tag == VERSION == Cargo.toml and the
   CHANGELOG section, tests, builds the UI, builds the .deb and install-tests it in a clean ubuntu:22.04 container
   (`tests/test_pkg_deb.sh`), the same for the .rpm in fedora:43, packages, signs `SHA256SUMS` with the release key
   (repo secret `ROKAST_SIGNING_KEY`, `scripts/sign-release`), publishes with that CHANGELOG section as notes.
   The key and how to rotate it: vault note "rokast release signing key"; the public keys are `config/release-signers`.
4. AUR (below), once the release is verified.

## Verify (all must pass)

- `gh run list -w release -L1` -> `completed success`.
- `gh release view vX.Y.Z --json assets -q '.assets[].name'` lists the tarball, `rokast-ui-x86_64-linux`, the `.deb`, the `.rpm`,
  `SHA256SUMS`, `SHA256SUMS.sig`.
- Download into an empty scratch dir; `ssh-keygen -Y verify -f config/release-signers -I rokast-release -n rokast-release
  -s SHA256SUMS.sig < SHA256SUMS` prints `Good "rokast-release" signature`; `sha256sum -c SHA256SUMS`; extract, `rokast-X.Y.Z/scripts/rokast version` prints X.Y.Z,
  and `ldd rokast-X.Y.Z/ui/target/release/rokast-ui | grep 'not found'` prints nothing on this laptop.

## AUR (`rokast` and `rokast-git`)

The PKGBUILDs live in `packaging/aur/<pkg>/`; this repo is their source of truth, the AUR repos are copies.

1. `packaging/aur/update X.Y.Z`: sets pkgver, pkgrel=1 and the release archive's sha256 in `rokast/PKGBUILD`,
   regenerates both `.SRCINFO`.
2. `bash tests/test_pkg_arch.sh rokast` (docker; `sudo systemctl start docker` if it is not running) -> `all ok`:
   clean-container makepkg with check(), namcap, install, doctor from `/usr/bin`. It builds from the working tree, so
   step 1's sha256 is not exercised here; makepkg in step 4 checks it.
3. Commit "AUR: rokast X.Y.Z" and push to main.
4. Push to the AUR, **only with the user's go-ahead** (their AUR account and SSH key):
   `git clone ssh://aur@aur.archlinux.org/rokast.git <scratch>/aur-rokast`, copy `PKGBUILD` and `.SRCINFO` in,
   `makepkg --verifysource` there (checks the sha256 against the published archive), commit "Update to X.Y.Z", push.
   `rokast-git` only needs a push when its PKGBUILD changes (its pkgver comes from git).
5. Verify: `curl -s 'https://aur.archlinux.org/rpc/v5/info?arg[]=rokast' | python3 -I -m json.tool` shows Version X.Y.Z-1.

## If the workflow fails

Fix on main, then move the tag: `git tag -d vX.Y.Z; git push origin :refs/tags/vX.Y.Z`, delete a half-made release with
`gh release delete vX.Y.Z -y`, and run `scripts/release X.Y.Z` again (it skips the bump commit when nothing changed).

## Notes

- Prebuilt UI needs GTK >= 4.6 and glibc >= 2.35 (Ubuntu 22.04 build). The tarball does NOT ship the patched
  wpa_supplicant (users build it against their own libnl/OpenSSL); the .deb and the AUR packages do, in
  `/usr/lib/rokast/wpa_supplicant`, built from the w1.fi 2.11 tarball (sha256 pinned in `packaging/deb/build-deb` and
  the PKGBUILDs; keep them equal).
- The .deb is also tested on ubuntu:24.04 and debian:12: `ROKAST_DEB_IMAGE=debian:12 bash tests/test_pkg_deb.sh <deb>`.
- Never add AI attribution to release commits or notes.
