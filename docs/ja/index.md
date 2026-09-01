# Ene ドキュメント

Ene は Rust で書かれたローカル AI コンパニオン型ハーネスです。このドキュメントでは、意図的に次の2種類を分離します。

- **要件**: Ene が何であるべきかを定義します。現在、対話的に作り直しています。確定済み要件が製品仕様の正です。
- **現行実装の説明**: 今のコードがどう動くかを説明します。ここに書かれている実装都合を、そのまま製品要件として扱いません。

## まず読むもの

| あなたが… | まず読むもの |
|---|---|
| 製品像・仕様を決めたい | [要件定義](requirements/README.md) |
| Ene を動かしたい | [クイックスタート](quickstart.md) |
| 設定を触りたい | [設定](configuration.md) |
| ツールを追加したい | [ツールを書く](guides/tools/write-a-tool.md) |
| 現在のコードを把握したい | [アーキテクチャ](concepts/architecture.md) → [クレートリファレンス](reference/crates.md) |

## ドキュメント地図

| セクション | 内容 |
|---|---|
| [要件定義](requirements/README.md) | 製品定義、ユースケース、機能・非機能要件、不変条件、用語、決定 |
| [クイックスタート](quickstart.md) | ビルド・設定・起動 |
| [設定](configuration.md) | 設定、環境変数、ファイル位置 |
| [コンセプト](concepts/architecture.md) | 現在実装の説明（要件ではない） |
| [アプリ](apps/stage.md) | 現在の stage / CLI / 旧 desktop |
| [ガイド](guides/character-editor.md) | 現在実装に対する作業別手順 |
| [リファレンス](reference/crates.md) | 現在のクレート地図・API |

## 正の情報源

**あるべき製品挙動**は `docs/requirements/` の確定済み文書、**現在の挙動・シグネチャ**はコードと rustdoc が正です。`docs/requirements/legacy/` は過去の判断を再確認するための参考資料であり、現在の要件ではありません。

- リポジトリ: <https://github.com/pexisgle/ene>
- Rust API docs: `cargo doc --workspace --no-deps`
