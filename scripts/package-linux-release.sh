#!/usr/bin/env bash
# Package Linux release artifacts from release-mode binaries in target/release.
#
# Usage: scripts/package-linux-release.sh <version> [target-dir] [dist-dir]
#
# Produces:
#   dist/ene-ctl-<version>-linux-x86_64.tar.gz  — CLI + core daemon + harness plugins
#   dist/ene-stage_<version>_amd64.deb          — stage .deb (plugins next to the binary)

set -euo pipefail

VERSION="${1:?usage: $0 <version> [target-dir] [dist-dir]}"
TARGET_DIR="${2:-target/release}"
DIST_DIR="${3:-dist}"

PLUGINS=(
  ene-harness-fs
  ene-harness-exec
  ene-harness-web
  ene-harness-utility
  ene-harness-app
  ene-harness-mcp
  ene-provider-openai-compat
  ene-provider-gguf
  ene-provider-anthropic
  ene-provider-elevenlabs
  ene-provider-voicevox
  ene-provider-edge-tts
)

mkdir -p "$DIST_DIR"

for bin in ene-ctl ene-core ene-stage "${PLUGINS[@]}"; do
  if [[ ! -f "$TARGET_DIR/$bin" ]]; then
    echo "error: missing release binary: $TARGET_DIR/$bin" >&2
    exit 1
  fi
done

CLI_ROOT="$DIST_DIR/ene-ctl-${VERSION}-linux-x86_64"
rm -rf "$CLI_ROOT"
mkdir -p "$CLI_ROOT/plugins"
cp "$TARGET_DIR/ene-ctl" "$CLI_ROOT/"
cp "$TARGET_DIR/ene-core" "$CLI_ROOT/"
for plugin in "${PLUGINS[@]}"; do
  cp "$TARGET_DIR/$plugin" "$CLI_ROOT/plugins/"
done
tar -czf "$DIST_DIR/ene-ctl-${VERSION}-linux-x86_64.tar.gz" -C "$DIST_DIR" "ene-ctl-${VERSION}-linux-x86_64"
rm -rf "$CLI_ROOT"

DEB_ROOT="$DIST_DIR/ene-stage-deb"
rm -rf "$DEB_ROOT"
mkdir -p "$DEB_ROOT/DEBIAN" \
  "$DEB_ROOT/usr/bin/plugins" \
  "$DEB_ROOT/usr/share/applications"

cp "$TARGET_DIR/ene-stage" "$DEB_ROOT/usr/bin/ene-stage"
cp "$TARGET_DIR/ene-core" "$DEB_ROOT/usr/bin/ene-core"
chmod 755 "$DEB_ROOT/usr/bin/ene-stage" "$DEB_ROOT/usr/bin/ene-core"
for plugin in "${PLUGINS[@]}"; do
  cp "$TARGET_DIR/$plugin" "$DEB_ROOT/usr/bin/plugins/"
  chmod 755 "$DEB_ROOT/usr/bin/plugins/$plugin"
done

cat >"$DEB_ROOT/usr/share/applications/ene.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=ene
Comment=Local AI companion stage
Exec=ene-stage
Icon=ene
Terminal=false
Categories=Utility;
EOF

cat >"$DEB_ROOT/DEBIAN/control" <<EOF
Package: ene-stage
Version: ${VERSION}
Section: utils
Priority: optional
Architecture: amd64
Maintainer: ene contributors <https://github.com/pexisgle/ene>
Depends: libgtk-3-0, libssl3 | libssl1.1, libx11-6, libxkbcommon0, libwayland-client0, libvulkan1
Description: ene stage — local AI companion
 ene stage is an egui + wgpu client that starts ene-core and shows
 companions on a native stage.
EOF

dpkg-deb --build --root-owner-group "$DEB_ROOT" \
  "$DIST_DIR/ene-stage_${VERSION}_amd64.deb"
rm -rf "$DEB_ROOT"

echo "Packaged:"
ls -1 "$DIST_DIR"/ene-ctl-"${VERSION}"-linux-x86_64.tar.gz \
  "$DIST_DIR/ene-stage_${VERSION}_amd64.deb"
