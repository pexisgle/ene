# テスト

```sh
cargo test --workspace
cargo test -p ene-session
cargo clippy --workspace --all-targets -- -D warnings
```

裸の `cargo test` は `ene-ctl`（default-members）だけです。必ず
`--workspace` か `-p <package>` を付けてください。CI はコアクレート・
アプリ・プラグインの3ジョブに分けます。

テストはコードの隣 (`#[cfg(test)]`) かクレートの `tests/` に置きます。
本番パスで unwrap してはいけません。テストモジュールは
`#![cfg_attr(test, expect(clippy::unwrap_used, ...))]` で外します。

NixOS ではホスト PATH に Cargo が無いことがあります。リポジトリルートの
direnv ラッパか `nix develop --command` を使ってください。
