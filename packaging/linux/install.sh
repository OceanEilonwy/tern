#!/bin/sh
# Installs Tern from this folder: the program, its menu entry and its icon.
#   ./install.sh                     for you alone, into ~/.local
#   sudo PREFIX=/usr/local ./install.sh   for everyone
#   ./install.sh --uninstall         takes it out again (same PREFIX)
# Needs GTK 4.22 and libadwaita 1.9 or newer, as on CachyOS, Arch and Void.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
PREFIX=${PREFIX:-$HOME/.local}
id=xyz.hollingworth.Tern
bin=$PREFIX/bin/tern
desktop=$PREFIX/share/applications/$id.desktop
icon=$PREFIX/share/icons/hicolor/scalable/apps/$id.svg
metainfo=$PREFIX/share/metainfo/$id.metainfo.xml

refresh() {
	gtk-update-icon-cache -qtf "$PREFIX/share/icons/hicolor" 2>/dev/null || true
	update-desktop-database -q "$PREFIX/share/applications" 2>/dev/null || true
}

if [ "${1:-}" = "--uninstall" ]; then
	rm -f "$bin" "$desktop" "$icon" "$metainfo"
	refresh
	echo "Tern is uninstalled. Your mail cache stays in ~/.cache/tern and settings in ~/.config/tern."
	exit 0
fi

install -Dm755 "$here/bin/tern" "$bin"
install -Dm644 "$here/share/icons/hicolor/scalable/apps/$id.svg" "$icon"
install -Dm644 "$here/share/metainfo/$id.metainfo.xml" "$metainfo"
mkdir -p "$(dirname "$desktop")"
# The menu entry names the program by its full path: ~/.local/bin is not on every desktop's path.
sed "s|^Exec=tern|Exec=$bin|" "$here/share/applications/$id.desktop" > "$desktop"
chmod 644 "$desktop"
refresh
echo "Tern is installed: find it in your applications menu, or run $bin"
