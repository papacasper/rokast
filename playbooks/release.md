# Release rokast

Goal: a GitHub Release `vX.Y.Z` with `rokast-X.Y.Z-x86_64-linux.tar.gz` (source + prebuilt desktop app),
`rokast-ui-x86_64-linux` and `SHA256SUMS`.

## Steps

1. Pick the version (semver: fix = patch, feature = minor). Add a `## X.Y.Z` section at the top of `CHANGELOG.md`
   (user-facing changes, short bullets). Commit and push it to main.
2. `scripts/release X.Y.Z`. It refuses unless on main, clean, in sync with origin, tag new, CHANGELOG section present;
   runs the Python + Rust tests; writes `VERSION` and `ui/Cargo.toml`; commits "Release vX.Y.Z" if anything changed; tags; pushes.
3. The tag push runs `.github/workflows/release.yml` (Ubuntu 22.04): re-checks tag == VERSION == Cargo.toml and the
   CHANGELOG section, tests, builds the UI, packages, publishes with that CHANGELOG section as notes.

## Verify (all must pass)

- `gh run list -w release -L1` -> `completed success`.
- `gh release view vX.Y.Z --json assets -q '.assets[].name'` lists the tarball, `rokast-ui-x86_64-linux`, `SHA256SUMS`.
- Download into an empty scratch dir, `sha256sum -c SHA256SUMS`, extract, `rokast-X.Y.Z/scripts/rokast version` prints X.Y.Z,
  and `ldd rokast-X.Y.Z/ui/target/release/rokast-ui | grep 'not found'` prints nothing on this laptop.

## If the workflow fails

Fix on main, then move the tag: `git tag -d vX.Y.Z; git push origin :refs/tags/vX.Y.Z`, delete a half-made release with
`gh release delete vX.Y.Z -y`, and run `scripts/release X.Y.Z` again (it skips the bump commit when nothing changed).

## Notes

- Prebuilt UI needs GTK >= 4.6 and glibc >= 2.35 (Ubuntu 22.04 build). The patched wpa_supplicant is NOT shipped:
  users build it (`scripts/build-supplicant`) against their own libnl/OpenSSL.
- Never add AI attribution to release commits or notes.
