#!/bin/sh
# Regenerate build-aux/flatpak/cargo-sources.json from Cargo.lock, so the Flatpak can build offline.
# Run from the repository root after changing dependencies.
set -e
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
curl -sSfL -o "$work/generator.py" \
    https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py
python3 -m venv "$work/venv"
"$work/venv/bin/pip" install -q aiohttp tomlkit
"$work/venv/bin/python" "$work/generator.py" Cargo.lock -o build-aux/flatpak/cargo-sources.json
echo "updated build-aux/flatpak/cargo-sources.json"
