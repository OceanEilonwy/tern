#!/bin/sh
# Builds the release tarball inside Void Linux (glibc), whose older glibc keeps the binary
# runnable on Void as well as on newer distributions such as CachyOS and Arch. Run as root in a
# ghcr.io/void-linux/void-glibc-full container, from the source tree.
set -eu
xbps-install -Syu xbps >/dev/null
xbps-install -Syu >/dev/null
xbps-install -y gcc pkg-config gtk4-devel libadwaita-devel openssl-devel dbus-devel rustup git curl tar gzip >/dev/null
rustup-init -y --profile minimal >/dev/null
. "$HOME/.cargo/env"
cargo build --release --locked -p tern
packaging/linux/tarball.sh "$@"
