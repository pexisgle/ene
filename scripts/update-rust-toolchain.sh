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

resolve_rust_toolchain_action_sha() {
  local tag="$1"
  git ls-remote https://github.com/dtolnay/rust-toolchain.git \
    "refs/tags/$tag" "refs/tags/$tag^{}" \
    | awk -v peeled="refs/tags/$tag^{}" '
        $2 == peeled { print $1; resolved = 1; exit }
        NR == 1 { fallback = $1 }
        END { if (!resolved && fallback != "") print fallback }
      '
}

rust_toolchain_action_sha="$(resolve_rust_toolchain_action_sha "$stable_version")"
if [[ ! "$rust_toolchain_action_sha" =~ ^[0-9a-f]{40}$ ]]; then
  rust_toolchain_action_sha="$(resolve_rust_toolchain_action_sha "v1")"
fi
if [[ ! "$rust_toolchain_action_sha" =~ ^[0-9a-f]{40}$ ]]; then
  printf 'Could not resolve dtolnay/rust-toolchain tag %s or v1 to a commit SHA.\n' "$stable_version" >&2
  exit 1
fi

sed -i -E \
  "s/^channel = \"[0-9]+\.[0-9]+\.[0-9]+\"/channel = \"$stable_version\"/" \
  rust-toolchain.toml

workflow_files=(.github/workflows/ci.yml)
for workflow in "${workflow_files[@]}"; do
  sed -i -E \
    -e "s|dtolnay/rust-toolchain@[0-9a-f]{40}( # [0-9]+\.[0-9]+\.[0-9]+)?|dtolnay/rust-toolchain@$rust_toolchain_action_sha # $stable_version|g" \
    -e "s|dtolnay/rust-toolchain@[0-9]+\.[0-9]+\.[0-9]+|dtolnay/rust-toolchain@$rust_toolchain_action_sha # $stable_version|g" \
    "$workflow"
done

pinned_version="$(awk -F'"' '/^channel =/ { print $2; exit }' rust-toolchain.toml)"
if [[ "$pinned_version" != "$stable_version" ]]; then
  printf 'Rust toolchain update did not update rust-toolchain.toml.\n' >&2
  exit 1
fi

for workflow in "${workflow_files[@]}"; do
  expected="dtolnay/rust-toolchain@$rust_toolchain_action_sha # $stable_version"
  if ! grep -Fq "$expected" "$workflow"; then
    printf 'CI workflow %s was not updated to the pinned Rust toolchain Action.\n' "$workflow" >&2
    exit 1
  fi
  if rg -n 'dtolnay/rust-toolchain@(stable|[0-9]+\.[0-9]+\.[0-9]+)' "$workflow"; then
    printf 'CI workflow %s still uses a mutable Rust toolchain Action reference.\n' "$workflow" >&2
    exit 1
  fi
done

printf 'Updated repository Rust toolchain to %s (%s).\n' \
  "$stable_version" "$rust_toolchain_action_sha"
