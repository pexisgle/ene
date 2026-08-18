# ene

ene は、ローカルで動く AI コンパニオン型エージェントハーネスです。コアデーモン
(`ene-core`) が会話・記憶・ツール・承認を持ち、stage / CLI / Web がその同一 API に
接続します。

## 概要
- **ワークスペース構成**: 複数クレートを含む Cargo ワークスペースです。
- **主なクレート / アプリ**:
  - `ene-session`: 追記専用の会話ログと usage 台帳
  - `ene-kernel`: 対話レーン (`prompt` / `steer` / `follow_up` / `abort` / `compact`)
  - `ene-companion`: soul・感情・記憶・内面・能動発話・キャラパッケージ
  - `ene-daemon` (`apps/ene-core`, バイナリ `ene-core`): コアデーモン
  - `ene-stage`: ネイティブ stage クライアント (egui + wgpu)
  - `ene-ctl`: CLI クライアント

## リポジトリ構造（抜粋）
- `crates/ene-session/` — セッションログ
- `crates/ene-kernel/` — 対話レーン
- `apps/ene-core/` — コアデーモン
- `apps/ene-stage/` — stage クライアント
- `apps/ene-ctl/` — CLI
- `plugins/harness/` — 同梱ツール (`fs` / `exec` / `web` / `utility`)
- `assets/` — サンプルキャラクターやアセット

## 前提条件
- Rust（ツールチェーンは `rust-toolchain.toml` を参照）。
- ネイティブ依存は checked-in の Nix flake（Vulkan、ALSA、OpenSSL、mold など）。

## ビルド

```bash
cargo build --workspace
```

## 実行

```bash
cargo run -p ene-ctl -- --help
cargo run -p ene-stage
cargo run -p ene-daemon
```

## テスト

```bash
cargo test --workspace
```

## 開発メモ
- ホスト契約は HTTP/WS (`ene-api`)。詳細は [`docs/index.md`](docs/index.md) および
  [`docs/concepts/architecture.md`](docs/concepts/architecture.md)。
- 設計文書は [`plans/harness-redesign/`](plans/harness-redesign/README.md)。
