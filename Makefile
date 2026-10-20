# Build and install Lumen.
#
#   make                       build the release binary
#   make install               install under /usr (override with PREFIX, stage with DESTDIR)
#   make install PREFIX=/app   what the Flatpak does
#   make deb                   build a .deb (on Debian or Ubuntu, see build-aux/debian/)
#
# Packagers: `make install DESTDIR="$pkgdir"`.

PREFIX  ?= /usr
DESTDIR ?=
ID      := io.github.imanolgo.Lumen
BIN     := target/release/lumen
# The project is Lumen, but it is installed as `lumen-viewer`: the AUR name `lumen` and the
# command `lumen` belong to another program, and packages cannot share a file.
APP     := lumen-viewer
CARGO   ?= cargo

.PHONY: all build translations install install-files uninstall check deb

all: build

build:
	$(CARGO) build --release --locked

translations:
	./build-aux/compile-translations.sh target/locale

install: build translations install-files

# Only copy files into place (the Flatpak builds first, offline, then calls this).
install-files:
	install -Dm755 $(BIN) $(DESTDIR)$(PREFIX)/bin/$(APP)
	install -Dm644 data/$(ID).desktop $(DESTDIR)$(PREFIX)/share/applications/$(ID).desktop
	install -Dm644 data/$(ID).metainfo.xml $(DESTDIR)$(PREFIX)/share/metainfo/$(ID).metainfo.xml
	install -Dm644 data/icons/hicolor/scalable/apps/$(ID).svg $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/$(ID).svg
	for size in 16 32 48 64 128 256 512; do \
		install -Dm644 data/icons/hicolor/$${size}x$${size}/apps/$(ID).png $(DESTDIR)$(PREFIX)/share/icons/hicolor/$${size}x$${size}/apps/$(ID).png; \
	done
	install -Dm644 data/icons/hicolor/symbolic/apps/$(ID)-symbolic.svg $(DESTDIR)$(PREFIX)/share/icons/hicolor/symbolic/apps/$(ID)-symbolic.svg
	for mo in target/locale/*/LC_MESSAGES/lumen.mo; do \
		[ -e "$$mo" ] || continue; \
		lang=$$(basename $$(dirname $$(dirname "$$mo"))); \
		install -Dm644 "$$mo" "$(DESTDIR)$(PREFIX)/share/locale/$$lang/LC_MESSAGES/lumen.mo"; \
	done

uninstall:
	rm -f $(DESTDIR)$(PREFIX)/bin/$(APP)
	rm -f $(DESTDIR)$(PREFIX)/share/applications/$(ID).desktop
	rm -f $(DESTDIR)$(PREFIX)/share/metainfo/$(ID).metainfo.xml
	rm -f $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/$(ID).svg
	rm -f $(DESTDIR)$(PREFIX)/share/icons/hicolor/*x*/apps/$(ID).png
	rm -f $(DESTDIR)$(PREFIX)/share/icons/hicolor/symbolic/apps/$(ID)-symbolic.svg
	rm -f $(DESTDIR)$(PREFIX)/share/locale/*/LC_MESSAGES/lumen.mo

# What CI runs.
check:
	desktop-file-validate data/$(ID).desktop
	appstreamcli validate --no-net data/$(ID).metainfo.xml
	$(CARGO) clippy --all-targets -- -D warnings
	$(CARGO) test

# A Debian/Ubuntu package of the release build.
deb:
	./build-aux/debian/build-deb.sh
