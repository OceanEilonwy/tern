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
	cargo test -p tern -p tern-core -p tern-tuta -p tern-cache

# Drives the real window on a headless display: KDE's kwin_wayland in virtual mode when installed
# (it draws real frames, so layout settles as on screen), else GTK's broadway. Each scene saves a
# picture only if its check passed.
UI_SCENES = sign-in folders-fail refresh-race sign-out-race draft suggest keys arrows busy panes search
test-ui:
	cargo build -p tern
	@dir=$$(mktemp -d); \
	if command -v kwin_wayland >/dev/null; then \
		kwin_wayland --virtual --no-lockscreen --socket=tern-test-$$$$ --width=1400 --height=860 >/dev/null 2>&1 & srv=$$!; \
		display="WAYLAND_DISPLAY=tern-test-$$$$ GDK_BACKEND=wayland"; \
	else \
		gtk4-broadwayd :77 >/dev/null 2>&1 & srv=$$!; display="GDK_BACKEND=broadway BROADWAY_DISPLAY=:77"; \
	fi; sleep 2; failed=; \
	for scene in $(UI_SCENES); do \
		demo=TERN_DEMO=1; [ $$scene = sign-in ] && demo=; \
		env $$display $$demo XDG_CONFIG_HOME=$$dir/config-$$scene TERN_SCREENSHOT=$$dir/$$scene.png TERN_SCREENSHOT_SCENE=$$scene \
			timeout 60 target/debug/tern 2>&1 | grep "screenshot failed"; \
		if test -s $$dir/$$scene.png; then echo "test-ui: $$scene ok"; else echo "test-ui: $$scene FAILED"; failed=1; fi; \
	done; kill $$srv; test -z "$$failed"

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
