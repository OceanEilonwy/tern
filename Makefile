PREFIX ?= $(HOME)/.local
APP_ID = xyz.hollingworth.Tern

.PHONY: build run demo test install uninstall

build:
	cargo build --release -p tern

run:
	cargo run -p tern

# Skips sign-in and opens the in-memory demo account.
demo:
	TERN_DEMO=1 cargo run -p tern

test:
	cargo test -p tern -p tern-core

install: build
	install -Dm755 target/release/tern $(PREFIX)/bin/tern
	install -Dm644 data/$(APP_ID).desktop $(PREFIX)/share/applications/$(APP_ID).desktop
	install -Dm644 data/icons/$(APP_ID).svg $(PREFIX)/share/icons/hicolor/scalable/apps/$(APP_ID).svg
	install -Dm644 data/$(APP_ID).metainfo.xml $(PREFIX)/share/metainfo/$(APP_ID).metainfo.xml
	-gtk-update-icon-cache -qtf $(PREFIX)/share/icons/hicolor
	-update-desktop-database -q $(PREFIX)/share/applications

uninstall:
	rm -f $(PREFIX)/bin/tern $(PREFIX)/share/applications/$(APP_ID).desktop \
		$(PREFIX)/share/icons/hicolor/scalable/apps/$(APP_ID).svg $(PREFIX)/share/metainfo/$(APP_ID).metainfo.xml
