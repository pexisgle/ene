#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
script="$repo_root/scripts/check-docs-links.sh"

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

# summary_paths() is defined inside check-docs-links.sh; source only its
# functions by stubbing the main body via an early exit marker.
functions_only="$(sed -n '/^extract_links()/,/^for file in/p' "$script" | sed '$d')"
eval "$functions_only"

printf '[foo](foo.md)\n' > "$tmpdir/en.md"
printf '[foo](../foo.md)\n' > "$tmpdir/ja.md"

en="$(summary_paths "$tmpdir/en.md")"
ja="$(summary_paths "$tmpdir/ja.md")"

diff_out="$(comm -3 <(printf '%s\n' "$en") <(printf '%s\n' "$ja"))"
if [[ -n "$diff_out" ]]; then
  printf 'test-check-docs-links: summary path normalization mismatch:\n%s\n' "$diff_out" >&2
  exit 1
fi

printf 'test-check-docs-links: ok\n'
