#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VRM_PATH="$REPO_ROOT/assets/characters/Alicia/AliciaSolid.vrm"
MOTIONS_DIR="$REPO_ROOT/assets/characters/Alicia/motions"
MANIFEST="$REPO_ROOT/third_party/assets.json"

force=0
for arg in "$@"; do
  case "$arg" in
    --force) force=1 ;;
    *)
      printf 'usage: %s [--force]\n' "${BASH_SOURCE[0]}" >&2
      printf '  --force  skip SHA-256 verification against third_party/assets.json\n' >&2
      exit 2
      ;;
  esac
done

verify() {
  local path="$1" rel
  rel="${path#"$REPO_ROOT"/}"
  if [[ "$force" -eq 1 ]]; then
    printf 'warning: skipping SHA-256 verification for %s (--force)\n' "$rel" >&2
    return 0
  fi
  if [[ ! -f "$MANIFEST" ]]; then
    printf 'warning: %s is missing; cannot verify %s\n' "$MANIFEST" "$rel" >&2
    return 0
  fi
  local expected actual
  expected="$(jq -er --arg path "$rel" '.assets[] | select(.path == $path) | .sha256' "$MANIFEST" 2>/dev/null)" || {
    printf 'warning: no manifest entry for %s; skipping verification\n' "$rel" >&2
    return 0
  }
  if ! command -v jq >/dev/null 2>&1 || ! command -v sha256sum >/dev/null 2>&1; then
    printf 'warning: jq or sha256sum not found; cannot verify %s\n' "$rel" >&2
    return 0
  fi
  actual="$(sha256sum "$path" | awk '{print $1}')"
  if [[ "$actual" != "$expected" ]]; then
    printf 'SHA-256 MISMATCH for %s\n' "$rel" >&2
    printf '  expected: %s\n  actual:   %s\n' "$expected" "$actual" >&2
    printf 'The file may be a different revision than the one this project was tested with.\n' >&2
    return 1
  fi
  printf 'verified: %s\n' "$rel"
}

vrm_present=0
motions_present=0

if [[ -f "$VRM_PATH" ]]; then
  verify "$VRM_PATH"
  vrm_present=1
else
  cat >&2 <<'EOF'
AliciaSolid.vrm is NOT distributed with this repository.

Its embedded VRM metadata declares allowRedistribution: false, so it must be
obtained from the official page by each user:

  1. Open https://3d.nicovideo.jp/alicia/
  2. Read and agree to the Niconi 3D-chan license terms on that page.
     The agreement step is interactive and cannot be automated here.
  3. Download the VRM file.
  4. Place it at:
       assets/characters/Alicia/AliciaSolid.vrm
EOF
fi

if compgen -G "$MOTIONS_DIR/VRMA_*.vrma" >/dev/null; then
  for motion in "$MOTIONS_DIR"/VRMA_*.vrma; do
    verify "$motion"
  done
  motions_present=1
else
  cat >&2 <<'EOF'
The VRMA motion files (VRMA_01 through VRMA_07) are NOT distributed with this
repository. Extractable-file redistribution is prohibited by their terms:

  1. Open https://booth.pm/ja/items/5512385
  2. Agree to the VRoid Project VRM Animation terms of use on that page.
     The agreement step is interactive and cannot be automated here.
  3. Download the 7-piece motion set and extract it.
  4. Place the .vrma files at:
       assets/characters/Alicia/motions/VRMA_01.vrma ... VRMA_07.vrma
EOF
fi

missing=0
[[ "$vrm_present" -eq 1 ]] || missing=$((missing + 1))
[[ "$motions_present" -eq 1 ]] || missing=$((missing + 1))
if [[ "$missing" -gt 0 ]]; then
  printf '\n%s asset group(s) still missing after following the steps above.\n' "$missing" >&2
  exit 1
fi
printf 'All character assets are present.\n'
