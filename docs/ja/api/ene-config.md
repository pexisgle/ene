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

実際の型 (`crates/ene-config/src/config.rs`) は次の通りです:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EneConfig {
    /// スキーマバージョン番号。
    pub version: u32,
    /// キャラクターカード名またはパス。
    pub character: String,
    /// ユーザーに表示される表示名。
    pub user_name: String,
    /// すべてのシステムプロンプトに注入される行動ルール。
    pub runtime_rules: String,

    #[serde(flatten)]
    #[schemars(skip)]
    /// プロバイダー、ツール、その他のサブ設定用のキャッチオール。
    pub extra: BTreeMap<String, serde_json::Value>,
}
```

### メソッド

| メソッド | シグネチャ | 説明 |
|---------|-----------|------|
| `get_section` | `fn get_section<T>(&self) -> Result<T, EneConfigError> where T: DeserializeOwned + Default + HasConfigKey` | `T::KEY` でサブセクションをデシリアライズ。キーが存在しない場合は `Ok(T::default())`。 |
| `set_section` | `fn set_section<T>(&mut self, section: &T) -> Result<(), EneConfigError> where T: Serialize + HasConfigKey` | `T::KEY` の下にサブセクションをシリアライズして挿入。 |

---

## `define_config!` マクロ

`define_config!` マクロで型付き設定セクションを宣言します。各セクションは独立してシリアライズ・デシリアライズ可能で、一意なレジストリキーを持ちます。次の 3 つのバリアントがあります:

1. `settings, "key", …` — `settings.json` の下にセクションを登録。
2. `character, "key", …` — `character_settings.json` の下にセクションを登録。
3. `$parent, "key", …` — 他のセクションの下にネストし、その `ConfigTarget` を継承。

```rust
ene_config::define_config! {
    settings,
    "llm",
    /// LLMバックエンドの設定。
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

このマクロは以下を行います:
1. `Debug`、`Clone`、`serde::Serialize`、`serde::Deserialize`、
   `schemars::JsonSchema` を derive する。
2. 型に対して `HasConfigKey` を実装する（`const KEY` は指定した文字列に
   設定され、`path()` はルートからのパスを返す）。
3. 各フィールドのインライン `= default` を使った `Default` impl を生成
   する（省略時は `Default::default()`）。
4. JSON スキーマを `__register_schema` に登録する（`ctor` フック経由）。

コンパニオンマクロ `define_tool_config!` はツール設定スキーマ用に
提供されており、代わりに `__register_tool_schema` を使用します。

---

## グローバル設定関数

### `load_config`

```rust
pub fn load_config() -> EneConfig
```

デフォルトパス（バイナリの隣の `assets/` ディレクトリ）から設定を
読み込みます。アプリケーション起動時の主要なエントリーポイント。
内部で `load_full_config()` を呼びます。

### `load_config_from`

```rust
pub fn load_config_from(assets_dir: &Path, config_path: &Path) -> EneConfig
```

明示的なディレクトリとファイルパスから設定を読み込みます。テストや
非標準デプロイに便利。内部で `load_full_config_from` を呼びます。

### `load_full_config` / `load_full_config_from`

```rust
pub fn load_full_config() -> EneConfig
pub fn load_full_config_from(assets_dir: &Path, config_path: &Path) -> EneConfig
```

完全な `EneConfig` 読み込み。`settings.json` を読み込み、`ENE_*`
環境変数を適用し、生成されたスキーマをアセットディレクトリに書き出し、
`update_global_config` 経由でグローバルシングルトンを更新します。

### `save_full_config`

```rust
pub fn save_full_config(config: &EneConfig) -> Result<(), std::io::Error>
```

`config` を JSON にシリアライズして標準設定ファイルに書き込み、
グローバルシングルトンを更新します。

### `update_section`

```rust
pub fn update_section<T>(value: &T) -> Result<(), EneConfigError>
where T: Serialize + DeserializeOwned + HasConfigKey
```

現在の設定を読み込み、`config.set_section(value)` を適用し、
1 回の呼び出しで保存します。

### `register_runtime_schema`

```rust
pub fn register_runtime_schema(key: &str, schema: serde_json::Value)
```

ランタイムで `key` の下に JSON スキーマの断片を登録します。
各ツールバイナリが自分の設定スキーマを報告した後、
`ToolHostManager` が呼び出します。通常は手動で使いません。

### `write_schemas`

```rust
pub fn write_schemas(assets_dir: &Path)
```

収集した JSON スキーマ断片を `assets_dir` の `settings.schema.json`、
`character_settings.schema.json`、および `character_card.schema.json` に
書き出します。CLI スタートアップ時に呼び出されてスキーマを最新状態
に保ちます。

### `generate_schema_json` / `generate_character_schema_json` / `generate_character_card_schema_json`

```rust
pub fn generate_schema_json() -> Result<String, serde_json::Error>
pub fn generate_character_schema_json() -> Result<String, serde_json::Error>
pub fn generate_character_card_schema_json() -> Result<String, serde_json::Error>
```

各設定ファイルの JSON スキーマを整形 JSON 文字列として返します。

### `resolve_character_path`

```rust
pub fn resolve_character_path(name: &str) -> String
```

キャラクター名をファイルシステムパスに解決します。素の名前は
`assets/characters/<name>/character.json` にマップされ、`/` や `\` を
含むパスはそのまま返されます。

---

## グローバル設定シングルトン

```rust
pub static GLOBAL_CONFIG: std::sync::OnceLock<std::sync::RwLock<EneConfig>>;
```

初回アクセス時に初期化されるプロセスグローバルな `RwLock<EneConfig>`。
アクターはスタートアップ時にこれを読み取ります。ランタイムでの
更新には `EneHandle::reconfigure` を使用してください。便宜的な
アクセサも提供されています:

| 関数 | 説明 |
|------|------|
| `get_global_config() -> EneConfig` | 現在のグローバル設定のクローン（未設定なら `EneConfig::default()`）。 |
| `update_global_config(config: EneConfig)` | グローバル設定をインプレースで置き換える。 |
| `get_global_section<T>() -> T` | グローバル設定からセクションをデシリアライズ（無い場合は `T::default()`）。 |

`ConfigStore` 型（`store.rs` を参照）はダーティ追跡と自動保存を追加
する高レベルラッパーです。

---

## 再エクスポートとヘルパー

`ene_config::lib.rs` はワークスペースクレートから以下の項目を再エクスポート
し、ダウンストリームコードが `ene_config::*` 名前空間一つで使えるように
します:

| 項目 | ソース |
|------|--------|
| `CharacterCardV3`、`CharacterCardData`、`CharacterAsset`、`EneExtension`、`ExpressionDefinition`、`Lorebook`、`LorebookEntry`、`ResolvedExpression`、`expand_cbs_macros`、`resolve_expressions` | `character_card` |
| `CharacterConfig`、`MotionEntry` | `character_config` |
| `EneConfig`、`HasConfigKey`、`ConfigTarget`、`__register_schema`、`__register_tool_schema`、`get_global_config`、`update_global_config`、`get_global_section`、`load_config`、`load_config_from`、`load_full_config`、`load_full_config_from`、`save_full_config`、`update_section`、`register_runtime_schema`、`resolve_character_path`、`generate_schema_json`、`generate_character_schema_json`、`generate_character_card_schema_json`、`write_schemas` | `config` |
| `ConfigError`、`EneConfigError` | `error` |
| `IS_DEV_BUILD`、`app_data_dir`、`assets_dir`、`builtin_tools_dir`、`config_file_path`、`models_dir`、`schema_file_path`、`character_schema_file_path`、`character_card_schema_file_path`、`character_settings_path`、`tool_socket_dir`、`user_tools_dir` | `paths` |
| `ensure_resource_dirs` | `resources` |
| `ConfigStore` | `store` |
| `define_config!`、`define_tool_config!`、`define_label_enum!` | トップレベルマクロ |
| `serde`、`schemars`、`ctor` | 再エクスポート |

`EneConfigError` が正準のエラー enum で、`ConfigError` は型エイリアス
(`pub type ConfigError = EneConfigError;`) です。

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

### `EneExtension` / `Lorebook` / `LorebookEntry`

`EneExtension` 構造体はキャラクターカード拡張データのオープンな入れ物
（例: `talkativeness`、`favoured_decision_style`、Lorebook）です。
含まれる `Lorebook` および `LorebookEntry` 型はデスクトップ UI と
メモリシステムで使用される型付きの形状です。

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

## キャラクター設定

`ene_config::CharacterConfig` は `EneConfig` に対するキャラクター固有
のコンパニオンです。キャラクターカードの隣の `character_settings.json`
から読み込まれ、`position`、モーションプリセット (`MotionEntry`)、
表情バインディングなどの UI/ランタイム設定を保持します。

```rust
pub struct CharacterConfig { /* … */ }
pub struct MotionEntry { /* … */ }
```

---

## 使用例

```rust
use ene_config::{load_config, EneConfig, EneConfigError};

// デフォルトパスから読み込む
let config = load_config();

// セクションを読み取る (キーが無いと T::default() を返す)
let llm: LlmConfig = config.get_section().unwrap_or_default();
println!("使用モデル: {}", llm.model);

// 変更して書き戻す
let mut config = config;
let mut llm = config.get_section::<LlmConfig>().unwrap_or_default();
llm.model = "gpt-4o-mini".to_string();
config.set_section(&llm).map_err(EneConfigError::from)?;
ene_config::save_full_config(&config)?;
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
