# Flatpak

`io.github.imanolgo.Lumen.yml` builds Lumen against the freedesktop 24.08 runtime with
the Rust SDK extension, entirely offline. Flatpak builds cannot download crates, so the
manifest lists them in `cargo-sources.json`, which is generated from `Cargo.lock`
and committed: **regenerate it whenever `Cargo.lock` changes**, or the build will fail.

```sh
# offline, from Cargo.lock alone:
python3 tools/update-flatpak-sources.py
# or flatpak-builder-tools' generator (needs python3, aiohttp and tomlkit)
curl -LO https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py
python3 flatpak-cargo-generator.py Cargo.lock -o build-aux/flatpak/cargo-sources.json

flatpak install --user flathub org.freedesktop.Platform//24.08 org.freedesktop.Sdk//24.08 \
    org.freedesktop.Sdk.Extension.rust-stable//24.08
flatpak-builder --user --install --force-clean build-dir build-aux/flatpak/io.github.imanolgo.Lumen.yml
flatpak run io.github.imanolgo.Lumen
```

Notes:

- Files are opened and saved through the desktop portals; the manifest only gives read access
  to Pictures and Documents (for flipping through a folder next to the file you opened).
- The decoder sandbox (Landlock and seccomp) only restricts the worker process further, so it
  should work inside Flatpak's own sandbox; that has not been tested. If it cannot start,
  Lumen decodes in-process and says so in the log.
- This manifest, with its generated sources, has been checked with `flatpak-builder
  --show-manifest`, but it has not been built yet: that needs the runtime and SDK (about a
  gigabyte).
