#!/bin/sh
# Makes Lumen.app and a .dmg around it (on a Mac): bundle.sh BINARY VERSION OUTPUT.dmg
# Not signed with a certificate: only the ad-hoc signature that Apple silicon insists on.
set -eu
binary=$1 version=$2 dmg=$3
root=$(cd "$(dirname "$0")/../.." && pwd)
work=$(mktemp -d)
app="$work/dmg/Lumen.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$work/AppIcon.iconset"
cp "$binary" "$app/Contents/MacOS/lumen-rs"
chmod 755 "$app/Contents/MacOS/lumen-rs"
sed "s/@VERSION@/$version/g" "$root/build-aux/macos/Info.plist" > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"
icon() { cp "$root/data/icons/${1}x${1}/apps/io.github.imanolgo.LumenRs.png" "$work/AppIcon.iconset/$2.png"; }
icon 16 icon_16x16; icon 32 icon_16x16@2x; icon 32 icon_32x32; icon 64 icon_32x32@2x
icon 128 icon_128x128; icon 256 icon_128x128@2x; icon 256 icon_256x256; icon 512 icon_256x256@2x
icon 512 icon_512x512
iconutil -c icns "$work/AppIcon.iconset" -o "$app/Contents/Resources/AppIcon.icns"
cp "$root/LICENSE" "$app/Contents/Resources/LICENSE"
codesign --force --sign - "$app"
codesign --verify --strict "$app"
ln -s /Applications "$work/dmg/Applications"
hdiutil create -quiet -volname Lumen -srcfolder "$work/dmg" -format UDZO -ov "$dmg"
rm -rf "$work"
