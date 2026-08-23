#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MANIFEST="${1:-$REPO_ROOT/third_party/assets.json}"

if [[ ! -f "$MANIFEST" ]]; then
  printf 'asset provenance manifest is missing: %s\n' "$MANIFEST" >&2
  exit 1
fi

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

tracked_assets="$tmp_dir/tracked"
manifest_assets="$tmp_dir/manifest"

git -C "$REPO_ROOT" ls-files -z assets \
  | while IFS= read -r -d '' path; do
      case "$path" in
        *.ttf|*.otf|*.woff|*.woff2|*.vrm|*.vrma|*.gltf|*.glb|*.png|*.jpg|*.jpeg|*.gif|*.webp|*.svg|*.mp3|*.wav|*.ogg|*.mp4|*.webm)
          printf '%s\n' "$path"
          ;;
      esac
    done \
  | sort -u >"$tracked_assets"

jq -e '.schema_version == 1 and (.assets | type == "array" and length > 0)' "$MANIFEST" >/dev/null
jq -r '.assets[].path' "$MANIFEST" | sort -u >"$manifest_assets"

if ! diff -u "$tracked_assets" "$manifest_assets"; then
  printf 'tracked binary/media assets and manifest entries differ\n' >&2
  exit 1
fi

if jq -e '
  any(.assets[];
    (.path | type) != "string" or
    (.sha256 | test("^[0-9a-f]{64}$") | not) or
    (.source | type) != "string" or
    (.license | type) != "string" or
    (.redistribution | type) != "string" or
    (.distribution | type) != "string" or
    (.provenance_status | type) != "string" or
    (.notice | type) != "string" or
    (.provenance_status == "unknown")
  )
' "$MANIFEST" >/dev/null; then
  printf 'asset provenance manifest contains an incomplete or unknown entry\n' >&2
  exit 1
fi

while IFS= read -r notice; do
  if [[ ! -f "$REPO_ROOT/$notice" ]]; then
    printf 'asset provenance notice is missing: %s\n' "$notice" >&2
    exit 1
  fi
done < <(jq -r '.assets[].notice' "$MANIFEST" | sort -u)

while IFS= read -r path; do
  expected="$(jq -er --arg path "$path" '.assets[] | select(.path == $path) | .sha256' "$MANIFEST")"
  actual="$(sha256sum "$REPO_ROOT/$path" | awk '{print $1}')"
  if [[ "$actual" != "$expected" ]]; then
    printf 'sha256 mismatch for %s: expected %s, got %s\n' "$path" "$expected" "$actual" >&2
    exit 1
  fi
done <"$tracked_assets"

printf 'asset provenance manifest covers all tracked binary/media assets and notices.\n'
