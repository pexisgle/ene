# はじめに

ene をローカルでビルドして実行し、LLM プロバイダを設定するまでの最短手順です。

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

Desktop は必要なタイミングで設定を読み込み、`EneHandle` を開いて、Performance キューに従って VRM を再生します。

## AI プロバイダを設定する

設定の読み込み順: コンパイル時デフォルト → OS ユーザー設定（または開発時のローカル `assets/settings.json`）→ `ENE_` 環境変数。

まず設定する項目:

- `ai.providers.default` — OpenAI 互換 API の `base_url` と `api_key`
- `ai.tasks.chat` — `provider`、`model`、任意の `max_tokens`
- `character` — フォルダ名（例: `"Alicia"`）またはカードパス
- `store.enabled` — 長期記憶の有効/無効

短い案内は [設定](configure.md)、全フィールドは [設定リファレンス](../reference/configuration/settings.md)。

## 次に読むもの

1. [システム概要](system-overview.md) — クレートと 1 ターンの流れ
2. [CLI](apps/cli.md) / [Desktop](apps/desktop.md)
3. [ツールカタログ](tools/overview.md)
4. 契約・API は [リファレンス](../reference/index.md)
