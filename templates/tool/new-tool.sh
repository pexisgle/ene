#!/usr/bin/env bash
# Scaffold a new tool plugin from this template.
#
# Usage: templates/tool/new-tool.sh <plugin-name> [namespace]
#
# Creates plugins/tool/<plugin-name>/ with placeholder names replaced.
# The namespace defaults to the plugin name with '-' replaced by '_'
# (tool names allow only alphanumerics, '_', '.', and ':').
set -euo pipefail

NAME="${1:?usage: $0 <plugin-name> [namespace]}"
NAMESPACE="${2:-${NAME//-/_}}"

if [[ ! "$NAME" =~ ^[a-zA-Z0-9_-]+$ ]]; then
  echo "error: plugin name must match [a-zA-Z0-9_-]" >&2
  exit 1
fi
if [[ ! "$NAMESPACE" =~ ^[a-zA-Z0-9_]+([.:][a-zA-Z0-9_]+)*$ ]]; then
  echo "error: namespace must use alphanumerics/_ with '.' or ':' separators (no leading, trailing, or consecutive separators)" >&2
  exit 1
fi

PROVIDER="$(printf '%s' "$NAME" | sed -E 's/(^|-|_)([a-zA-Z])/\U\2/g')ToolProvider"
TARGET="plugins/tool/$NAME"

if [[ -e "$TARGET" ]]; then
  echo "error: $TARGET already exists" >&2
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
mkdir -p "$TARGET"
cp -R "$SCRIPT_DIR"/src "$SCRIPT_DIR"/Cargo.toml "$TARGET"/

for file in "$TARGET"/Cargo.toml "$TARGET"/src/*.rs; do
  sed -i \
    -e "s/__PLUGIN_NAME__/$NAME/g" \
    -e "s/__NAMESPACE__/$NAMESPACE/g" \
    -e "s/__PROVIDER_NAME__/$PROVIDER/g" \
    "$file"
done

echo "created $TARGET"
echo "next steps:"
echo "  1. cargo fmt --all && cargo check -p ene-plugin-$NAME"
echo "  2. add '\"$NAME\": { \"enable\": true }' to plugins.list in settings.json"
echo "  3. run the app and verify with /tool list"
