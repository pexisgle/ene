# ツール SDK 関連クレート

> **クレート**: `ene-tool-sdk` | `ene-plugin-db` | `ene-tool-macros`

ツールプラグインの開発、ツールバイナリのステートフルなストレージアクセス、ツール定義ボイラープレート用の proc-macro を支援するヘルパーライブラリクレート群です。RAG によるツール探索は `ene-rag` に属します（[RAG ポリシー層](rag.md)を参照）。

---

## アーキテクチャ境界

- `ene-tool-sdk` は `ToolAction` トレイト、`ActionSetProvider`、`prelude` を提供します。依存先は `ene-plugin` (さらにその依存先は `ene-plugin-proto` のみ) であり、`ene-runtime`、`ene-mind`、`ene-store` には依存しません。
- `ene-plugin-db` は状態を保持するツールバイナリ (例: ファイルシステムの Undo 履歴、TODO ストア) が使用する IPC *クライアント* です。独自のデータベース接続を開く代わりに、ホストの `db_server` (`ene-store` が所有) とソケット越しに通信します — 状態を保持するツールバイナリが2つ目の SQLite ライターになることはありません。
- `ene-tool-macros` は proc-macro のみのクレートです: `#[tool(...)]`/`#[arg(...)]` 属性からコンパイル時に `ToolSpec`/`ToolAction` のボイラープレートを生成し、それ自体のランタイムロジックは持ちません。
- RAG によるツール選択はこれらの SDK クレートではなく `ene-rag` (`tool` モジュール) が所有します。詳細は [RAG ポリシー層](rag.md) を参照してください。

## 設計思想

- **なぜツール定義に derive マクロを使うか**: 各ツールアクションは JSON スキーマ、引数のデシリアライズ、`ToolAction`/`ToolSpec` の連携コードを必要とし、これを毎回手書きすると重複が発生します。`#[derive(ToolAction)]` は構造体のフィールドと `#[tool(...)]`/`#[arg(...)]` 属性からこれらを生成するため、開発者は `run` メソッドだけを書けば済みます。
- **なぜ検索拡張生成 (RAG) によるツール選択が存在するか**: 利用可能なツールの数が増えるにつれ、毎ターン全ツールの完全なスキーマを送信すると、大きく、かつ無制限にプロンプトのトークン予算を消費してしまいます。`ene-rag` のツールパイプラインはツールの説明文/パラメータをインデックス化し、埋め込み類似度とオプションの LLM 再ランクを用いて、現在のターンに関連するツールのみを注入します。

## API リファレンス

構造体・メソッドのシグネチャはここには転記しません — 転記すると必ず陳腐化します。最新かつ正確な API は rustdoc を生成して参照してください:

```sh
cargo doc -p ene-tool-sdk --open
cargo doc -p ene-plugin-db --open
cargo doc -p ene-tool-macros --open
```

開発用には `ene_tool_sdk::prelude`、`ToolAction`/`ToolSpec` derive マクロの詳細は `ene-tool-macros` から始めてください。

---

## 関連ドキュメント
- [プラグインと MCP システム](../concepts/plugins-and-mcp.md)
- [プラグインシステム関連クレート](plugin-system.md)
