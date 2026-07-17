# はじめに

ene をローカルでビルドして動かし、LLM プロバイダを繋ぐまでの最短ルートです。

## 前提条件

- `rust-toolchain.toml` に沿った Rust ツールチェーン
- Linux では Nix / `direnv` のワークスペースシェルを使い、プロジェクトと同じ `cargo` を使う
- `ene-desktop` をビルドする場合はネイティブ GUI 依存（GTK / Wayland / Windows SDK など）

## ビルド

```bash
cargo build --workspace
```

リリース:

```bash
cargo build --workspace --release
```

## CLI を実行

```bash
cargo run -p ene-cli -- --help
cargo run -p ene-cli
```

## Desktop を実行

```bash
cargo run -p ene-desktop --release
```

Desktop は必要時に設定を読み、`EneHandle` を開き、Performance キューで VRM を再生します。

## プロバイダを設定する

設定の読み込み順: コンパイル時デフォルト → OS ユーザー設定（または開発時のローカル `assets/settings.json`）→ `ENE_` 環境変数。

まず触る項目:

- `provider.base_url` / `provider.model` / `provider.api_key`
- `character` — カード名またはパス
- `store.enabled` — 長期記憶の有効/無効

短い案内は [設定](configure.md)、全フィールドは [設定リファレンス](../reference/configuration/settings.md)。

## 次に読むもの

1. [システム概要](system-overview.md) — クレートと 1 ターンの流れ
2. [CLI](apps/cli.md) / [Desktop](apps/desktop.md)
3. [ツールカタログ](tools/overview.md)
4. 契約・API は [リファレンス](../reference/index.md)
