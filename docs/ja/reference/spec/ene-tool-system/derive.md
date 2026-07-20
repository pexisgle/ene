# `ToolSpec` および `ToolAction` 導出マクロ仕様 (`ene-tool-derive`)

`ene-tool-derive` クレートには、Rust の構造体を LLM が認識するツールインターフェース記述（`ToolSpec`）および実行時パラメータシリアライザ（`ToolAction`）にビルドする手続き型マクロが含まれます。

---

## 1. 属性定義マクロエントリーポイント (`lib.rs`)

#### `derive_tool_spec`
*   **シグネチャ**: `pub fn derive_tool_spec(input: TokenStream) -> TokenStream`
*   **説明**: `#[derive(ToolSpec)]` マクロの展開を行います。構造体の構文ツリー、コンテナ属性、およびフィールド注釈コメントなどをパースし、ツールの機能パラメータ記述メタデータを自動生成します。

#### `derive_tool_action`
*   **シグネチャ**: `pub fn derive_tool_action(input: TokenStream) -> TokenStream`
*   **説明**: `#[derive(ToolAction)]` マクロを展開し、LLM からの JSON 引数をパースして型キャストし、実体実行を呼び出し元に移譲するコードを作成します。

#### `tool_action`
*   **シグネチャ**: `pub fn tool_action(attr: TokenStream, input: TokenStream) -> TokenStream`
*   **説明**: ツール関数を簡潔に宣言するためのヘルプ属性マクロです。

---

## 2. マクロ展開およびコード生成ロジック (`lib.rs`)

#### `expand_tool_spec`
*   **シグネチャ**: `fn expand_tool_spec(ast: &DeriveInput) -> syn::Result<TokenStream2>`
*   **プロセス**:
    1.  構造体に付与されているコンテナ属性 `#[tool(...)]` を読み込みます。
    2.  `collect_field_instructions` を呼び出し、構造体のフィールドを解析してパラメータ仕様を組み立てます。
    3.  `schemars` パラメータ生成ロジックを実行し、引数 JSON Schema 内のルート定義に `additionalProperties: false` 属性を設定します（LLM による引数の捏造を防止します）。
    4.  マッピング名と表示名を表す `TOOL_NAME` および `DISPLAY_NAME` の文字列定数を生成します。
    5.  メタデータ取得トレイトの実装コードを出力します。

#### `expand_tool_action_derive`
*   **シグネチャ**: `fn expand_tool_action_derive(ast: &DeriveInput) -> syn::Result<TokenStream2>`
*   **説明**: `#[derive(ToolAction)]` が宣言された構造体向けに、JSON 引数のロードと型バインドを行うコードを展開します。

#### `expand_tool_action`
*   **シグネチャ**: `fn expand_tool_action(item: &mut syn::ItemImpl, args_ty: &syn::Type)`
*   **説明**: アクション実行インターフェースの実装コードを展開します。

#### `collect_field_instructions`
*   **シグネチャ**: `fn collect_field_instructions(fields: &syn::punctuated::Punctuated<syn::Field, syn::token::Comma>) -> syn::Result<Vec<TokenStream2>>`
*   **説明**: 各フィールドを順次検査し、`#[arg(...)]` パラメータ記述や Rust 側のドキュメント用コメントを取得してメタデータを作成します。`#[tool(skip)]` 指定がある場合は、このスキーマ出力処理から該当項目を完全に除外（スキップ）します。

#### `apply_serde_attrs`
*   **シグネチャ**: `fn apply_serde_attrs(f: &syn::Field, instr: &mut FieldInstr)`
*   **説明**: `#[serde(rename = "...")]` などの Serde 指定が存在する場合、生成するスキーマ内のプロパティ名も自動で連動するように変換をかけます。

#### `emit_field`
*   **Signature**: `fn emit_field(instr: &FieldInstr) -> TokenStream2`
*   **Description**: フィールド単体の JSON スキーマ構築用マクロ表現トークンを書き出します。

---

## 3. アトリビュートパーサーとユーティリティ (`attr.rs`)

`attr` モジュールは `darling` を介してアトリビュート定義をデコードします。

#### `is_hidden`
*   **Signature**: `pub const fn is_hidden(&self) -> bool`
*   **Description**: 該当ツールが LLM 向け検索インデックスから隠蔽（Hidden）されているかを返します。

#### `has_tool_skip`
*   **Signature**: `pub fn has_tool_skip(field: &syn::Field) -> bool`
*   **Description**: フィールドに `#[tool(skip)]` アトリビュートが付与されているかを返します。

#### `full_name`
*   **シグネチャ**: `pub fn full_name(&self) -> String`
*   **説明**: ツールの名前空間と具体的なアクション名（例: `fs.read`）を結合して完全なツール ID を返します。

#### `display_name_value`
*   **Signature**: `pub fn display_name_value(&self, _default: String) -> String`
*   **Description**: 指定された表示名アトリビュート値を取得します。

#### `summary_value`
*   **Signature**: `pub fn summary_value(&self) -> darling::Result<String>`
*   **Description**: ツール機能概要文を取得します。

#### `description_value`
*   **Signature**: `pub fn description_value(&self) -> String`
*   **Description**: 詳細な挙動説明テキストを取得します。

#### `category_path` / `side_effects_path`
*   **Signature**: `pub fn category_path(&self) -> TokenStream2`
*   **Description**: カテゴリ分類および副作用（`side_effects`）の有無フラグのコードトークンを返します。

#### `keywords_list` / `string_list` / `related_list`
*   **Signature**: `pub fn keywords_list(&self, kind: &str) -> Vec<String>`
*   **Description**: アトリビュート内の文字列配列定義を読み取り正規化します。

#### `version_tokens`
*   **Signature**: `pub fn version_tokens(&self) -> TokenStream2`
*   **Description**: バージョン情報を解決するトークンを生成します。

#### `examples_value`
*   **Signature**: `pub fn examples_value(&self) -> TokenStream2`
*   **Description**: プロンプト用のツール呼び出しの例（Examples）をパースし、コード表現トークンを返します。

#### `args_const_ident`
*   **Signature**: `pub fn args_const_ident(&self, struct_ident: &syn::Ident) -> syn::Ident`
*   **Description**: スキーマ検証用の一時的な定数識別子名を返します。

#### `parse_version`
*   **シグネチャ**: `fn parse_version(s: &str) -> Option<(u32, u32, u32)>`
*   **説明**: バージョン文字列（`1.2.3` など）から、主要・マイナー・パッチバージョン数値を抽出します。

#### `title_case`
*   **Signature**: `fn title_case(s: &str) -> String`
*   **Description**: 単語の先頭を大文字化（Title Case）します。

#### `path_token`
*   **Signature**: `fn path_token(name: &str, default_module: &str) -> TokenStream2`
*   **Description**: 指定された名前空間に対応するモジュールアクセスパスのトークンを返します。
