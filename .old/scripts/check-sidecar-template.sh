#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

fixture_dir="plugins/provider/sidecar-smoke"
lock_backup="$(mktemp)"
cp Cargo.lock "$lock_backup"

cleanup() {
  rm -rf "$fixture_dir"
  cp "$lock_backup" Cargo.lock
  rm -f "$lock_backup"
}
trap cleanup EXIT

if [[ -e "$fixture_dir" ]]; then
  printf 'fixture path already exists: %s\n' "$fixture_dir" >&2
  exit 1
fi

mkdir -p "$fixture_dir/src"
cp templates/sidecar/Cargo.toml "$fixture_dir/Cargo.toml"
sed -i '1i\
[package]\
name = "ene-provider-sidecar-smoke"\
version.workspace = true\
edition.workspace = true\
publish = false\
\
[[bin]]\
name = "ene-provider-sidecar-smoke"\
path = "src/main.rs"\
\
' "$fixture_dir/Cargo.toml"
printf '%s\n' 'mod sidecar;' '' 'fn main() {}' > "$fixture_dir/src/main.rs"

bash templates/sidecar/new-sidecar.sh sidecar-smoke
cargo check --manifest-path "$fixture_dir/Cargo.toml"

printf 'sidecar template fixture compiled successfully.\n'
