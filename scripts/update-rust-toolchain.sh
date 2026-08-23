#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

stable_version="$(curl --fail --silent --show-error --location \
  https://static.rust-lang.org/dist/channel-rust-stable.toml \
  | awk '
      /^\[pkg\.rust\]$/ { in_rust = 1; next }
      in_rust && /^\[/ { in_rust = 0 }
      in_rust && !found && /^version = / {
        version = $3
        gsub(/"/, "", version)
        sub(/\(.*/, "", version)
        print version
        found = 1
      }
    ')"

if [[ ! "$stable_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  printf 'Could not determine the stable Rust version.\n' >&2
  exit 1
fi

sed -i -E \
  "s/^channel = \"[0-9]+\.[0-9]+\.[0-9]+\"/channel = \"$stable_version\"/" \
  rust-toolchain.toml

sed -i -E \
  "s#dtolnay/rust-toolchain@[0-9]+\.[0-9]+\.[0-9]+#dtolnay/rust-toolchain@$stable_version#g" \
  .github/workflows/ci.yml .github/workflows/tmp-merge-1011.yml

for file in README.md docs/quickstart.md docs/ja/quickstart.md \
  docs/guides/rust-toolchain.md docs/ja/guides/rust-toolchain.md; do
  sed -i -E "s/Rust [0-9]+\.[0-9]+\.[0-9]+/Rust $stable_version/g" "$file"
done

pinned_version="$(awk -F'"' '/^channel =/ { print $2; exit }' rust-toolchain.toml)"
if [[ "$pinned_version" != "$stable_version" ]]; then
  printf 'Rust toolchain update did not update rust-toolchain.toml.\n' >&2
  exit 1
fi

if rg -n 'dtolnay/rust-toolchain@stable' .github/workflows/ci.yml .github/workflows/tmp-merge-1011.yml; then
  printf 'A CI workflow still uses the mutable stable toolchain reference.\n' >&2
  exit 1
fi

printf 'Updated repository Rust toolchain to %s.\n' "$stable_version"
