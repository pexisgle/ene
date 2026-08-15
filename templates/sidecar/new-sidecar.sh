#!/usr/bin/env bash
# Scaffold the sidecar lifecycle module into an existing provider plugin.
#
# Usage: templates/sidecar/new-sidecar.sh <plugin-name>
#
# Creates plugins/provider/<plugin-name>/src/sidecar/ with placeholder names
# replaced. The plugin crate must already exist (see templates/tool for the
# plugin scaffolding pattern).
set -euo pipefail

NAME="${1:?usage: $0 <plugin-name>}"
if [[ ! "$NAME" =~ ^[a-zA-Z0-9_-]+$ ]]; then
  echo "error: plugin name must match [a-zA-Z0-9_-]" >&2
  exit 1
fi

UPPER="$(printf '%s' "$NAME" | tr '[:lower:]' '[:upper:]' | tr '-' '_')"
TARGET="plugins/provider/$NAME/src/sidecar"

if [[ ! -d "plugins/provider/$NAME" ]]; then
  echo "error: plugins/provider/$NAME does not exist" >&2
  exit 1
fi
if [[ -e "$TARGET" ]]; then
  echo "error: $TARGET already exists" >&2
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
mkdir -p "$TARGET"
cp -R "$SCRIPT_DIR"/src/sidecar/. "$TARGET"/

for file in "$TARGET"/*.rs; do
  sed -i \
    -e "s/__SIDECAR_NAME__/$NAME/g" \
    -e "s/__SIDECAR_UPPER__/$UPPER/g" \
    "$file"
done

echo "created $TARGET"
echo "next steps:"
echo "  1. declare 'mod sidecar;' in the plugin's main.rs"
echo "  2. copy the dependency block from templates/sidecar/Cargo.toml"
echo "  3. merge sidecar::config::SidecarConfig into the plugin's config type"
echo "  4. adapt the health probe URL and the preset schema to the engine"
echo "  5. follow templates/sidecar/CHECKLIST.md before enabling the plugin"
