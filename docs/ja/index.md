# Ene ドキュメント

Ene は Rust で書かれた **ローカル AI コンパニオン型ハーネス**です。コンパニオンは
ユーザーのデバイスに住み、話し、聞き、覚え、承認 plane の下でツールを実行します。
状態はコアデーモン (`ene-core`) が持ち、desktop / CLI / stage / Web は同一 API のクライアントです。

完成形の定義は
[`plans/harness-redesign/`](../../plans/harness-redesign/README.md) にあります。

## このドキュメントの対象

| あなたが… | まず読むもの |
|---|---|
| Ene を動かしたい利用者 | [クイックスタート](quickstart.md) |
| コンパニオンと設定を触る人 | [設定](configuration.md) → [コンセプト](concepts/architecture.md) |
| ツールを足す開発者 | [ツールを書く](guides/tools/write-a-tool.md) |
| このリポジトリの貢献者 | [アーキテクチャ](concepts/architecture.md) → [クレートリファレンス](reference/crates.md) |

## ドキュメント地図

| セクション | 内容 |
|---|---|
| [クイックスタート](quickstart.md) | `ene-ctl` / `ene-desktop` のビルドと起動 |
| [設定](configuration.md) | `settings.json`、環境変数、ファイル位置 |
| [コンセプト](concepts/architecture.md) | プロセスモデル、カード、記憶、プラグイン |
| [アプリ](apps/cli.md) | CLI と desktop のユーザーガイド |
| [ガイド](guides/character-editor.md) | 作業別の手順 |
| [リファレンス](reference/crates.md) | クレート地図と描画 API |

日本語版は [日本語ドキュメント](index.md) です。
`docs/` の各ページには `docs/ja/` に対応ページがあります。

## 正の情報源

ページとコードが食い違ったらコードが勝ちます。食い違いを見つけたら報告してください。

- リポジトリ: <https://github.com/pexisgle/ene>
- Rust API docs: `cargo doc --workspace --no-deps`
