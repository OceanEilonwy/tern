PREFIX ?= $(HOME)/.local
APP_ID = xyz.hollingworth.Tern

.PHONY: build run demo test test-ui install uninstall android

build:
	cargo build --release -p tern

run:
	cargo run -p tern

# Skips sign-in and opens the in-memory demo account.
demo:
	TERN_DEMO=1 cargo run -p tern

test: test-ui
	cargo test -p tern -p tern-mail -p tern-rules -p tern-html -p tern-compose -p tern-demo -p tern-tuta -p tern-cache -p tern-sync -p tern-app

# Drives the real window on a headless display: KDE's kwin_wayland in virtual mode when installed
# (it draws real frames, so layout settles as on screen), else GNOME's mutter, headless (as in CI;
# it needs a D-Bus session, and with no GPU set GSK_RENDERER=cairo), else GTK's broadway. Each scene saves a
# picture only if its check passed.
UI_SCENES = sign-in mailto default-app folders-fail refresh-race sign-out-race draft suggest keys arrows busy panes search focus-first spam rules rule-edit menu from quote cleanup
test-ui:
	cargo build -p tern
	@dir=$$(mktemp -d); \
	mkdir -p $$dir/data/applications; \
	sed "s|^Exec=tern|Exec=$$PWD/target/debug/tern|" data/$(APP_ID).desktop > $$dir/data/applications/$(APP_ID).desktop; \
	printf '[Desktop Entry]\nName=Other Mail\nExec=true %%u\nType=Application\nMimeType=x-scheme-handler/mailto;\n' > $$dir/data/applications/other-mail.desktop; \
	printf '[MIME Cache]\nx-scheme-handler/mailto=$(APP_ID).desktop;other-mail.desktop;\n' > $$dir/data/applications/mimeinfo.cache; \
	mkdir -p $$dir/config-default-app; \
	printf '[Default Applications]\nx-scheme-handler/mailto=other-mail.desktop;\n' > $$dir/config-default-app/mimeapps.list; \
	if command -v kwin_wayland >/dev/null; then \
		kwin_wayland --virtual --no-lockscreen --socket=tern-test-$$$$ --width=1400 --height=860 >/dev/null 2>&1 & srv=$$!; \
		display="WAYLAND_DISPLAY=tern-test-$$$$ GDK_BACKEND=wayland"; \
	elif command -v mutter >/dev/null; then \
		mutter --headless --wayland --no-x11 --wayland-display=tern-test-$$$$ --virtual-monitor 1400x860 >/dev/null 2>&1 & srv=$$!; \
		display="WAYLAND_DISPLAY=tern-test-$$$$ GDK_BACKEND=wayland"; \
	else \
		gtk4-broadwayd :77 >/dev/null 2>&1 & srv=$$!; display="GDK_BACKEND=broadway BROADWAY_DISPLAY=:77"; \
	fi; sleep 2; failed=; \
	for scene in $(UI_SCENES); do \
		demo=TERN_DEMO=1; case $$scene in sign-in|mailto) demo=;; esac; \
		env $$display $$demo XDG_CONFIG_HOME=$$dir/config-$$scene XDG_DATA_HOME=$$dir/data TERN_SCREENSHOT=$$dir/$$scene.png TERN_SCREENSHOT_SCENE=$$scene \
			timeout 60 target/debug/tern 2>&1 | grep "screenshot failed"; \
		if test -s $$dir/$$scene.png; then echo "test-ui: $$scene ok"; else echo "test-ui: $$scene FAILED"; failed=1; fi; \
	done; kill $$srv; test -z "$$failed"

# Both go through install.sh, which also writes the menu entry and icon.
install: build
	PREFIX=$(PREFIX) ./install.sh

uninstall:
	PREFIX=$(PREFIX) ./install.sh --uninstall

# The Android app, optimized; see android/README.md.
android:
	cd android && ./gradlew :app:assembleRelease
