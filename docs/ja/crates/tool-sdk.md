# ツール開発関連クレート

> **クレート**: `ene-plugin-db` | `ene-tool-macros` | `ene-tool-rag`

ツールプラグインの開発、ツールバイナリのステートフルなストレージアクセス、ツール定義ボイラープレート用の proc-macro、および RAG によるツール探索を支援するヘルパーライブラリクレート群です。

`ToolAction` トレイト、`ActionSetProvider`/`SingleActionProvider` アダプタ、およびツール開発用の `prelude` は **`ene-plugin`** 本体に配置されています（[プラグインシステム関連クレート](plugin-system.md)を参照）。これらはかつての独立クレート `ene-tool-sdk` から統合され、同クレートは削除されました。

---

## アーキテクチャ境界

- `ene-plugin` はツール開発サーフェス（`ToolAction`、`ActionSetProvider`、`prelude::tool`）を所有します。依存先は `ene-plugin-proto` と `ene-infer` のみであり、`ene-runtime`、`ene-mind`、`ene-store` には依存しません。
- `ene-plugin-db` は状態を保持するツールバイナリ (例: ファイルシステムの Undo 履歴、TODO ストア) が使用する IPC *クライアント* です。独自のデータベース接続を開く代わりに、ホストの `db_server` (`ene-store` が所有) とソケット越しに通信します — 状態を保持するツールバイナリが2つ目の SQLite ライターになることはありません。
- `ene-tool-macros` は proc-macro のみのクレートです: `#[tool(...)]`/`#[arg(...)]` 属性からコンパイル時に `ToolSpec`/`ToolAction` のボイラープレートを生成し、それ自体のランタイムロジックは持ちません。生成コードは `::ene_plugin::` パスを参照します。
- `ene-tool-rag` は `ene-ai` (埋め込み)、`ene-store` (インデックス保存)、`ene-plugin-proto` (ツール型) に依存し、トークン予算の制約下でプロンプトパケットに注入するツールを選択するために `ene-runtime` から利用されます。

## 設計思想

- **なぜツール SDK を `ene-plugin` に統合したか**: `ene-plugin` / `ene-tool-sdk` の分離は「汎用プラグインファサード」と「ツール専用シュガー」を分ける意図でしたが、`ene-plugin` はすでにツール固有の型（`ToolPlugin`、`ToolProviderPlugin`）を無条件で公開しており、`html`/`truncate` を `ene-util` に切り出した後（#300）は `ene-tool-sdk` に依存隔離の効果も残っていませんでした。2クレート維持はゼロのメリットに対して cargo feature 管理コストだけを増やしていました。
- **なぜツール定義に derive マクロを使うか**: 各ツールアクションは JSON スキーマ、引数のデシリアライズ、`ToolAction`/`ToolSpec` の連携コードを必要とし、これを毎回手書きすると重複が発生します。`#[derive(ToolAction)]` は構造体のフィールドと `#[tool(...)]`/`#[arg(...)]` 属性からこれらを生成するため、開発者は `run` メソッドだけを書けば済みます。
- **なぜ検索拡張生成 (RAG) によるツール選択が存在するか**: 利用可能なツールの数が増えるにつれ、毎ターン全ツールの完全なスキーマを送信すると、大きく、かつ無制限にプロンプトのトークン予算を消費してしまいます。`ene-tool-rag` はツールの説明文/パラメータをインデックス化し、埋め込み類似度とオプションの LLM 再ランクを用いて、現在のターンに関連するツールのみを注入します。

## API リファレンス

構造体・メソッドのシグネチャはここには転記しません — 転記すると必ず陳腐化します。最新かつ正確な API は rustdoc を生成して参照してください:

```sh
cargo doc -p ene-plugin --open
cargo doc -p ene-plugin-db --open
cargo doc -p ene-tool-macros --open
cargo doc -p ene-tool-rag --open
```

開発用には `ene_plugin::prelude`、`ToolAction`/`ToolSpec` derive マクロの詳細は `ene-tool-macros` から始めてください。

---

## 関連ドキュメント
- [プラグインと MCP システム](../concepts/plugins-and-mcp.md)
- [プラグインシステム関連クレート](plugin-system.md)
