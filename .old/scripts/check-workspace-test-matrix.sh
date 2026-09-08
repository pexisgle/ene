#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

matrix_packages="$tmp_dir/matrix-packages"
workspace_packages="$tmp_dir/workspace-packages"
sorted_matrix_packages="$tmp_dir/sorted-matrix-packages"

awk '
  /^  test:$/ { in_test = 1 }
  in_test && /^    steps:$/ { exit }
  in_test { print }
' .github/workflows/ci.yml \
  | { grep -oE -- '-p [[:alnum:]_-]+' || true; } \
  | awk '{ print $2 }' > "$matrix_packages"

if [[ ! -s "$matrix_packages" ]]; then
  printf 'could not find test matrix packages in .github/workflows/ci.yml\n' >&2
  exit 1
fi

sort "$matrix_packages" > "$sorted_matrix_packages"
duplicates="$(uniq -d "$sorted_matrix_packages")"
if [[ -n "$duplicates" ]]; then
  printf 'test matrix contains duplicate packages:\n%s\n' "$duplicates" >&2
  exit 1
fi

cargo metadata --no-deps --format-version 1 --locked \
  | jq -r '.packages[].name' \
  | sort -u > "$workspace_packages"

if ! diff -u "$workspace_packages" "$sorted_matrix_packages"; then
  printf 'CI test matrix does not match the Cargo workspace package set.\n' >&2
  exit 1
fi

printf 'CI test matrix covers every workspace package exactly once.\n'
