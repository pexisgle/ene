#!/usr/bin/env bash
# Package Linux release artifacts from release-mode binaries in target/release.
#
# Usage: scripts/package-linux-release.sh <version> [target-dir] [dist-dir]
#
# Produces:
#   dist/ene-cli-<version>-linux-x86_64.tar.gz   — CLI + built-in tools + provider plugins
#   dist/ene-desktop_<version>_amd64.deb         — desktop .deb (tools in /usr/bin/tools/, plugins in /usr/bin/plugins/)

set -euo pipefail

VERSION="${1:?usage: $0 <version> [target-dir] [dist-dir]}"
TARGET_DIR="${2:-target/release}"
DIST_DIR="${3:-dist}"

TOOLS=(
  ene-tool-fs
  ene-tool-web
  ene-tool-utility
  ene-tool-app
  ene-tool-browser
)

PLUGINS=(
  ene-plugin-llama-cpp
  ene-plugin-llama-server
)

mkdir -p "$DIST_DIR"

for bin in ene-cli ene-desktop "${TOOLS[@]}" "${PLUGINS[@]}"; do
  if [[ ! -f "$TARGET_DIR/$bin" ]]; then
    echo "error: missing release binary: $TARGET_DIR/$bin" >&2
    exit 1
  fi
done

CLI_ROOT="$DIST_DIR/ene-cli-${VERSION}-linux-x86_64"
rm -rf "$CLI_ROOT"
mkdir -p "$CLI_ROOT/tools" "$CLI_ROOT/plugins"
cp "$TARGET_DIR/ene-cli" "$CLI_ROOT/"
for tool in "${TOOLS[@]}"; do
  cp "$TARGET_DIR/$tool" "$CLI_ROOT/tools/"
done
for plugin in "${PLUGINS[@]}"; do
  cp "$TARGET_DIR/$plugin" "$CLI_ROOT/plugins/"
done
# Ship a sidecar llama-server binary when the release build provides one
# (optional: the plugin also resolves it from PATH / server_path config).
if [[ -f "$TARGET_DIR/llama-server" ]]; then
  cp "$TARGET_DIR/llama-server" "$CLI_ROOT/plugins/llama-server"
fi
tar -czf "$DIST_DIR/ene-cli-${VERSION}-linux-x86_64.tar.gz" -C "$DIST_DIR" "ene-cli-${VERSION}-linux-x86_64"
rm -rf "$CLI_ROOT"

# Release builds resolve built-in tools from <exe_dir>/tools/ (see ene-config paths).

DEB_ROOT="$DIST_DIR/ene-desktop-deb"
rm -rf "$DEB_ROOT"
mkdir -p "$DEB_ROOT/DEBIAN" \
  "$DEB_ROOT/usr/bin/tools" \
  "$DEB_ROOT/usr/bin/plugins" \
  "$DEB_ROOT/usr/share/applications"

cp "$TARGET_DIR/ene-desktop" "$DEB_ROOT/usr/bin/ene-desktop"
chmod 755 "$DEB_ROOT/usr/bin/ene-desktop"
for tool in "${TOOLS[@]}"; do
  cp "$TARGET_DIR/$tool" "$DEB_ROOT/usr/bin/tools/"
  chmod 755 "$DEB_ROOT/usr/bin/tools/$tool"
done
for plugin in "${PLUGINS[@]}"; do
  cp "$TARGET_DIR/$plugin" "$DEB_ROOT/usr/bin/plugins/"
  chmod 755 "$DEB_ROOT/usr/bin/plugins/$plugin"
done
if [[ -f "$TARGET_DIR/llama-server" ]]; then
  cp "$TARGET_DIR/llama-server" "$DEB_ROOT/usr/bin/plugins/llama-server"
  chmod 755 "$DEB_ROOT/usr/bin/plugins/llama-server"
fi

cat >"$DEB_ROOT/usr/share/applications/ene.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=ene
Comment=Local AI character assistant
Exec=ene-desktop
Icon=ene
Terminal=false
Categories=Utility;
EOF

cat >"$DEB_ROOT/DEBIAN/control" <<EOF
Package: ene-desktop
Version: ${VERSION}
Section: utils
Priority: optional
Architecture: amd64
Maintainer: ene contributors <https://github.com/pexisgle/ene>
Depends: libgtk-3-0, libayatana-appindicator3-1, libssl3 | libssl1.1, libx11-6, libxkbcommon0, libwayland-client0, libpipewire-0.3-0, libvulkan1
Description: ene desktop — local AI character assistant
 ene desktop is a winit + wgpu GUI for chatting with VRM characters,
 running built-in tools, and managing long-term memory locally.
EOF

dpkg-deb --build --root-owner-group "$DEB_ROOT" \
  "$DIST_DIR/ene-desktop_${VERSION}_amd64.deb"
rm -rf "$DEB_ROOT"

echo "Packaged:"
ls -1 "$DIST_DIR"/ene-cli-"${VERSION}"-linux-x86_64.tar.gz \
  "$DIST_DIR/ene-desktop_${VERSION}_amd64.deb"
