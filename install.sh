#!/usr/bin/env bash
# Builds Tern from this source tree and installs it with its menu entry and icon.
#
#   ./install.sh                         build, then install for you alone (~/.local)
#   sudo PREFIX=/usr/local ./install.sh  install for everyone (builds as you first if run via make)
#   ./install.sh --uninstall             take it out again (same PREFIX)
#   ./install.sh --deps                  only show how to install what the build needs
#
# Needs Rust (https://rustup.rs), GTK 4.22 and libadwaita 1.9 or newer, OpenSSL and D-Bus headers.
set -euo pipefail

cd "$(dirname "$0")"
PREFIX=${PREFIX:-$HOME/.local}
ID=xyz.hollingworth.Tern
BIN=$PREFIX/bin/tern
DESKTOP=$PREFIX/share/applications/$ID.desktop
ICON=$PREFIX/share/icons/hicolor/scalable/apps/$ID.svg
METAINFO=$PREFIX/share/metainfo/$ID.metainfo.xml

say() { printf '\033[1m%s\033[0m\n' "$*"; }
fail() { printf '\033[31m%s\033[0m\n' "$*" >&2; exit 1; }

deps() {
	local id=""
	[ -r /etc/os-release ] && id=$(. /etc/os-release; echo "${ID:-} ${ID_LIKE:-}")
	case "$id" in
		*arch*) echo "sudo pacman -S --needed base-devel gtk4 libadwaita openssl dbus" ;;
		*void*) echo "sudo xbps-install -S gcc pkg-config gtk4-devel libadwaita-devel openssl-devel dbus-devel" ;;
		*fedora*) echo "sudo dnf install gcc pkgconf-pkg-config gtk4-devel libadwaita-devel openssl-devel dbus-devel" ;;
		*opensuse*) echo "sudo zypper install gcc pkgconf gtk4-devel libadwaita-devel libopenssl-devel dbus-1-devel" ;;
		*debian*|*ubuntu*) echo "sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev libssl-dev libdbus-1-dev"
			echo "(Debian, Ubuntu and Linux Mint before 2026 ship a GTK too old for Tern; use the Flatpak instead.)" ;;
		*) echo "Install a C compiler, pkg-config, and the development files of GTK 4, libadwaita, OpenSSL and D-Bus." ;;
	esac
	echo "Rust: curl https://sh.rustup.rs -sSf | sh"
}

refresh() {
	gtk-update-icon-cache -qtf "$PREFIX/share/icons/hicolor" 2>/dev/null || true
	update-desktop-database -q "$PREFIX/share/applications" 2>/dev/null || true
}

case "${1:-}" in
	--deps) deps; exit 0 ;;
	--uninstall)
		rm -f "$BIN" "$DESKTOP" "$ICON" "$METAINFO"
		refresh
		say "Tern is uninstalled. Your mail cache stays in ~/.cache/tern and settings in ~/.config/tern."
		exit 0 ;;
	"") ;;
	*) fail "Unknown option $1. Try --deps or --uninstall." ;;
esac

# What the build needs, checked first so a missing piece says how to get it.
command -v cargo >/dev/null || { deps; fail "Rust's cargo is missing."; }
command -v pkg-config >/dev/null || { deps; fail "pkg-config is missing."; }
for need in "gtk4 4.22" "libadwaita-1 1.9" "openssl 3" "dbus-1 1"; do
	set -- $need
	pkg-config --atleast-version="$2" "$1" 2>/dev/null || { deps; fail "$1 $2 or newer is missing (found: $(pkg-config --modversion "$1" 2>/dev/null || echo none))."; }
done

# Built as the user who owns the tree, even when installing with sudo.
if [ ! -x target/release/tern ] || [ "$(id -u)" != 0 ]; then
	say "Building Tern (a few minutes the first time)…"
	cargo build --release --locked -p tern
fi

say "Installing into $PREFIX…"
install -Dm755 target/release/tern "$BIN"
install -Dm644 data/icons/$ID.svg "$ICON"
install -Dm644 data/$ID.metainfo.xml "$METAINFO"
mkdir -p "$(dirname "$DESKTOP")"
# The menu entry names the program by its full path: ~/.local/bin is not on every desktop's path.
sed "s|^Exec=tern|Exec=$BIN|" data/$ID.desktop > "$DESKTOP"
chmod 644 "$DESKTOP"
refresh
say "Tern is installed: find it in your applications menu, or run $BIN"
