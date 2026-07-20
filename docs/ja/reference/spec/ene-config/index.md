# `ene-config` 構成設定およびキャラクターカード仕様

`ene-config` クレートは、JSON 形式による構成設定ファイルの入出力、SillyTavern 互換キャラクターカード（V3）の読み込みとパース、スキーマバリデーターのグローバル登録、およびマクロによる定義手段を提供します。

---

## 1. 構成設定管理メソッド (`store.rs`)

### `ConfigStore` (パブリック / 構造体)
システム設定キャッシュの保持および永続化ファイルの管理を行う主要なオブジェクトです。

#### `load`
*   **シグネチャ**: `pub fn load() -> Self`
*   **説明**: 設定のロードを試み（`try_load`）、失敗した場合はデフォルトの設定情報をロードした `ConfigStore` を返します。

#### `try_load`
*   **シグネチャ**: `pub fn try_load() -> Result<Self, EneConfigError>`
*   **説明**: ローカルの構成ファイル（`config.json`）をパースし、メモリ内のキャッシュ設定ツリーとしてロードします。

#### `from_config`
*   **シグネチャ**: `pub fn from_config(config: EneConfig) -> Self`
*   **説明**: メモリ内に直接構成設定を指定して初期化します。

#### `config`
*   **シグネチャ**: `pub fn config(&self) -> EneConfig`
*   **説明**: 現在の有効な構成設定情報のクローンコピーを取得します。

#### `with_config_mut`
*   **シグネチャ**: `pub fn with_config_mut(&self, f: impl FnOnce(&mut EneConfig))`
*   **説明**: 設定パラメータを変更し、メモリのダーティ（変更あり）フラグを真にセットします。

#### `set_config`
*   **シグネチャ**: `pub fn set_config(&self, config: EneConfig)`
*   **説明**: 構成設定情報を上書きセットし、ダーティフラグをセットします。

#### `get_section`
*   **シグネチャ**: `pub fn get_section<T>(&self) -> T where T: serde::de::DeserializeOwned + Default + crate::HasConfigKey`
*   **説明**: キー名に対応する設定ブロック（AI、DB、UI、SpringBone 等）を逆シリアライズして取得します。

#### `set_section`
*   **シグネチャ**: `pub fn set_section<T>(&self, section: &T) where T: serde::Serialize + crate::HasConfigKey`
*   **説明**: 設定ブロックの内容を指定し、キャッシュツリーを更新してダーティフラグを設定します。

#### `character_config`
*   **シグネチャ**: `pub fn character_config(&self) -> CharacterConfig`
*   **説明**: キャラクター固有の設定情報を取得します。

#### `load_character_config`
*   **シグネチャ**: `pub fn load_character_config(&self, character_name: &str)`
*   **説明**: キャラクターの設定ファイルをストレージからロードします。

#### `set_character_config`
*   **シグネチャ**: `pub fn set_character_config(&self, config: CharacterConfig)`
*   **説明**: キャラクター設定をアクティブキャッシュに割り当てます。

#### `get_character_section` / `set_character_section`
*   **シグネチャ**: `pub fn get_character_section<T>(&self) -> T ...` (および書き込みメソッド)
*   **説明**: キャラクター構成配下のサブ設定ブロックを取得・更新します。

#### `flush_if_dirty`
*   **シグネチャ**: `pub fn flush_if_dirty(&self, character_name: Option<&str>) -> std::io::Result<bool>`
*   **説明**: メモリ変更フラグ（ダーティフラグ）が設定されている場合のみ、設定変更をファイルシステムにフラッシュ（永続化）保存します。

#### `flush`
*   **シグネチャ**: `pub fn flush(&self, character_name: Option<&str>) -> std::io::Result<()>`
*   **説明**: ダーティフラグの状態にかかわらず、強制的にすべての設定情報をファイルシステムへ書き出します。

#### `is_dirty`
*   **シグネチャ**: `pub fn is_dirty(&self) -> bool`
*   **説明**: キャッシュに変更が生じている（ディスクと不一致である）かを返します。

---

## 2. キャラクターカードパースおよびスキーマ処理 (`config.rs` & `character_card.rs`)

#### `update_global_config` / `get_global_config`
*   **シグネチャ**: `pub fn update_global_config(config: EneConfig)` (および読み込みメソッド)
*   **説明**: スレッド安全なアクターグローバル共有構成設定キャッシュを書き込み・取得します。

#### `__register_schema` / `__register_tool_schema`
*   **シグネチャ**: `pub fn __register_schema<T: JsonSchema + HasConfigKey>(target: ConfigTarget, parent_key: Option<&str>)`
*   **説明**: 構成オブジェクトに対応する JSON スキーマをバリデーターテーブルに登録し、CLI ツール等からのスキーマエクスポートの自動フックを提供します。

#### `EneConfig::get_section` / `set_section`
*   **Signature**: `pub fn get_section<T>(&self) -> Result<T, EneConfigError> ...`
*   **Description**: 特定のキーを設定オブジェクトにシリアライズします。

#### `generate_schema_json` / `generate_character_schema_json`
*   **シグネチャ**: `pub fn generate_schema_json() -> Result<String, serde_json::Error>`
*   **説明**: システムの構成設定検証用 JSON Schema 文字列を自動生成して書き出します。

#### `load_character_card`
*   **シグネチャ**: `pub fn load_character_card(name_or_path: &str) -> Result<crate::CharacterCardV3, crate::EneConfigError>`
*   **説明**: SillyTavern 形式のキャラクター定義ファイル（PNG のメタデータ領域、または JSON 文字列）を解決して、`CharacterCardV3` モデルとしてデシリアライズします。

#### `expand_cbs_macros`
*   **シグネチャ**: `pub fn expand_cbs_macros(text: &str, char_name: &str, user_name: &str) -> String`
*   **説明**: カードプロンプト内の `{{char}}` や `{{user}}` といったキャラクター記述プレースホルダーを、動的な会話名に置換・マクロ展開します。

#### `resolve_expressions`
*   **シグネチャ**: `pub fn resolve_expressions(card: &CharacterCardV3) -> Vec<ResolvedExpression>`
*   **説明**: カード内で定義されている個々の表情の名前、ブレンドシェイプ重み、および遷移パラメータの一覧を抽出解決します。

---

## 3. マクロ定義

#### `define_config!` / `define_tool_config!`
*   **説明**: 新たな構成設定用の構造体を宣言し、各種自動シリアライザの派生、グローバルキーのバインド、および自動 JSON Schema 登録用フックをコード生成するマクロです。

---

## 4. テキスト切り詰めユーティリティ (`truncate.rs`)

#### `Truncate::chars`
*   **シグネチャ**: `pub fn chars(text: &str, max_chars: usize) -> String`
*   **説明**: 文字列が指定文字数を超える場合に、文字の末尾を安全に切り詰めます。

#### `Truncate::simple`
*   **シグネチャ**: `pub fn simple(text: &str, max_chars: usize) -> String`
*   **説明**: 指定文字数を超過した場合、切り詰めた末尾に三点リーダー「...」を付加します。

#### `Truncate::detailed`
*   **シグネチャ**: `pub fn detailed(text: &str, max_chars: usize) -> String`
*   **説明**: トークン圧迫アラート等のために、何文字切り詰められたかのメタ情報ラベル（例: `[truncated 200 chars]`）を末尾にバインドして切り詰めます。

#### `Truncate::output` / `Truncate::tail`
*   **Signature**: `pub fn output(text: &str, max_lines: usize, max_bytes: usize) -> TruncateResult`
*   **Description**: 改行行数またはバイトサイズの限界に沿って文字列を切り詰めます。

---

## 5. キャッシュおよびデータディレクトリ解決パス (`paths.rs`)

#### `app_data_dir`
*   **シグネチャ**: `pub fn app_data_dir() -> PathBuf`
*   **説明**: アプリケーションの基本データ領域（例: `~/.gemini/antigravity/`）への絶対パスを解決します。

#### `assets_dir`
*   **シグネチャ**: `pub fn assets_dir() -> PathBuf`
*   **説明**: キャラクターアセットや静的リソースが格納されるフォルダへのパスを解決します。

#### `models_dir`
*   **シグネチャ**: `pub fn models_dir() -> PathBuf`
*   **説明**: ダウンロードされた量子化 GGUF モデルファイルがキャッシュされるディレクトリパスを返します。

#### `config_file_path`
*   **シグネチャ**: `pub fn config_file_path() -> PathBuf`
*   **説明**: 主要な `config.json` ファイルの配置パスを返します。

#### `tool_socket_dir`
*   **シグネチャ**: `pub fn tool_socket_dir() -> PathBuf`
*   **説明**: ツール IPC プロキシサーバーの Unix Domain Socket ファイルを配置する一時ディレクトリを返します。
