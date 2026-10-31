#!/bin/sh
# Packs a release build into dist/tern-<version>-<arch>-linux.tar.gz with install.sh.
#   packaging/linux/tarball.sh [version]
set -eu
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
version=${1:-$(sed -n 's/^version = "\(.*\)"/\1/p' crates/app/Cargo.toml | head -1)}
arch=$(uname -m)
name=tern-$version-$arch-linux
stage=dist/$name
id=xyz.hollingworth.Tern
rm -rf "$stage"
install -Dm755 target/release/tern "$stage/bin/tern"
install -Dm644 data/$id.desktop "$stage/share/applications/$id.desktop"
install -Dm644 data/icons/$id.svg "$stage/share/icons/hicolor/scalable/apps/$id.svg"
install -Dm644 data/$id.metainfo.xml "$stage/share/metainfo/$id.metainfo.xml"
install -Dm755 packaging/linux/install.sh "$stage/install.sh"
install -Dm644 LICENSE "$stage/LICENSE"
tar -C dist -czf "dist/$name.tar.gz" "$name"
echo "dist/$name.tar.gz"
