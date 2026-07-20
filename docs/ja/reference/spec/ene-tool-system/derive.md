# `ToolSpec` & `ToolAction` コード生成マクロ仕様 (`ene-tool-derive`)

`ene-tool-derive` クレートは、ツールの引数用構造体から LLM 提示用の JSON スキーマ（`ToolSpec`）や、実行ハンドラトレイト（`ToolAction`）のボイラープレートコードをコンパイル時に自動生成する proc-macro マクロライブラリです。

---

## 1. `#[derive(ToolSpec)]` 仕様

このマクロは、構造体の定義と doc コメントをパースし、型安全な JSON Schema を構築します。

### スキーマ生成手順
1.  **基本スキーマの構築**: `schemars` クレートを用いて、構造体のフィールド型（`String`, `i32` 等）から JSON Schema のプロパティ定義を抽出。
2.  **`additionalProperties: false` の強制**:
    LLM がスキーマ定義に存在しない架空の引数を勝手にでっち上げて送信するのを防ぐため、生成される JSON Schema のルートオブジェクトに `additionalProperties: false` 属性を強制挿入します。
3.  **メタデータの反映**: 構造体およびフィールドにアノテーションされた `#[tool(...)]` および `#[arg(...)]` 属性から、説明文やキーワードリスト、カテゴリ情報を抽出し、`ToolSpec` 構造体に展開します。
4.  **定数（Constants）の自動生成**:
    ツール名などのスペルミスを防ぐため、コンパイル時に以下の `pub const` 定数定義を自動で出力します。
    -   `pub const TOOL_NAME: &'static str = "namespace.name";`
    -   `pub const DISPLAY_NAME: &'static str = "...";`

---

## 2. 属性リファレンス (Attributes)

### 1. コンテナ属性 `#[tool(...)]` (構造体自身)
*   `namespace: String`: ツールの名前空間（例: `fs`, `web`）。
*   `name: String`: ツールのアクション名（例: `read`, `search`）。
*   `summary: String`: ツールの機能概要（LLMが参照）。
*   `category: String`: ツールカテゴリ（例: `Filesystem`, `Utility`）。
*   `side_effects: bool`: 実行によって状態変化や外部破壊（ファイルの書き換えなど）が発生するかどうか。
*   `sandbox_required: bool`: サンドボックス環境での実行が必須かどうか。
*   `keywords_primary / keywords_secondary`: ツールRAG検索用のポジティブマッチ用単語リスト。
*   `keywords_negative`: RAGでこのツールを除外するためのペナルティ対象キーワードリスト。

### 2. フィールド属性 `#[arg(...)]` / `#[tool(...)]`
*   `description: String`: 引数フィールドの説明（指定が無い場合はフィールドの doc コメントが使用されます）。
*   `enum_values: Vec<String>`: 引数として許容される文字列候補値の制限。
*   `min / max`: 数値引数の許容最小値/最大値。
*   `skip`: **状態変数のスキップ指示**。LLMにはこのフィールドを見せず、JSONのシリアライズ対象からも除外します。

---

## 3. `#[derive(ToolAction)]` 仕様と実行フロー

`ToolAction` マクロは、スキーマ生成に加え、ツールのアクション実行トレイト `ene_tool_common::ToolAction` の実装を自動生成します。

### 自動生成される `execute` メソッドの制御フロー
```rust
async fn execute(&self, arguments_json: &str) -> Result<String, ToolError> {
    // 1. LLMから送られてきたJSON引数を、この構造体(引数オブジェクト)にデシリアライズ
    let mut args: Self = serde_json::from_str(arguments_json)?;

    // 2. ステートフルフィールド(#[tool(skip)]されたサンドボックス等)のコピー
    //    親(self)が持っている実行環境のアドレスやDB接続ソケットなどの参照を、
    //    新しく生成された args インスタンスへコピーしてグラウンディングします。
    args.sandbox = self.sandbox.clone();

    // 3. ユーザー定義の run 関数の呼び出し
    args.run().await
}
```
*   **注意**: ツール開発者は、対応する `impl` ブロック内に、`async fn run(&self) -> Result<String, ToolError>` メソッドを自前で実装する必要があります。
