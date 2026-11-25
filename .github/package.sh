#!/usr/bin/env bash
# Packages the release build (target/release/rs3manager) into dist/rs3manager-VERSION-linux-x86_64.tar.gz with its
# launcher entry, icon and licenses, and writes the archive's SHA-256 beside it.
set -euo pipefail
cd "$(dirname "$0")/.."
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
name="rs3manager-$version-linux-x86_64"
target=${CARGO_TARGET_DIR:-target}
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/$name" dist
install -m755 "$target/release/rs3manager" "$stage/$name/rs3manager"
install -m644 gui/assets/rs3manager.desktop gui/assets/rs3manager.svg LICENSE NOTICE README.md "$stage/$name/"
tar -C "$stage" -czf "dist/$name.tar.gz" "$name"
(cd dist && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256")
echo "dist/$name.tar.gz"
