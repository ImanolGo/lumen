#!/usr/bin/env python3
"""Rewrites build-aux/flatpak/cargo-sources.json from Cargo.lock, with no network and no
third-party code: each crate becomes an archive source (with the checksum Cargo.lock records)
plus the inline `.cargo-checksum.json` cargo's vendored-sources mode needs. It writes the same
layout flatpak-cargo-generator does for crates.io dependencies; run tools/check-flatpak-sources.py
afterwards. Dependencies from git or other sources are not supported (there are none).
"""
import json
import re
import sys
from pathlib import Path

root = Path(__file__).resolve().parent.parent
lock = (root / "Cargo.lock").read_text()
packages = []
for block in lock.split("[[package]]")[1:]:
    field = lambda key: (m.group(1) if (m := re.search(rf'^{key} = "(.*)"', block, re.M)) else None)
    name, version, source, checksum = field("name"), field("version"), field("source"), field("checksum")
    if not source:
        continue  # the application itself
    if source != "registry+https://github.com/rust-lang/crates.io-index":
        sys.exit(f"{name} {version} comes from {source}; this script only handles crates.io")
    if not checksum:
        sys.exit(f"{name} {version} has no checksum in Cargo.lock")
    packages.append((name, version, checksum))

sources = []
for name, version, checksum in sorted(set(packages)):
    dest = f"cargo/vendor/{name}-{version}"
    sources.append({
        "type": "archive",
        "archive-type": "tar-gzip",
        "url": f"https://static.crates.io/crates/{name}/{name}-{version}.crate",
        "sha256": checksum,
        "dest": dest,
    })
    sources.append({
        "type": "inline",
        "contents": json.dumps({"package": checksum, "files": {}}),
        "dest": dest,
        "dest-filename": ".cargo-checksum.json",
    })
sources.append({
    "type": "inline",
    "contents": '[source.vendored-sources]\ndirectory = "cargo/vendor"\n\n[source.crates-io]\nreplace-with = "vendored-sources"\n',
    "dest": "cargo",
    "dest-filename": "config",
})
out = root / "build-aux/flatpak/cargo-sources.json"
out.write_text(json.dumps(sources, indent=4) + "\n")
print(f"wrote {out.relative_to(root)}: {len(packages)} crates")
