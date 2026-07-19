# はじめに

ene をローカルでビルドして実行し、LLM プロバイダを設定するまでの最短手順です。

## 前提条件

- `rust-toolchain.toml` に沿った Rust ツールチェーン
- Linux では Nix / `direnv` のワークスペースシェルを使い、プロジェクトと同じ `cargo` を使う
- `ene-desktop` をビルドする場合はネイティブ GUI 依存（GTK / Wayland / Windows SDK など）

## ビルド

日常の開発では `ene-cli` のみがビルドされます（ワークスペースの `default-members`）。他のパッケージは必要なときだけ明示的に指定してください。

```bash
cargo build
cargo check -p ene-cli
cargo run -p ene-cli
```

Desktop:

```bash
cargo run -p ene-desktop
```

ワークスペース全体（CI / PR 前の検証）:

```bash
cargo build --workspace
cargo test --workspace
```

リリース:

```bash
cargo build --workspace --release
```

### ビルド性能

dev プロファイルでは、ワークスペース部材のデバッグ情報を最小限（`line-tables-only`）にし、依存クレートの debuginfo は無効化します。重いランタイム依存（`wgpu`、`egui`、`rapier3d` など）だけ `opt-level = 2` で最適化します。`sccache` と Nix シェル経由の `mold` で再ビルドを短縮できます。sccache のキャッシュは `target/` ではなく `~/.cache/sccache` に保存されます。

プロファイル変更後に `target/` のサイズを比較する場合は、一度 `cargo clean` してから計測してください。任意の掃除: `cargo install cargo-sweep` のあと `cargo sweep -t 7`。

デバッガでステップ実行するときは `--profile debugging` を使います。

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
