PREFIX ?= $(HOME)/.local
APP_ID = xyz.hollingworth.Tern

.PHONY: build run demo test test-ui install uninstall

build:
	cargo build --release -p tern

run:
	cargo run -p tern

# Skips sign-in and opens the in-memory demo account.
demo:
	TERN_DEMO=1 cargo run -p tern

test: test-ui
	cargo test -p tern -p tern-core -p tern-tuta

# Drives the real window on a headless broadway display. Each scene saves a picture only if its
# check passed: sign-in signs in as "demo" through the Sign In button and needs the mailbox to open;
# the others need the window to recover when folders fail, a refresh races a load, or folders
# arrive after signing out.
UI_SCENES = sign-in folders-fail refresh-race sign-out-race
test-ui:
	cargo build -p tern
	@dir=$$(mktemp -d); gtk4-broadwayd :77 >/dev/null 2>&1 & bw=$$!; sleep 1; failed=; \
	for scene in $(UI_SCENES); do \
		demo=TERN_DEMO=1; [ $$scene = sign-in ] && demo=; \
		env $$demo GDK_BACKEND=broadway BROADWAY_DISPLAY=:77 TERN_SCREENSHOT=$$dir/$$scene.png \
			TERN_SCREENSHOT_SCENE=$$scene timeout 60 target/debug/tern 2>&1 | grep "screenshot failed"; \
		if test -s $$dir/$$scene.png; then echo "test-ui: $$scene ok"; else echo "test-ui: $$scene FAILED"; failed=1; fi; \
	done; kill $$bw; test -z "$$failed"

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
