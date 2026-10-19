#!/bin/sh
# Compile po/*.po into <target>/<lang>/LC_MESSAGES/lumen.mo.
# Usage: build-aux/compile-translations.sh [target-dir]   (default: target/locale)
# Then try one with:  LANGUAGE=es LUMEN_LOCALEDIR=target/locale cargo run
set -e
target="${1:-target/locale}"
for po in po/*.po; do
    [ -e "$po" ] || continue
    lang="$(basename "$po" .po)"
    mkdir -p "$target/$lang/LC_MESSAGES"
    msgfmt --check --output-file="$target/$lang/LC_MESSAGES/lumen.mo" "$po"
done
