#!/bin/sh
# Build a .deb of Lumen. Run from the repository root on the Debian or Ubuntu release you
# are packaging for: it needs GNOME 49-era libraries (libadwaita 1.8, poppler-glib 25.06),
# so Ubuntu 26.04 or later, or Debian testing.
#
#   sudo apt install build-essential dpkg-dev gettext libgtk-4-dev libadwaita-1-dev \
#       libpoppler-glib-dev libseccomp-dev   # plus Rust 1.93 or newer, e.g. from rustup
#   build-aux/debian/build-deb.sh    # writes lumen-viewer_<version>-1_<arch>.deb
#
# The package's dependencies are worked out by dpkg-shlibdeps from the libraries the
# binary links against, so they match the release it was built on.
set -eu

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
revision=${DEB_REVISION:-1}
arch=$(dpkg --print-architecture)
out=${1:-lumen-viewer_${version}-${revision}_${arch}.deb}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
stage=$work/stage

make build translations
make install-files DESTDIR="$stage" PREFIX=/usr
strip --strip-unneeded "$stage/usr/bin/lumen-viewer"

docs=$stage/usr/share/doc/lumen-viewer
install -d "$docs"
cat > "$docs/copyright" <<EOF
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: Lumen
Source: https://github.com/ImanolGo/lumen

Files: *
Copyright: Imanol Gomez
License: GPL-3.0-or-later
 On Debian systems, the full text of the GNU General Public License version 3
 can be found in /usr/share/common-licenses/GPL-3.
EOF
gzip -9n -c CHANGELOG.md > "$docs/changelog.gz"

# dpkg-shlibdeps wants to run next to a debian/control.
mkdir -p "$work/src/debian"
printf 'Source: lumen-viewer\n\nPackage: lumen-viewer\nArchitecture: any\n' > "$work/src/debian/control"
depends=$(cd "$work/src" && dpkg-shlibdeps -O "$stage/usr/bin/lumen-viewer" | sed -n 's/^shlibs:Depends=//p')

install -d "$stage/DEBIAN"
cat > "$stage/DEBIAN/control" <<EOF
Package: lumen-viewer
Version: $version-$revision
Architecture: $arch
Maintainer: Imanol Gomez <imanol@steinergomez.com>
Installed-Size: $(du -sk "$stage/usr" | cut -f1)
Depends: $depends
Recommends: glycin-loaders
Section: graphics
Priority: optional
Homepage: https://github.com/ImanolGo/lumen
Description: fast image and PDF viewer with light editing and markup
 Lumen opens pictures and PDF documents in one native GNOME app. It rotates,
 flips, crops, resizes and adjusts the colour of pictures, marks up pictures
 and PDFs with shapes, arrows, text and highlights, and rearranges, merges
 and exports PDF pages. Large files load in the background, so the window
 never freezes.
EOF

dpkg-deb --root-owner-group -Zxz --build "$stage" "$out"
echo "wrote $out"
