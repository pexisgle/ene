#!/usr/bin/env bash
# Generate a signed artifact catalog from release binaries.
#
# Usage:
#   scripts/publish-catalog.sh VERSION KEY_ID KEY_HEX_FILE BASE_URL OUT_JSON \
#       ID=PATH [ID=PATH ...]
#
#   VERSION      Catalog version to sign.
#   KEY_ID       Key id recorded in the catalog (see `ene catalog keygen`).
#   KEY_HEX_FILE File containing the hex-encoded Ed25519 private key.
#   BASE_URL     HTTPS base URL where the artifacts are hosted. Each
#                artifact's URL becomes BASE_URL/<basename of PATH>.
#   OUT_JSON     Output signed catalog JSON path.
#   ID=PATH      One artifact per argument: catalog id = local file path.
#
# Requires the `ene` binary on PATH (or ENE_BIN pointing at it). Hosting the
# resulting catalog.json at any static HTTPS URL and pointing
# `ArtifactConfig.catalog_url` at it activates the distribution pipeline.
set -euo pipefail

VERSION="${1:?usage: $0 VERSION KEY_ID KEY_HEX_FILE BASE_URL OUT_JSON ID=PATH...}"
KEY_ID="${2:?missing KEY_ID}"
KEY_HEX_FILE="${3:?missing KEY_HEX_FILE}"
BASE_URL="${4:?missing BASE_URL}"
OUT_JSON="${5:?missing OUT_JSON}"
shift 5

if [[ $# -eq 0 ]]; then
  echo "error: pass at least one ID=PATH artifact" >&2
  exit 1
fi
if [[ "$BASE_URL" != https://* ]]; then
  echo "error: BASE_URL must be https:// (got $BASE_URL)" >&2
  exit 1
fi

ENE_BIN="${ENE_BIN:-ene}"
if ! command -v "$ENE_BIN" >/dev/null 2>&1; then
  echo "error: '$ENE_BIN' not found on PATH (set ENE_BIN to the ene binary)" >&2
  exit 1
fi
if [[ ! -f "$KEY_HEX_FILE" ]]; then
  echo "error: key file not found: $KEY_HEX_FILE" >&2
  exit 1
fi
KEY_HEX="$(tr -d '[:space:]' < "$KEY_HEX_FILE")"
if [[ ${#KEY_HEX} -ne 64 ]]; then
  echo "error: private key must be 64 hex characters" >&2
  exit 1
fi

SPEC="$(mktemp)"
trap 'rm -f "$SPEC"' EXIT

{
  echo "{"
  echo "  \"version\": $VERSION,"
  echo "  \"artifacts\": ["
  first=1
  for entry in "$@"; do
    id="${entry%%=*}"
    path="${entry#*=}"
    if [[ -z "$id" || -z "$path" || ! -f "$path" ]]; then
      echo "error: invalid artifact '$entry' (expected ID=PATH to an existing file)" >&2
      exit 1
    fi
    if [[ $first -ne 1 ]]; then
      echo ","
    fi
    first=0
    sha256="$(sha256sum "$path" | cut -d' ' -f1)"
    size="$(stat -c%s "$path")"
    base="$(basename "$path")"
    printf '    { "id": %s, "kind": "sidecar", "version": "1", "urls": ["%s/%s"], "sha256": "%s", "size": %s }' \
      "$(printf '%s' "$id" | jq -R . 2>/dev/null || printf '"%s"' "$id")" \
      "$BASE_URL" "$base" "$sha256" "$size"
  done
  echo ""
  echo "  ]"
  echo "}"
} > "$SPEC"

"$ENE_BIN" catalog build \
  --spec "$SPEC" \
  --key-id "$KEY_ID" \
  --key-hex "$KEY_HEX" \
  --out "$OUT_JSON"

echo "wrote $OUT_JSON"
echo "next: host the artifacts and catalog.json under $BASE_URL, then set"
echo "  plugins.artifact.catalog_url = $BASE_URL/catalog.json"
echo "  plugins.artifact.catalog_keys = [{ key_id: $KEY_ID, public_key_hex: <public.hex> }]"
echo "  plugins.artifact.enabled = true"
