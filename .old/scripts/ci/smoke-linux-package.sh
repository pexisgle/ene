#!/usr/bin/env bash
set -euo pipefail

# The .deb declares hand-written runtime dependencies; installing it into a
# pristine image is the only check that catches a drifted Depends: line.
IMAGE="${SMOKE_IMAGE:-ubuntu:24.04}"

DIST_DIR="${1:?usage: scripts/ci/smoke-linux-package.sh <dist-dir>}"
DIST_DIR="$(cd "$DIST_DIR" && pwd)"

deb_path="$(find "$DIST_DIR" -maxdepth 1 -name 'ene-stage_*_amd64.deb' | head -n1)"
version="$(basename "$deb_path")"
version="${version#ene-stage_}"
version="${version%_amd64.deb}"
if [[ -z "$version" ]]; then
  printf 'no ene-stage_*.deb found in %s\n' "$DIST_DIR" >&2
  exit 1
fi

DEB_FILE="$DIST_DIR/ene-stage_${version}_amd64.deb"
TAR_FILE="$DIST_DIR/ene-ctl-${version}-linux-x86_64.tar.gz"
ARCHIVE_ROOT="ene-ctl-${version}-linux-x86_64"

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

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

tar -tzf "$TAR_FILE" > "$tmp_dir/tar-contents"
for binary in ene-ctl ene-core; do
  if ! grep -Fqx "$ARCHIVE_ROOT/$binary" "$tmp_dir/tar-contents"; then
    printf 'release archive is missing %s\n' "$binary" >&2
    exit 1
  fi
done
for plugin in "${PLUGINS[@]}"; do
  if ! grep -Fqx "$ARCHIVE_ROOT/plugins/$plugin" "$tmp_dir/tar-contents"; then
    printf 'release archive is missing plugin %s\n' "$plugin" >&2
    exit 1
  fi
done

check_installed_tree() {
  for path in \
    /usr/bin/ene-core \
    /usr/bin/ene-stage \
    /usr/share/applications/ene.desktop \
    /usr/share/pixmaps/ene.png; do
    if [[ ! -e "$path" ]]; then
      printf 'installed package is missing %s\n' "$path" >&2
      exit 1
    fi
  done
  test -x /usr/bin/ene-core
  test -x /usr/bin/ene-stage

  binaries=(/usr/bin/ene-core /usr/bin/ene-stage)
  for plugin in "${PLUGINS[@]}"; do
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
}
run_cli_checks() {
  timeout 10 /usr/bin/ene-core --help >/dev/null
  mkdir -p "$tmp_dir/archive"
  tar -xzf "$TAR_FILE" -C "$tmp_dir/archive"
  timeout 10 "$tmp_dir/archive/$ARCHIVE_ROOT/ene-ctl" --help >/dev/null
}

# A missing Depends: entry only fails inside a pristine image; a host always
# has the build machine's full library set, so the local fallback is weaker
# by construction.
if command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
  docker run --rm -i --pull=always \
    --env "SMOKE_PLUGINS=$(IFS=' '; echo "${PLUGINS[*]}")" \
    --env "ARCHIVE_ROOT=$ARCHIVE_ROOT" \
    --volume "$DEB_FILE:/tmp/ene-stage.deb:ro" \
    --volume "$TAR_FILE:/tmp/ene-release.tar.gz:ro" \
    --volume "$tmp_dir:/tmp/smoke-work:rw" \
    "$IMAGE" bash -s <<'EOF'
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive

read -r -a plugins <<<"$SMOKE_PLUGINS"

check_installed_tree() {
  for path in \
    /usr/bin/ene-core \
    /usr/bin/ene-stage \
    /usr/share/applications/ene.desktop \
    /usr/share/pixmaps/ene.png; do
    if [[ ! -e "$path" ]]; then
      printf 'installed package is missing %s\n' "$path" >&2
      exit 1
    fi
  done
  test -x /usr/bin/ene-core
  test -x /usr/bin/ene-stage

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
}

apt-get update
apt-get install -y --no-install-recommends /tmp/ene-stage.deb

check_installed_tree

timeout 10 /usr/bin/ene-core --help >/dev/null
mkdir -p /tmp/smoke-work/archive
tar -xzf /tmp/ene-release.tar.gz -C /tmp/smoke-work/archive
timeout 10 "/tmp/smoke-work/archive/$ARCHIVE_ROOT/ene-ctl" --help >/dev/null
EOF
else
  printf 'docker unavailable; running weaker local checks against the host\n' >&2
  sudo dpkg -i "$DEB_FILE" || sudo apt-get -f install -y
  check_installed_tree
  run_cli_checks
fi

printf 'Linux package smoke checks passed (%s).\n' "$IMAGE"
