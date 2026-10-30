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

# Signs in as "demo" through the real Sign In button on a headless broadway display and
# checks the mailbox opens (it once stayed on the sign-in page).
test-ui:
	cargo build -p tern
	@dir=$$(mktemp -d); gtk4-broadwayd :77 >/dev/null 2>&1 & bw=$$!; sleep 1; \
	GDK_BACKEND=broadway BROADWAY_DISPLAY=:77 TERN_SCREENSHOT=$$dir/sign-in.png TERN_SCREENSHOT_SCENE=sign-in \
		timeout 60 target/debug/tern 2>/dev/null; kill $$bw; \
	test -s $$dir/sign-in.png && echo "test-ui: sign-in opens the mailbox" || { echo "test-ui: sign-in did not open the mailbox"; exit 1; }

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
