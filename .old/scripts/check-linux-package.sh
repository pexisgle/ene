#!/usr/bin/env bash
set -euo pipefail

VERSION="${1:?usage: scripts/check-linux-package.sh <version> [dist-dir]}"
DIST_DIR="${2:-dist}"
DIST_DIR="$(cd "$DIST_DIR" && pwd)"
DEB_FILE="$DIST_DIR/ene-stage_${VERSION}_amd64.deb"
TAR_FILE="$DIST_DIR/ene-ctl-${VERSION}-linux-x86_64.tar.gz"

PLUGINS=(
  ene-tool-fs
  ene-tool-exec
  ene-tool-web
  ene-tool-utility
  ene-tool-app
  ene-tool-mcp
  ene-provider-openai-compat
  ene-provider-gguf
  ene-provider-anthropic
  ene-provider-elevenlabs
  ene-provider-voicevox
  ene-provider-edge-tts
)

for file in "$DEB_FILE" "$TAR_FILE"; do
  if [[ ! -f "$file" ]]; then
    printf 'missing release artifact: %s\n' "$file" >&2
    exit 1
  fi
done

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

tar -tzf "$TAR_FILE" > "$tmp_dir/tar-contents"
archive_root="ene-ctl-${VERSION}-linux-x86_64"
for binary in ene-ctl ene-core; do
  if ! grep -Fqx "$archive_root/$binary" "$tmp_dir/tar-contents"; then
    printf 'release archive is missing %s\n' "$binary" >&2
    exit 1
  fi
done
for plugin in "${PLUGINS[@]}"; do
  if ! grep -Fqx "$archive_root/plugins/$plugin" "$tmp_dir/tar-contents"; then
    printf 'release archive is missing plugin %s\n' "$plugin" >&2
    exit 1
  fi
done

docker run --rm -i --pull=always \
  --env "ARCHIVE_ROOT=$archive_root" \
  --volume "$DEB_FILE:/tmp/ene-stage.deb:ro" \
  --volume "$TAR_FILE:/tmp/ene-release.tar.gz:ro" \
  ubuntu:22.04 bash -s <<'EOF'
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive

plugins=(
  ene-tool-fs
  ene-tool-exec
  ene-tool-web
  ene-tool-utility
  ene-tool-app
  ene-tool-mcp
  ene-provider-openai-compat
  ene-provider-gguf
  ene-provider-anthropic
  ene-provider-elevenlabs
  ene-provider-voicevox
  ene-provider-edge-tts
)

apt-get update
apt-get install -y --no-install-recommends /tmp/ene-stage.deb

test -x /usr/bin/ene-core
test -x /usr/bin/ene-stage
test -f /usr/share/applications/ene.desktop
test -f /usr/share/pixmaps/ene.png

binaries=(/usr/bin/ene-core /usr/bin/ene-stage)
for plugin in "${plugins[@]}"; do
  path="/usr/bin/plugins/$plugin"
  if [[ ! -x "$path" ]]; then
    printf 'installed package is missing executable plugin %s\n' "$plugin" >&2
    exit 1
  fi
  binaries+=("$path")
done

for binary in "${binaries[@]}"; do
  if ! ldd_output="$(ldd "$binary" 2>&1)"; then
    printf 'ldd failed for %s:\n%s\n' "$binary" "$ldd_output" >&2
    exit 1
  fi
  if grep -Fq 'not found' <<<"$ldd_output"; then
    printf 'unresolved shared library for %s:\n%s\n' "$binary" "$ldd_output" >&2
    exit 1
  fi
done

timeout 10 /usr/bin/ene-core --help >/dev/null
mkdir -p /tmp/ene-release
tar -xzf /tmp/ene-release.tar.gz -C /tmp/ene-release
timeout 10 "/tmp/ene-release/$ARCHIVE_ROOT/ene-ctl" --help >/dev/null
EOF

printf 'Linux package and release archive smoke checks passed.\n'
