#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VRM_PATH="$REPO_ROOT/assets/characters/Alicia/AliciaSolid.vrm"
MOTIONS_DIR="$REPO_ROOT/assets/characters/Alicia/motions"
MANIFEST="$REPO_ROOT/third_party/assets.json"
REQUIRED_MOTIONS=(
  "$MOTIONS_DIR/VRMA_01.vrma"
  "$MOTIONS_DIR/VRMA_02.vrma"
  "$MOTIONS_DIR/VRMA_03.vrma"
  "$MOTIONS_DIR/VRMA_04.vrma"
  "$MOTIONS_DIR/VRMA_05.vrma"
  "$MOTIONS_DIR/VRMA_06.vrma"
  "$MOTIONS_DIR/VRMA_07.vrma"
)

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
  cat >&2 <<'VRM_HELP_END'
AliciaSolid.vrm is NOT distributed with this repository.

Its embedded VRM metadata declares allowRedistribution: false, so it must be
obtained from the official page by each user:

  1. Open https://3d.nicovideo.jp/alicia/
  2. Read and agree to the Niconi 3D-chan license terms on that page.
     The agreement step is interactive and cannot be automated here.
  3. Download the VRM file.
  4. Place it at:
       assets/characters/Alicia/AliciaSolid.vrm
VRM_HELP_END
fi

# Each of the seven motion files is required by the manifest as part of one
# set; checking them individually catches partial placement that a directory
# glob would accept.
missing_motions=()
for motion in "${REQUIRED_MOTIONS[@]}"; do
  if [[ -f "$motion" ]]; then
    verify "$motion"
  else
    missing_motions+=("$motion")
  fi
done

if [[ ${#missing_motions[@]} -eq ${#REQUIRED_MOTIONS[@]} ]]; then
  cat >&2 <<'MOTION_HELP_END'
The VRMA motion files (VRMA_01 through VRMA_07) are NOT distributed with this
repository. Extractable-file redistribution is prohibited by their terms:

  1. Open https://booth.pm/ja/items/5512385
  2. Agree to the VRoid Project VRM Animation terms of use on that page.
     The agreement step is interactive and cannot be automated here.
  3. Download the 7-piece motion set and extract it.
  4. Place the .vrma files at:
       assets/characters/Alicia/motions/VRMA_01.vrma ... VRMA_07.vrma
MOTION_HELP_END
elif [[ ${#missing_motions[@]} -gt 0 ]]; then
  printf '\nThe motion set is incomplete; the following %d of %d files are missing:\n' \
    "${#missing_motions[@]}" "${#REQUIRED_MOTIONS[@]}" >&2
  for motion in "${missing_motions[@]}"; do
    printf '  %s\n' "${motion#"$REPO_ROOT"/}" >&2
  done
  printf '\nA partially extracted set cannot be used; place all seven files and rerun.\n' >&2
  exit 1
else
  motions_present=1
fi

missing=0
[[ "$vrm_present" -eq 1 ]] || missing=$((missing + 1))
[[ "$motions_present" -eq 1 ]] || missing=$((missing + 1))
[[ "$missing" -eq 0 ]] && exit 0
printf '\n%s asset group(s) still missing; follow the steps above and rerun to verify.\n' "$missing" >&2
