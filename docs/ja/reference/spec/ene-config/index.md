# `ene-config` 設定・キャラクターカード仕様

`ene-config` クレートは、JSON形式の設定ファイルのロード/セーブ、キャラクターカードV3 (CharacterCardV3) の解析、スキーマ生成、自動保存（Dirtyトラッキング付き）、および設定定義用の declarative macros を提供する構成管理モジュールです。

---

## 1. 主要データ構造

### `EneConfig` (公開 / 構造体)
システム全体の設定項目を保持するトップレベル構造体。内部に各サブセクション（`ai`, `store`, `mind`, `tools`, `desktop` 等）のJSON表現をマッピングし、`get_section::<T>()` メソッドで型安全に取り出せます。

### `ConfigStore` (公開 / 構造体)
ディスク上の `config.json` を管理する設定ストア。
*   `load() -> Result<Self, ConfigError>`: 設定ファイルをロード。存在しない場合はデフォルト構成で自動作成。
*   `try_load()`: フェイルハード版ロード。CLI起動などで使用。
*   `save(&self) -> Result<(), ConfigError>`: `dirty` フラグが立っている場合のみファイルへシリアライズ。

### `CharacterCardV3` (公開 / 構造体)
SillyTavern等の規格に準拠した V3 キャラクターカードモデル。自己認識、キャラクター名、立ち絵アセットパス、Lorebook、表情BlendShape定義などを保持します。

---

## 2. マクロによるスキーマ自動登録 (CBSマクロ)

Ene では、設定構造体をボイラープレートなしで宣言し、自動的にJSONスキーマ検証用のフックを差し込むためのマクロを提供しています。

### `define_config!`
*   **構文例**:
    ```rust
    define_config!(
        settings,
        "mind.emotion",
        pub struct EmotionConfig {
            pub enabled: bool = true,
            pub decay_half_life_minutes: f64 = 60.0,
        }
    );
    ```
*   **マクロ展開ロジック**:
    -   自動的に `Serialize`, `Deserialize`, `JsonSchema` トレイトを derive します。
    -   `Default` トレイトの実装で、`field = default` に指定されたインライン既定値を適用。
    -   **Ctorによる静的登録**:
        `linkme` または `ctor` クレート（`#[ctor]` マクロ）を用いて、プログラム起動時に自動で `__register_schema` を呼び出し、グローバルスキーマレジストリにスキーマ定義と構成キー（例: `mind.emotion`）を登録します。

### `define_tool_config!`
*   外部ツールの設定用。`define_config!` と同様のスキーマ検証をツールバイナリ向けに提供します。

---

## 3. スマートテキスト切り詰め (`Truncate`)

文字数によるコンテキスト制限や表示切り詰めの際、マルチバイト文字（日本語のひらがな・漢字など）がコードポイントの途中で切断されて文字化けするのを防ぐユーティリティ。

*   `Truncate::truncate_chars(&self, max_chars: usize) -> TruncateResult`:
    -   UTF-8 文字境界（Unicode Scalar Value）を正しく認識し、`max_chars` 文字に収まるように安全にスライス。
    -   切り詰めが発生した場合は、末尾に `...` 等を安全に追加します。
