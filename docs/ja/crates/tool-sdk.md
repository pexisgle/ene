# ツール開発関連クレート

> **クレート**: `ene-plugin-db` | `ene-tool-macros`

ツールプラグインの開発、ツールバイナリのステートフルなストレージアクセス、ツール定義ボイラープレート用の proc-macro を支援するヘルパーライブラリクレート群です。RAG によるツール探索は `ene-rag` に属します（[RAG ポリシー層](rag.md)を参照）。

`ToolAction` トレイト、`ActionSetProvider`/`SingleActionProvider` アダプタ、およびツール開発用の `prelude` は **`ene-plugin`** 本体に配置されています（[プラグインシステム関連クレート](plugin-system.md)を参照）。これらはかつての独立クレート `ene-tool-sdk` から統合され、同クレートは削除されました。

---

## アーキテクチャ境界

- `ene-plugin` はツール開発サーフェス（`ToolAction`、`ActionSetProvider`、`prelude::tool`）を所有します。ワークスペース内の依存先は `ene-plugin-proto`（ワイヤプロトコル）、`ene-infer`（ローカル推論の規律）、`ene-tool-macros`（proc-macro derive）のみであり、それ以外は標準的な async/シリアライズ系エコシステムクレート（`tokio`、`tokio-stream`、`tokio-util`、`async-trait`、`schemars`、`serde`/`serde_json`、`tracing`、`thiserror`、`parking_lot`、`base64`）です。`ene-runtime`、`ene-mind`、`ene-store` には依存しません。
- `ene-plugin-db` は状態を保持するツールバイナリ (例: ファイルシステムの Undo 履歴、TODO ストア) が使用する IPC *クライアント* です。独自のデータベース接続を開く代わりに、ホストの `db_server` (`ene-store` が所有) とソケット越しに通信します — 状態を保持するツールバイナリが2つ目の SQLite ライターになることはありません。
- `ene-tool-macros` は proc-macro のみのクレートです: `#[tool(...)]`/`#[arg(...)]` 属性からコンパイル時に `ToolSpec`/`ToolAction` のボイラープレートを生成し、それ自体のランタイムロジックは持ちません。生成コードは `::ene_plugin::` パスを参照します。
- RAG によるツール選択はこれらの SDK クレートではなく `ene-rag` (`tool` モジュール) が所有します。詳細は [RAG ポリシー層](rag.md) を参照してください。

## 設計思想

- **なぜツール SDK を `ene-plugin` に統合したか**: `ene-plugin` / `ene-tool-sdk` の分離は「汎用プラグインファサード」と「ツール専用シュガー」を分ける意図でしたが、`ene-plugin` はすでにツール固有の型（`ToolPlugin`、`ToolProviderPlugin`）を無条件で公開しており、`html`/`truncate` を `ene-util` に切り出した後（#300）は `ene-tool-sdk` に依存隔離の効果も残っていませんでした。2クレート維持はゼロのメリットに対して cargo feature 管理コストだけを増やしていました。
- **なぜツール定義に derive マクロを使うか**: 各ツールアクションは JSON スキーマ、引数のデシリアライズ、`ToolAction`/`ToolSpec` の連携コードを必要とし、これを毎回手書きすると重複が発生します。`#[derive(ToolAction)]` は構造体のフィールドと `#[tool(...)]`/`#[arg(...)]` 属性からこれらを生成するため、開発者は `run` メソッドだけを書けば済みます。
- **なぜ `ene-tool-macros` が `ene-plugin` の省略不可な依存なのか**: この proc-macro クレートは非常に小さく、`schemars`/`syn` はすでに `ene-plugin-proto` 経由で依存グラフに存在するため、`tool` feature でゲートしても依存の隔離というメリットは得られません。一方でツールプラグイン向けの `use ene_plugin::prelude::*;` を複雑化させてしまうだけで、実質的な利益はゼロです。
- **なぜ検索拡張生成 (RAG) によるツール選択が存在するか**: 利用可能なツールの数が増えるにつれ、毎ターン全ツールの完全なスキーマを送信すると、大きく、かつ無制限にプロンプトのトークン予算を消費してしまいます。`ene-rag` のツールパイプラインはツールの説明文/パラメータをインデックス化し、埋め込み類似度とオプションの LLM 再ランクを用いて、現在のターンに関連するツールのみを注入します。
- **ツール設計思想 — メガツール vs 個別ツール**: ツールプラグイン群は現在、2つのアーキテクチャパターンを用いています。(1) **メガツール方式**（fs、app、browser）はドメインごとに1つのバイナリを提供し、内部で多数のアクションをディスパッチします。プロセスのオーバーヘッドと IPC の往復を最小化し、1プロセス内でアクション同士が状態を共有できます。(2) **個別ツール方式**（web、utility）はそれぞれが単一の責務に集中した複数の小さなプラグインを提供し、Tool RAG におけるセマンティックマッチングの精度を高めます。将来的にどちらか一方へ統一する可能性はありますが、まだ決定していません。新しいツールを設計する際は、具体的なユースケースに応じて起動オーバーヘッドと検索精度のトレードオフを天秤にかけてください。

## API リファレンス

構造体・メソッドのシグネチャはここには転記しません — 転記すると必ず陳腐化します。最新かつ正確な API は rustdoc を生成して参照してください:

```sh
cargo doc -p ene-plugin --open
cargo doc -p ene-plugin-db --open
cargo doc -p ene-tool-macros --open
```

開発用には `ene_plugin::prelude`、`ToolAction`/`ToolSpec` derive マクロの詳細は `ene-tool-macros` から始めてください。

---

## 関連ドキュメント
- [プラグインと MCP システム](../concepts/plugins-and-mcp.md)
- [プラグインシステム関連クレート](plugin-system.md)
