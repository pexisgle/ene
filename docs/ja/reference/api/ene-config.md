# `ene-config` — APIリファレンス

> **クレート:** `ene-config`  
> **役割:** Eneシステムの設定読み込み、スキーマ生成、キャラクターカード型の管理。

---

## 概要

`ene-config` は [`figment`](https://docs.rs/figment) のレイヤー型設定システムを使用してすべてのランタイム設定を管理します。`EneConfig` 型、型付きセクションを宣言するための `define_config!` マクロ、グローバル設定状態、そしてキャラクターカードのフォーマットを定義します。旧 `ene-common` から吸収した `Truncate` / `TruncateResult`（`ene_config::truncate`）も所有します。ツールは通常 [`ene-tool-common`](./ene-tool-common.md) の再エクスポートを使います。

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
pub fn load_config() -> Result<EneConfig, EneConfigError>
```

デフォルトパス（バイナリの隣の `assets/` ディレクトリ）から設定を
読み込みます。アプリケーション起動時の主要なエントリーポイント。
内部で `load_full_config()` を呼びます。`settings.json` が不正な
形式である場合や `ENE_*` 環境変数を期待する型にパースできない場合は
`Err(EneConfigError::GenericConfigError(..))` を返します — 不正な
設定ファイルに対して**サイレントにデフォルトへフォールバックすること
はありません**。（ホスト起動時の使い勝手のために意図的にフォールバック
を残している呼び出し元については [`ConfigStore::load`](#configstore)
を参照してください。）

### `load_config_from`

```rust
pub fn load_config_from(assets_dir: &Path, config_path: &Path) -> Result<EneConfig, EneConfigError>
```

明示的なディレクトリとファイルパスから設定を読み込みます。テストや
非標準デプロイに便利。内部で `load_full_config_from` を呼びます。

### `load_full_config` / `load_full_config_from`

```rust
pub fn load_full_config() -> Result<EneConfig, EneConfigError>
pub fn load_full_config_from(assets_dir: &Path, config_path: &Path) -> Result<EneConfig, EneConfigError>
```

完全な `EneConfig` 読み込み。優先度順に次のレイヤーを重ねます：
`EneConfig::default()` → `settings.json` → `ENE_*` 環境変数
（`__` 区切り、`ENE_PROVIDER__API_KEY` が `provider.api_key` パスに
マップされるよう小文字にケースフォールディングされる）。成功時は
生成されたスキーマをアセットディレクトリに書き出し、`Ok` を返す前に
`update_global_config` 経由でグローバルシングルトンを更新します。
失敗時（不正な形式の JSON、型が一致しない環境変数など）は
グローバルシングルトンやディスク上のスキーマファイルに**一切触れず**
`Err(EneConfigError::GenericConfigError(..))` を返します。

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

### `ensure_resource_dirs`

```rust
pub fn ensure_resource_dirs() -> Result<PathBuf, EneConfigError>
```

アプリケーションのデータディレクトリを初期化します。初回起動時に、配布ディレクトリからOS標準のアプリケーションデータディレクトリ（Windowsの場合は `%APPDATA%` など）にデフォルトのアセットをコピーします。

**配布時のアセット同梱ポリシー:**
リリース用の配布パッケージを軽量に保つため、ローカル開発用や一時的なファイルを配布用の `assets/` ディレクトリに含めないでください。`.gitignore` に登録されている以下のリソースは、リリースアーカイブに**同梱してはいけません**：
* `assets/models/` （ローカルのモデルキャッシュ）
* `assets/schema/` 配下に自動生成されるスキーマファイル（`settings.schema.json` など）
* データベースファイル（`*.db*`）やドットファイル

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
する高レベルラッパーです。詳細は下記の [`ConfigStore`](#configstore) を
参照してください。

---

## `ConfigStore`

`ConfigStore` は `ene-desktop` と `ene-cli` がランタイムで使用する単一の
永続化レイヤーです。グローバルな [`EneConfig`](#eneconfig) とアクティブな
[`CharacterConfig`](#キャラクター設定) の両方をラップし、それぞれに
未保存の変更があるかどうかをアトミックなダーティフラグで追跡します。
これにより、定期的なフラッシュシステム（例えば1フレームごとに実行される
Bevy システム）が変更されていないファイルを再シリアライズ・再書き込み
することなく、安価に [`flush_if_dirty`](#configstoreのメソッド) を毎ティック
呼び出すことができます。

```rust
pub struct ConfigStore { /* 非公開: RwLock<EneConfig> + RwLock<CharacterConfig> + 2つの AtomicBool */ }
```

### コンストラクタ

| コンストラクタ | シグネチャ | 説明 |
|---|---|---|
| `load` | `fn load() -> Self` | 標準の figment パイプラインを介してグローバル設定をディスクから読み込む。読み込みに失敗した場合はエラーをログに記録し、`EneConfig::default()` にフォールバックする — これはクレート内で唯一、以前のサイレントデフォルトの挙動を維持している呼び出し箇所であり、ユーザーが壊れた `settings.json` を修正する前でもホストプロセスがストアを構築できる必要があるためである。キャラクター設定は `CharacterConfig::default()` として開始する。後で `load_character_config` を呼び出して populate すること。 |
| `try_load` | `fn try_load() -> Result<Self, EneConfigError>` | `load` と同様だが、サイレントにフォールバックせず読み込みエラーを伝播する。ユーザーが設定の問題を即座に確認できるよう、CLI 起動時にはこちらを優先すべき。 |
| `from_config` | `fn from_config(config: EneConfig) -> Self` | 既に読み込まれた `EneConfig`（例：テストで構築したもの、または他の手段で読み込んだもの）からストアを構築する。 |

### メソッド {#configstoreのメソッド}

| メソッド | シグネチャ | 説明 |
|---|---|---|
| `config` | `fn config(&self) -> EneConfig` | 現在のグローバル設定のクローンを返す。 |
| `with_config_mut` | `fn with_config_mut(&self, f: impl FnOnce(&mut EneConfig))` | 書き込みロックの下でグローバル設定に対して `f` を実行し、その後ストアをダーティにマークする。増分編集には `set_config` よりも優先して使用すべき。 |
| `set_config` | `fn set_config(&self, config: EneConfig)` | グローバル設定全体を置き換え、ストアをダーティにマークする。 |
| `get_section<T>` | `fn get_section<T>(&self) -> T where T: DeserializeOwned + Default + HasConfigKey` | グローバル設定から型付きセクションを読み取る。エラー（キーが無い、またはデシリアライズ失敗）が発生した場合は `T::default()` を返す。これは `get_global_section` と一致する挙動。 |
| `set_section<T>` | `fn set_section<T>(&self, section: &T) where T: Serialize + HasConfigKey` | グローバル設定に型付きセクションを書き込み、ストアをダーティにマークする。基盤となる `EneConfig::set_section` のエラーは握りつぶされる（`.ok()`）— このメソッドは呼び出し元の視点からは失敗しない。 |
| `character_config` | `fn character_config(&self) -> CharacterConfig` | 現在のキャラクター固有設定のクローンを返す。 |
| `with_character_config_mut` | `fn with_character_config_mut(&self, f: impl FnOnce(&mut CharacterConfig))` | 書き込みロックの下でキャラクター固有設定に対して `f` を実行し、その後ダーティにマークする。 |
| `load_character_config` | `fn load_character_config(&self, character_name: &str)` | `character_name` の `character_settings.json`（`character_settings_path` 経由）を読み取り、メモリ上の `CharacterConfig` を置き換える。ファイルが存在しない、またはパースに失敗した場合は `CharacterConfig::default()` にフォールバックする。ストアをダーティにマーク**しない**（読み込みであり変更ではないため）。 |
| `set_character_config` | `fn set_character_config(&self, config: CharacterConfig)` | キャラクター固有設定を置き換え、ダーティにマークする。 |
| `get_character_section<T>` | `fn get_character_section<T>(&self) -> T where T: DeserializeOwned + Default + HasConfigKey` | キャラクター固有設定の `extra` マップから型付きセクションを読み取る。 |
| `set_character_section<T>` | `fn set_character_section<T>(&self, section: &T) where T: Serialize + HasConfigKey` | キャラクター固有設定の `extra` マップに型付きセクションを書き込み、ダーティにマークする。 |
| `flush_if_dirty` | `fn flush_if_dirty(&self, character_name: Option<&str>) -> std::io::Result<bool>` | グローバル設定のダーティフラグが立っていればディスクに保存する（フラグをクリアする）。キャラクター固有設定についても、そのダーティフラグが立っていて*かつ* `character_name` が `Some` の場合に保存する。何かが書き込まれた場合は `Ok(true)`、何もダーティでなければ `Ok(false)` を返す。フレームごとの自動保存システムが呼び出すべきメソッド。 |
| `flush` | `fn flush(&self, character_name: Option<&str>) -> std::io::Result<()>` | 両方のダーティフラグを強制的に `true` にし、`flush_if_dirty` を呼び出して両方の設定を無条件にディスクへ書き込む。シャットダウン時や明示的な「保存」アクションで使用する。 |
| `is_dirty` | `fn is_dirty(&self) -> bool` | グローバル設定またはキャラクター固有設定のいずれかに未保存の変更がある場合 `true`。 |

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

### `EneConfigError`

```rust
pub enum EneConfigError {
    MissingBaseUrl { env_var: String },
    MissingApiKey { env_var: String },
    NoCharacterCard,
    CardReadError(#[from] std::io::Error),
    JsonError(#[from] serde_json::Error),
    GenericConfigError(String),
    IoError(#[source] std::io::Error),
}
```

| バリアント | 意味 |
|---|---|
| `MissingBaseUrl { env_var }` | AI プロバイダーのベース URL が空で、フォールバックとなる環境変数も設定されていない。`env_var` はチェックされた変数名（例：`"OPENAI_BASE_URL"`）。 |
| `MissingApiKey { env_var }` | API キーが空で、フォールバックとなる環境変数も設定されていない。 |
| `NoCharacterCard` | キャラクターカードが読み込まれる前に要求された。 |
| `CardReadError(std::io::Error)` | ディスクからキャラクターカードファイルを読み取る際の I/O 失敗。`#[from] std::io::Error` を実装しているため、カード読み取りに対する `?` が自動的に変換される。 |
| `JsonError(serde_json::Error)` | キャラクターカードまたは設定ファイルの JSON パースに失敗した。`#[from] serde_json::Error` を実装している。 |
| `GenericConfigError(String)` | 自由形式のメッセージを持つ設定エラーのキャッチオール — `load_config`/`load_full_config_from` が不正な `settings.json` や不正な形式の `ENE_*` 環境変数で返すのはこれであり、`set_section`/`get_section` がシリアライズ／デシリアライズ失敗や間違った設定ターゲットを介したセクションの読み書き（[`EneConfig::get_section`](#メソッド) を参照）で返すのもこれ。 |
| `IoError(std::io::Error)` | キャラクターカードの読み取り*ではない*一般的な I/O エラー（`CardReadError` とは異なり、このバリアントは `#[from]` を実装していないため、呼び出し元は明示的にラップする必要がある）。 |

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

## キャラクター設定 {#キャラクター設定}

`ene_config::CharacterConfig` は `EneConfig` に対するキャラクター固有
のコンパニオンです。キャラクターカードの隣の `character_settings.json`
から読み込まれ、デスクトップ 3D レンダラー用の UI/ランタイム設定 —
モデルのトランスフォーム、注視動作、ロード時に再生するデフォルトの
モーション/表情 — を保持します。`EneConfig` と同様に、
`define_config!(character, "key", …)` で登録された型付きセクションのための
`#[serde(flatten)]` キャッチオール `extra` マップを持ちます。

```rust
pub struct CharacterConfig {
    pub character_position: [f32; 3],  // デフォルト: [0.0, 0.0, 0.0]
    pub model_scale: f32,               // デフォルト: 1.0
    pub look_at_strength: f32,           // デフォルト: 0.6
    pub default_motion: String,          // デフォルト: ""
    pub default_expression: String,      // デフォルト: "neutral"
    pub extra: BTreeMap<String, serde_json::Value>,
}
```

| フィールド | 説明 |
|---|---|
| `character_position` | シーン内のキャラクターモデルの3D位置 `[x, y, z]`。 |
| `model_scale` | キャラクターモデルに適用される均一スケール係数。 |
| `look_at_strength` | キャラクターの視線がユーザーをどれだけ強く追従するか。`0.0`（決してユーザーを見ない）から `1.0`（常にユーザーを見る）まで。 |
| `default_motion` | デフォルトで再生するモーションの名前。キャラクターのモーションリストの `MotionEntry.name` に一致するべき。 |
| `default_expression` | デフォルトで適用する表情の名前（例：`"neutral"`）。 |
| `extra` | `character` ターゲットの `define_config!` セクション用キャッチオールマップ。 |

### メソッド

| メソッド | シグネチャ | 説明 |
|---|---|---|
| `get_section<T>` | `fn get_section<T>(&self) -> Result<T, EneConfigError> where T: DeserializeOwned + Default + HasConfigKey` | `T::path()` により `extra` から `character` ターゲットのサブセクションをデシリアライズする。パスが存在しない場合は `Ok(T::default())`。デバッグビルドでは `T::TARGET == ConfigTarget::Character` をアサートする。 |
| `set_section<T>` | `fn set_section<T>(&mut self, section: &T) -> Result<(), EneConfigError> where T: Serialize + HasConfigKey` | `T::path()` により `character` ターゲットのサブセクションをシリアライズして `extra` に挿入する。 |

### `MotionEntry`

単一の名前付きモーションファイル参照で、キャラクター固有のモーション
リスト（通常はカスタムの `character` ターゲット設定セクション内の
`Vec<MotionEntry>` として保存される）で使用されます。

```rust
pub struct MotionEntry {
    pub name: String,  // 例: "VRMA_01"
    pub path: String,  // 例: "motions/VRMA_01.vrma"
}
```

---

## プロンプトテンプレート: `PromptLibrary`

`PromptLibrary` は、`ene-runtime` 全体で使用される LLM 向けのプロンプト
文字列（システムプロンプトのフレーミング、感情ルール、
メモリ/サマライザー/エクストラクター/感情分類器のテンプレート）を
読み込みます。ユーザー向けの文章をコンパイル済みコードから分離し、
各文字列に安定したローカライズ可能な場所を与えます。

```rust
pub struct PromptLibrary { /* 非公開: PromptLibraryData + lang */ }
```

### コンストラクタ

| コンストラクタ | シグネチャ | 説明 |
|---|---|---|
| `load` | `fn load(lang: &str) -> Self` | 言語コードに対応する組み込みのプロンプトセットを読み込む。`"ja"`/`"jp"` は日本語のデフォルトを読み込み、それ以外（未知のコードを含む）は英語にフォールバックする。 |
| `built_in_english` | `fn built_in_english() -> Self` | コンパイル時に埋め込まれた英語のプロンプトを返す（`crates/ene-config/prompts/en.json` と付随する `.md` フラグメントを `include_str!` 経由で）。ここでのパース失敗はビルド時のバグであり、実行時の条件ではない。 |
| `built_in_japanese` | `fn built_in_japanese() -> Self` | 上記と同様、日本語版（`prompts/ja.json`）。 |

### アクセサ

| メソッド | 戻り値 | 説明 |
|---|---|---|
| `lang()` | `&str` | このライブラリが読み込まれた言語コード（`"en"` または `"ja"`）。 |
| `system()` | `&SystemPrompts` | システムプロンプトのフレーミング：`mascot_context`、セクションヘッダー（`behavior_rules_header`、`character_header`、`personality_header`、`background_header`、`scene_header`、`examples_header`）。`render_mascot_context(char_name, user_name)` を持つ。 |
| `emotion()` | `&EmotionPrompts` | 感情タグ出力ルールのテキスト、トークン/例のヘッダー、感情別の例、`natural_dialogue_contract`。 |
| `memory()` | `&MemoryPrompts` | エピソードメモリの想起テンプレート。`render_summary_item(age, text)` と `render_facts_header(user_name)` を持つ。 |
| `summarizer()` | `&SummarizerPrompts` | LLM サマライザーのシステム/ユーザープロンプトテンプレート。`render_system(user_name, char_name, existing_facts, conversation)` と `render_user_prompt(user_name, existing_facts, conversation)` を持つ。 |
| `split()` | `&SplitPrompts` | セッション分割理由のメッセージテンプレート（`reason_timeout`、`reason_topic`、`reason_context`、`reason_composite`、`reason_manual`）。`render_reason_timeout(minutes)`、`render_reason_topic(similarity)`、`render_reason_composite(score)` を持つ。 |
| `extractor()` | `&ExtractorPrompts` | LLM メモリエクストラクターのシステム/ユーザープロンプトテンプレート。`render_user_prompt(conversation, pattern_hints)` を持つ。 |
| `affect_classifier()` | `&AffectClassifierPrompts` | LLM 感情分類器のシステム/ユーザープロンプトテンプレート。`render_user_prompt(current_affect, conversation)` を持つ。 |

### `substitute`

```rust
pub fn substitute(template: &str, vars: &[(&str, &str)]) -> String
```

`template` 内のすべての `{name}` プレースホルダーを `vars` の対応する
値で置き換えます。未知のプレースホルダーはそのまま残されます
（パニックもエラーも発生しません）。これは上記のすべての `render_*`
ヘルパーが基盤としているプリミティブです。他の `substitute` という
名前との衝突を避けるため、クレートルートでは `substitute_prompt_vars`
として再エクスポートされています。

```rust,no_run
use ene_config::PromptLibrary;

let lib = PromptLibrary::load("en");
let framing = lib.system().render_mascot_context("Alicia", "Sam");
let facts_header = lib.memory().render_facts_header("Sam");
```

---

## 使用例

### `EneConfig` を直接読み込んで変更する

```rust,no_run
use ene_config::{load_config, save_full_config, EneConfigError};

fn update_model(new_model: &str) -> Result<(), Box<dyn std::error::Error>> {
    // デフォルトパスで読み込む。不正な settings.json では EneConfigError を伝播する。
    let mut config = load_config()?;

    // セクションを読み取る (キーが無いと T::default() を返す)
    let mut llm: LlmConfig = config.get_section().unwrap_or_default();
    println!("使用モデル: {}", llm.model);

    // 変更して書き戻す
    llm.model = new_model.to_string();
    config.set_section(&llm).map_err(EneConfigError::from)?;
    save_full_config(&config)?;
    Ok(())
}
```

### `ConfigStore` を使う（長時間実行するプロセスに推奨）

```rust,no_run
use ene_config::ConfigStore;

fn update_model_via_store(store: &ConfigStore, new_model: &str) {
    store.with_config_mut(|_config| {
        // 実際には EneConfig の生フィールドではなく、型付きセクションを変更する。
    });

    let mut llm: LlmConfig = store.get_section();
    llm.model = new_model.to_string();
    store.set_section(&llm);

    // 実際のホストでは1フレーム/1ティックごとに呼ばれる。ここでは例のために強制する。
    let _ = store.flush(None);
}
```

---

## 新しい設定フィールドの追加

`AGENTS.md` の**レシピR2**に従ってください：

1. `crates/ene-config/src/config.rs` の該当する構造体を `define_config!` で編集する。
2. `cargo run -p ene-cli` を1回実行して `assets/settings.schema.json` を再生成する。
3. `docs/reference/configuration/settings.md` および `docs/ja/reference/configuration/settings.md` に新しいフィールドをドキュメント化する。

---

## 関連項目

- [`ene-runtime`](./ene-runtime.md) — ランタイムで `EneConfig` を消費する
- [設定ガイド](../configuration/settings.md) — エンドユーザー向け設定リファレンス
