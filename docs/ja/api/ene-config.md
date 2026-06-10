# `ene-config` — APIリファレンス

> **クレート:** `ene-config`  
> **役割:** Eneシステムの設定読み込み、スキーマ生成、キャラクターカード型の管理。

---

## 概要

`ene-config` は [`figment`](https://docs.rs/figment) のレイヤー型設定システムを使用してすべてのランタイム設定を管理します。`EneConfig` 型、型付きセクションを宣言するための `define_config!` マクロ、グローバル設定状態、そしてキャラクターカードのフォーマットを定義します。

### 読み込み優先順位

設定は以下の優先度順（後のレイヤーが優先）で解決されます：

```
1. コンパイル時デフォルト値  （define_config! ブロックにハードコード）
         ↓
2. assets/settings.json     （ユーザーの設定ファイル）
         ↓
3. ENE_* 環境変数           （例：ENE_LLM__API_KEY）
```

> **注意:** `assets/settings.schema.json` と `character_settings.schema.json` は自動生成されgitignoreされています。コミットや手動編集は行わないでください。`cargo run -p ene-cli` の実行ごとに再生成されます。

---

## `EneConfig`

トップレベルの設定コンテナです。内部的にはセクションキーから型付きセクションデータへのマップを保持します。

```rust
pub struct EneConfig { /* 非公開 */ }
```

### メソッド

| メソッド | シグネチャ | 説明 |
|---------|-----------|------|
| `get_section` | `fn get_section<T: ConfigSection>(&self) -> Option<T>` | 型 `T` の設定セクションを返します。存在しない場合は `None`。 |
| `set_section` | `fn set_section<T: ConfigSection>(&mut self, section: &T)` | 型 `T` の設定セクションを置き換えまたは挿入します。 |

---

## `define_config!` マクロ

`define_config!` マクロで型付き設定セクションを宣言します。各セクションは独立してシリアライズ・デシリアライズ可能で、一意なレジストリキーを持ちます。

```rust
define_config! {
    /// LLMバックエンドの設定。
    #[section = "llm"]
    pub struct LlmConfig {
        /// OpenAI互換APIのベースURL。
        pub api_base: String = "https://api.openai.com/v1".to_string(),

        /// APIキー（ENE_LLM__API_KEY 環境変数でも設定可能）。
        pub api_key: String = String::new(),

        /// チャット補完に使用するモデル名。
        pub model: String = "gpt-4o".to_string(),

        /// トークン単位の最大コンテキストウィンドウ。
        pub max_tokens: usize = 4096,
    }
}
```

このマクロは以下を行います：
1. `serde::Serialize`、`serde::Deserialize`、`Clone`、`Debug` を derive します。
2. 指定された `#[section]` キーで `ConfigSection` を実装します。
3. JSONスキーマを生成し、`register_runtime_schema` に登録します。

---

## グローバル設定関数

### `load_config`

```rust
pub fn load_config() -> EneConfig
```

デフォルトパス（バイナリの隣の `assets/` ディレクトリ）から設定を読み込みます。アプリケーション起動時の主要なエントリーポイントです。

### `load_config_from`

```rust
pub fn load_config_from(assets_dir: &Path, config_path: &Path) -> EneConfig
```

明示的なディレクトリとファイルパスから設定を読み込みます。テストや非標準デプロイに便利です。

### `register_runtime_schema`

```rust
pub fn register_runtime_schema(key: &str, schema: serde_json::Value)
```

`key` の下にJSONスキーマの断片を登録します。各セクションの `define_config!` によって自動的に呼び出されます。通常、アプリケーションから直接呼び出す必要はありません。

### `write_schemas`

```rust
pub fn write_schemas(assets_dir: &Path)
```

収集したJSONスキーマ断片を `assets_dir` の `settings.schema.json` と `character_settings.schema.json` に書き出します。CLIスタートアップ時に呼び出されてスキーマを最新状態に保ちます。

### `resolve_character_path`

```rust
pub fn resolve_character_path(name: &str) -> String
```

キャラクター名をファイルシステムパスに解決します。設定済みのキャラクターディレクトリを検索します。

---

## グローバル設定シングルトン

```rust
pub static GLOBAL_CONFIG: OnceLock<RwLock<EneConfig>>;
```

初回アクセス時に初期化されるプロセスグローバルな `RwLock<EneConfig>`。アクターはスタートアップ時にこれを読み取ります。ランタイムでの更新には `EneHandle::reconfigure` を使用してください。

---

## キャラクターカード型

EneはキャラクターデータにCharacterCard V3フォーマットを使用します。

### `CharacterCardV3`

```rust
pub struct CharacterCardV3 {
    /// キャラクターのコアデータ（名前、説明、性格、シナリオなど）。
    pub data: CharacterCardData,

    /// カードが参照する埋め込みバイナリアセット（画像、音声など）。
    pub assets: Vec<CharacterAsset>,
}
```

### `CharacterCardData`

キャラクターカードのすべてのテキストフィールドを保持します：`name`、`description`、`personality`、`scenario`、`first_mes`、`alternate_greetings`、`system_prompt`、および拡張データ。

### `CharacterAsset`

```rust
pub struct CharacterAsset {
    /// アセットのMIMEタイプ（例：`"image/png"`）。
    pub asset_type: String,

    /// カードテキスト内でアセットを参照する論理名。
    pub name: String,

    /// URIまたは埋め込みBase64データ。
    pub uri: String,
}
```

### `ExpressionDefinition`

キャラクターに関連付けられた名前付き表情または状態を定義します。デスクトップレンダラーで使用されます。

```rust
pub struct ExpressionDefinition {
    pub name: String,
    pub asset_name: String,
    pub trigger_tokens: Vec<String>,
}
```

### `ResolvedExpression`

ロード済みアセットに対して `ExpressionDefinition` を解決した結果。表示に使える実際の画像データを含みます。

```rust
pub struct ResolvedExpression {
    pub name: String,
    pub image_data: Vec<u8>,
    pub trigger_tokens: Vec<String>,
}
```

---

## キャラクターカード関数

### `expand_cbs_macros`

```rust
pub fn expand_cbs_macros(card: &CharacterCardV3) -> String
```

カードのシステムプロンプト内のCBS（Character Book Substitution）マクロ構文を展開します。LLMコンテキストに注入する準備が整った展開済み文字列を返します。

### `resolve_expressions`

```rust
pub fn resolve_expressions(card: &CharacterCardV3) -> Vec<ResolvedExpression>
```

カードの `assets` と `ExpressionDefinition` を反復処理し、画像データを読み込んで、デスクトップUIのための `Vec<ResolvedExpression>` を生成します。

---

## 使用例

```rust
use ene_config::{load_config, EneConfig};

// デフォルトパスから読み込む
let config = load_config();

// セクションを読み取る
if let Some(llm) = config.get_section::<LlmConfig>() {
    println!("使用モデル: {}", llm.model);
    println!("APIベース: {}", llm.api_base);
}

// 変更して書き戻す
let mut config = config;
let mut llm = config.get_section::<LlmConfig>().unwrap_or_default();
llm.model = "gpt-4o-mini".to_string();
config.set_section(&llm);
```

---

## 新しい設定フィールドの追加

`AGENTS.md` の**レシピR2**に従ってください：

1. `crates/ene-config/src/config.rs` の該当する構造体を `define_config!` で編集する。
2. `cargo run -p ene-cli` を1回実行して `assets/settings.schema.json` を再生成する。
3. `docs/configuration/settings.md`（英語および日本語）に新しいフィールドをドキュメント化する。

---

## 関連項目

- [`ene-core`](./ene-core.md) — ランタイムで `EneConfig` を消費する
- [設定ガイド](../configuration/settings.md) — エンドユーザー向け設定リファレンス
