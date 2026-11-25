# Releasing

Version 0.1.0 was tagged on 24 November 2026. The AUR packages and the Flatpak are not published
yet. This file is the checklist for a release.

## Before tagging

1. Set the version in `Cargo.toml` (and run `cargo build` so `Cargo.lock` follows), in
   `build-aux/aur/lumen-rs/PKGBUILD` (`pkgver`, `pkgrel=1`), and add a `<release>` with today's date
   to `data/io.github.imanolgo.LumenRs.metainfo.xml`.
2. Move the `Unreleased` entries in `CHANGELOG.md` under the new version, with the same date.
3. If `Cargo.lock` changed, regenerate the Flatpak sources and check them:

   ```sh
   python3 tools/update-flatpak-sources.py
   python3 tools/check-flatpak-sources.py
   ```

   (`tools/update-flatpak-sources.py` rebuilds the file from `Cargo.lock` alone, offline. The upstream `flatpak-cargo-generator.py`, which writes the same layout, is in [flatpak-builder-tools](https://github.com/flatpak/flatpak-builder-tools/tree/master/cargo); it needs `aiohttp` and `tomlkit`.)
4. Run `make test` and `make lint`, and check that CI is green on `main`, including the `packaging`, `aur` and `audit` jobs.
5. Build the Flatpak once, and try it (it has never been built):

   ```sh
   flatpak-builder --user --install --force-clean build-dir build-aux/flatpak/io.github.imanolgo.LumenRs.yml
   flatpak run io.github.imanolgo.LumenRs
   ```

6. Run the real thing: `make install PREFIX=$HOME/.local`, then open a folder, a PDF and a form.
   Update the screenshots in `data/screenshots/` if the look changed (see [DEVELOPMENT.md](DEVELOPMENT.md)).

## Tagging

```sh
git tag -a v0.1.0 -m "Lumen (Rust/iced) 0.1.0"
git push origin v0.1.0
```

The tag runs `.github/workflows/release.yml`: it checks that the tag matches `Cargo.toml`, runs the
tests, builds an x86_64 tarball (the `make install DESTDIR=… PREFIX=/usr` layout) and a `.deb`
(`cargo deb`), writes `SHA256SUMS`, and creates a GitHub release with the three. It ran for
v0.1.0 and published all three.

## AUR

There are two packages in `build-aux/aur/`: `lumen-rs` (a tagged release) and `lumen-rs-git` (the
latest commit). Each AUR package is its own git repository, and publishing needs an AUR account
with your SSH key. After the tag exists:

```sh
cd build-aux/aur/lumen-rs
updpkgsums                              # fills in the tarball's checksum (it is SKIP until then)
makepkg --printsrcinfo > .SRCINFO
makepkg -si                             # a real build and install, to check it
```

Then push `PKGBUILD` and `.SRCINFO` to `ssh://aur@aur.archlinux.org/lumen-rs.git`. For `lumen-rs-git`
only `.SRCINFO` needs regenerating when its PKGBUILD changes; its version is computed at build time.
CI checks that each `.SRCINFO` matches its PKGBUILD. Check first that the name `lumen-rs` is free on the AUR.

## Flatpak

`build-aux/flatpak/` has the manifest and the generated Cargo sources. Submitting to Flathub needs
the manifest to build from a tagged git source instead of `type: dir` (the comment in the manifest
shows how) and its own review; see Flathub's submission guide.

## After releasing

- Install the release package and run it.
- Move on: add an empty `## [Unreleased]` section to `CHANGELOG.md`.
