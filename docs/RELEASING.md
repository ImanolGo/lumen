# Releasing

## Before tagging

1. Set the version in `Cargo.toml`, `build-aux/aur/lumen-viewer/PKGBUILD` (`pkgver`, and `pkgrel=1`) and add a
   `<release>` to `data/io.github.imanolgo.Lumen.metainfo.xml` with today's date.
2. Move the `Unreleased` entries in `CHANGELOG.md` under the new version, with the same date.
3. Run `make check` and, if the dependencies changed, `build-aux/update-cargo-sources.sh`.
4. Build the Flatpak once locally:

   ```sh
   flatpak-builder --user --install --force-clean build-dir build-aux/flatpak/io.github.imanolgo.Lumen.yaml
   flatpak run io.github.imanolgo.Lumen
   ```

5. Run the release workflow by hand (Actions → Release → Run workflow, or
   `gh workflow run release.yml`). It builds the tarball, the Flatpak bundle and the `.deb`
   without publishing anything; check that all three jobs pass.

## Tagging

```sh
git tag -a v0.1.0 -m "Lumen 0.1.0"
git push origin v0.1.0
```

The tag runs the release workflow again, which this time attaches the three files to a GitHub release.

## AUR

There are two packages: `lumen-viewer` (releases) and `lumen-viewer-git` (the main branch). The
project is Lumen, but the packages and the command are `lumen-viewer`: the AUR name `lumen` (and
`/usr/bin/lumen`) belongs to another program. Each AUR package is its own git repository;
publishing needs an AUR account with your SSH key, and the PKGBUILD must be reachable at the root
of a public git repository, which is what the AUR "Git Clone URL" in the submission form points at.

After the tag exists, update the checksum of the release package and regenerate `.SRCINFO`:

```sh
cd build-aux/aur/lumen-viewer
updpkgsums
makepkg -f                      # check that it builds
makepkg --printsrcinfo > .SRCINFO
```

Commit those changes here, then push the two files to the AUR:

```sh
git clone ssh://aur@aur.archlinux.org/lumen-viewer.git /tmp/aur-lumen-viewer
cp build-aux/aur/lumen-viewer/{PKGBUILD,.SRCINFO} /tmp/aur-lumen-viewer/
git -C /tmp/aur-lumen-viewer add PKGBUILD .SRCINFO
git -C /tmp/aur-lumen-viewer commit -m "Update to 0.4.2"
git -C /tmp/aur-lumen-viewer push
```

`lumen-viewer-git` is published the same way (`ssh://aur@aur.archlinux.org/lumen-viewer-git.git`).
It works out its version from git, so it only needs pushing again when the PKGBUILD itself changes.

## Debian and Ubuntu

The release workflow builds the `.deb` on Ubuntu 26.04. To build one locally on Ubuntu 26.04+ or
Debian testing: `make deb`. The package's dependencies come from `dpkg-shlibdeps`, so a `.deb`
built on one release targets that release and newer ones.
