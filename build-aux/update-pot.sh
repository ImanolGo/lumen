#!/bin/sh
# Regenerate po/lumen.pot from the sources and merge it into every translation.
# Run from the repository root.
set -e
xgettext --from-code=UTF-8 --language=Rust \
    --keyword=tr --keyword=n_ --keyword=tr_fmt:1 \
    --package-name=lumen --copyright-holder="Imanol Gomez" \
    --msgid-bugs-address="https://github.com/ImanolGo/lumen/issues" \
    --add-comments=TRANSLATORS \
    --files-from=po/POTFILES.in --output=po/lumen.pot
for po in po/*.po; do
    [ -e "$po" ] && msgmerge --update --backup=none --quiet "$po" po/lumen.pot
done
echo "po/lumen.pot has $(grep -c '^msgid ' po/lumen.pot) entries"
