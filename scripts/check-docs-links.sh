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

# Fragment-only links resolve within the page itself, so only the
# path portion can be checked against the filesystem.
verify_relative_link() {
  local file="$1" dir="$2" link="$3" target
  target="${link%%#*}"
  [[ -z "$target" ]] && return 0
  if [[ ! -e "$dir/$target" ]]; then
    printf 'docs-links: broken link in %s: (%s)\n' "$file" "$link" >&2
    fail=1
  fi
}

verify_file_links() {
  local file="$1" dir link
  dir="$(dirname "$file")"
  while IFS= read -r link; do
    if ! is_skipped_link "$link"; then
      verify_relative_link "$file" "$dir" "$link"
    fi
  done < <(extract_links "$file")
}

# JA summary entries point back into docs/ via ../ prefixes;
# stripping them makes JA paths directly comparable to EN paths.
summary_paths() {
  extract_links "$1" \
    | sed -E -e 's/#.*$//' -e 's#^(\./|\.\./)+##' \
    | while IFS= read -r link; do
      if ! is_skipped_link "$link"; then
        printf '%s\n' "$link"
      fi
    done \
    | sort -u
}

for file in docs/SUMMARY.md docs/ja/SUMMARY.md; do
  verify_file_links "$file"
done

shopt -s globstar nullglob
for file in docs/**/*.md; do
  case "$file" in
    docs/book/* | docs/ja/book/*) continue ;;
    docs/SUMMARY.md | docs/ja/SUMMARY.md) continue ;;
  esac
  verify_file_links "$file"
done

en_paths="$(summary_paths docs/SUMMARY.md)"
ja_paths="$(summary_paths docs/ja/SUMMARY.md)"
missing_in_ja="$(comm -23 <(printf '%s\n' "$en_paths") <(printf '%s\n' "$ja_paths"))"
missing_in_en="$(comm -13 <(printf '%s\n' "$en_paths") <(printf '%s\n' "$ja_paths"))"

if [[ -n "$missing_in_ja" ]]; then
  printf 'docs-links: entries missing from docs/ja/SUMMARY.md:\n%s\n' "$missing_in_ja" >&2
  fail=1
fi
if [[ -n "$missing_in_en" ]]; then
  printf 'docs-links: entries missing from docs/SUMMARY.md:\n%s\n' "$missing_in_en" >&2
  fail=1
fi

if [[ "$fail" -ne 0 ]]; then
  printf 'docs-links: documentation link check failed.\n' >&2
fi
exit "$fail"
