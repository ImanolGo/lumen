PREFIX ?= /usr/local
DESTDIR ?=
APP_ID := io.github.imanolgo.LumenRs
BIN := target/release/lumen-rs

.PHONY: all build test lint install uninstall clean

all: build

build:
	cargo build --release --locked

test:
	cargo test --locked

lint:
	cargo fmt --check
	cargo clippy --all-targets --locked -- -D warnings

install: build
	install -Dm755 $(BIN) $(DESTDIR)$(PREFIX)/bin/lumen-rs
	install -Dm644 data/$(APP_ID).desktop $(DESTDIR)$(PREFIX)/share/applications/$(APP_ID).desktop
	install -Dm644 data/$(APP_ID).metainfo.xml $(DESTDIR)$(PREFIX)/share/metainfo/$(APP_ID).metainfo.xml
	cd data/icons && for f in */apps/$(APP_ID).*; do \
		install -Dm644 "$$f" "$(DESTDIR)$(PREFIX)/share/icons/hicolor/$$f"; \
	done
	-update-desktop-database -q $(DESTDIR)$(PREFIX)/share/applications 2>/dev/null
	-gtk-update-icon-cache -q -t $(DESTDIR)$(PREFIX)/share/icons/hicolor 2>/dev/null

uninstall:
	rm -f $(DESTDIR)$(PREFIX)/bin/lumen-rs
	rm -f $(DESTDIR)$(PREFIX)/share/applications/$(APP_ID).desktop
	rm -f $(DESTDIR)$(PREFIX)/share/metainfo/$(APP_ID).metainfo.xml
	rm -f $(DESTDIR)$(PREFIX)/share/icons/hicolor/*/apps/$(APP_ID).*

clean:
	cargo clean
