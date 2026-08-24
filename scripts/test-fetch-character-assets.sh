#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
pass=0
fail=0

run_case() {
  local label="$1" expected_exit="$2"
  shift 2
  local tmp_dir
  tmp_dir="$(mktemp -d)"
  mkdir -p "$tmp_dir/scripts" "$tmp_dir/third_party"
  cp "$SCRIPT_DIR/fetch-character-assets.sh" "$tmp_dir/scripts/"
  cp "$SCRIPT_DIR/../third_party/assets.json" "$tmp_dir/third_party/"
  "$@" "$tmp_dir"
  set +e
  bash "$tmp_dir/scripts/fetch-character-assets.sh" --force >"$tmp_dir/out.log" 2>&1
  local actual_exit=$?
  set -e
  if [[ $actual_exit -eq $expected_exit ]]; then
    printf 'PASS: %s\n' "$label"
    pass=$((pass + 1))
  else
    printf 'FAIL: %s (expected exit %d, got exit %d)\n' "$label" "$expected_exit" "$actual_exit"
    cat "$tmp_dir/out.log" >&2
    fail=$((fail + 1))
  fi
  rm -rf "$tmp_dir"
}

# All absent: bootstrap flow must still succeed.
run_case "all absent" 0 true

# Partial placement: some but not all VRMA files present must fail.
partial_place() {
  local dir="$1"
  mkdir -p "$dir/assets/characters/Alicia/motions"
  for f in VRMA_01 VRMA_02 VRMA_03; do
    printf 'stub' > "$dir/assets/characters/Alicia/motions/$f.vrma"
  done
}
run_case "partial placement (3 of 7)" 1 partial_place

# Complete motion set without VRM: only the VRM group is missing, exit 0.
complete_motions() {
  local dir="$1" i
  mkdir -p "$dir/assets/characters/Alicia/motions"
  for i in 1 2 3 4 5 6 7; do
    printf 'stub-%02d' "$i" > "$dir/assets/characters/Alicia/motions/VRMA_0$i.vrma"
  done
}
run_case "complete motions, no vrm" 0 complete_motions

printf '\n%d passed, %d failed\n' "$pass" "$fail"
[[ $fail -eq 0 ]]
