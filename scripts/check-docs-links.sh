#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

fail=0

extract_links() {
  grep -oE '\]\([^)]+\)' "$1" \
    | sed -e 's/^\](//' -e 's/)$//' \
    || true
}

is_skipped_link() {
  case "$1" in
    '' | '#'*) return 0 ;;
    http://* | https://* | ftp://* | mailto:*) return 0 ;;
    *) return 1 ;;
  esac
}

verify_file_links() {
  local file="$1" dir link target
  dir="$(dirname "$file")"
  while IFS= read -r link; do
    if is_skipped_link "$link"; then
      continue
    fi
    target="${link%%#*}"
    [[ -z "$target" ]] && continue
    if [[ ! -e "$dir/$target" ]]; then
      printf 'docs-links: broken link in %s: (%s)\n' "$file" "$link" >&2
      fail=1
    fi
  done < <(extract_links "$file")
}

shopt -s globstar nullglob
for file in docs/**/*.md; do
  # Legacy documents are preserved as historical snapshots. Their relative links
  # were authored at older repository paths and are intentionally not maintained.
  case "$file" in
    docs/requirements/legacy/*) continue ;;
  esac
  verify_file_links "$file"
done

if [[ "$fail" -ne 0 ]]; then
  printf 'docs-links: documentation link check failed.\n' >&2
fi
exit "$fail"
