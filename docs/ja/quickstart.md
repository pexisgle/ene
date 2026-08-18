# クイックスタート

このページは、チェックアウトからデーモンとクライアント起動までの最短経路です。
ビルド環境の詳細はリポジトリの `AGENTS.md` にあります。

## 1. 要件

- **Linux** だけが開発・CI の対象です。Windows は Linux からのクロスコンパイル、
  macOS は非対応です。
- **Rust ≥ 1.85**（edition 2024）。CI は stable です。
- ネイティブ依存: Vulkan、ALSA、OpenSSL、`libclang`、`mold`、Wayland/X11。
  checked-in の Nix flake が全部出します。

```sh
nix develop --command cargo build --workspace
```

`direnv` が有効なら、そのまま `cargo` で構いません。

## 2. ビルド

```sh
cargo build --workspace
cargo build -p ene-ctl
```

## 3. CLI を動かす

```sh
cargo run -p ene-ctl -- core start
cargo run -p ene-ctl -- --help
```

`ene-ctl` は stage / Web と同じ HTTP/WS API を使います。自分で `ene-core` を
起動した場合は `--url` と `--token`（または `ENE_API_URL` / `ENE_API_TOKEN`）
を合わせてください。

## 4. stage を動かす

```sh
cargo run -p ene-stage
```

stage は必要なら `ene-core` を子プロセスとして起動し、ネイティブウィンドウに
コンパニオンを出し、詳細は別窓です。

会話プロバイダプラグインが無い間、応答は Echo のみです。

## 5. テスト

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

必ず `--workspace` か `-p <package>` を付けてください。default-members は
`ene-ctl` だけです。
