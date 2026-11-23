#!/usr/bin/env python3
"""Checks that build-aux/flatpak/cargo-sources.json lists exactly the crates in Cargo.lock.

The Flatpak build is offline, so a crate missing from that file (or left in it after an
update) breaks it. Regenerate the file with flatpak-cargo-generator when this fails; see
build-aux/flatpak/README.md.
"""
import json
import re
import sys
from pathlib import Path

root = Path(__file__).resolve().parent.parent
lock = (root / "Cargo.lock").read_text()
wanted = set()
for block in lock.split("[[package]]")[1:]:
    name = re.search(r'^name = "(.*)"', block, re.M)
    version = re.search(r'^version = "(.*)"', block, re.M)
    source = re.search(r'^source = "(.*)"', block, re.M)
    if name and version and source and source.group(1).startswith("registry+"):
        wanted.add(f"{name.group(1)}-{version.group(1)}")

listed = set()
for entry in json.loads((root / "build-aux/flatpak/cargo-sources.json").read_text()):
    if entry.get("type") == "archive":
        m = re.search(r"/crates/[^/]+/(.+)\.crate$", entry["url"])
        if m:
            listed.add(m.group(1))

missing, extra = sorted(wanted - listed), sorted(listed - wanted)
if missing or extra:
    print(f"cargo-sources.json is out of date: {len(missing)} missing, {len(extra)} no longer needed")
    for name in missing[:20]:
        print("  missing:", name)
    for name in extra[:20]:
        print("  extra:  ", name)
    sys.exit(1)
print(f"cargo-sources.json matches Cargo.lock ({len(wanted)} crates)")
