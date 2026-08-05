# derive マクロ

`ene-plugin-macros` は、プレーンな構造体をプラグインの表面に変える
derive/属性マクロを提供します。このページは属性リファレンスです。

## `#[derive(ToolAction)]`

アクション構造体の `ToolAction` 実装（仕様・引数パース・ディスパッチ）を
生成します。構造体の `#[tool(...)]` とフィールドの `#[arg(...)]` を読みます。

## `#[derive(ToolSpec)]`

挙動なしで構造体の `ToolSpec`（スキーマ+メタデータ）だけを生成します。
サブスキーマとして共有される型に便利です。

## `#[tool_action]`（属性マクロ）

関数スタイルのアクションに `#[tool(...)]` メタデータを付ける代替形式です。
正確な展開契約は `crates/ene-plugin-macros/src/lib.rs` を参照してください。

## `#[tool(...)]` コンテナ属性

`ToolAction`/`ToolSpec` 構造体に適用:

| キー | 型 | 意味 |
|---|---|---|
| `namespace` | string | 名前空間プレフィックス（必須） |
| `name` | string | アクション名（必須） |
| `summary` | string | モデル向け 1 行要約 |
| `description` | string | 完全な説明 |
| `category` | string | 表示カテゴリ |
| `keywords_primary`, `keywords_secondary` | string | カンマ区切り検索キーワード |
| `side_effects` | string | 副作用宣言（例: `"FileSystem { mutates: true }"`） |
| `background_capable` | flag | 遅延実行を許可 |
| `internal` | flag | ツールレジストリ/スキーマから隠す |

## `#[arg(...)]` フィールド属性

| キー | 型 | 意味 |
|---|---|---|
| `internal` | flag | JSON スキーマから除外 |
| `enum_values` | string | カンマ区切りの許可値 |
| `default` | string | スキーマのデフォルト |
| `minimum`, `maximum` | int | 数値の境界 |
| `min_length`, `max_length` | int | 文字列長の境界 |
| `min_items`, `max_items` | int | 配列の境界 |
| `description` | string | フィールド doc コメントの上書き |

## プロバイダー derive

| Derive | 生成物 |
|---|---|
| `#[derive(LlmPlugin)]` | `llm_spec()` + `LLM_PROVIDER_KIND` |
| `#[derive(TtsPlugin)]` | `tts_spec()` + `TTS_PROVIDER_KIND` |
| `#[derive(SttPlugin)]` | `stt_spec()` + `STT_PROVIDER_KIND` |
| `#[derive(VadPlugin)]` | `vad_spec()` + `VAD_PROVIDER_KIND` |

4 つすべてが 1 つの `#[provider(...)]` 属性を共有します（
`#[derive(LlmPlugin, TtsPlugin)]` のような複合プロバイダーも単一属性）:

| キー | 適用対象 | 意味 |
|---|---|---|
| `kind` | すべて | プロバイダー種別文字列（例: `"openai"`・`"local"`） |
| `models` | LLM | カンマ区切りモデル名 |
| `voices`, `formats` | TTS | ボイス/形式の一覧 |
| `streaming` | LLM | ストリーミングチャット対応 |
| `vision` | LLM | 画像入力対応 |
| `context_window` | LLM | 公表コンテキストサイズ |
| `max_in_flight`, `queue_depth` | すべて | 受付ヒント |
| `frame_size`, `sample_rate` | STT/VAD | 音声パラメータ |
| `resource_class` | LLM | 受付用リソースクラス |
| `provides`, `requires` | プラグイン全体 | capability 宣言（`"llm/chat@1, embed@1"`） |

derive は仕様コンストラクタだけを生成します。非同期ハンドラを含む
`impl LlmPlugin { ... }` ブロックは手書きです。`provides`/`requires` メソッド
は `LlmPlugin` 展開からのみ生成されるため、capability を宣言するには
LLM derive と TTS/STT/VAD derive を組み合わせてください。

capability 文字列はコンパイル時に文法検証されるため、タイポはビルド失敗に
なります。

## `define_config!` / `define_tool_config!`

プロシージャルマクロではありませんが、設定の相棒です。`ene-config` の宣言的
マクロが設定セクション（起動時に JSON スキーマレジストリへ登録）とツール
設定スキーマを定義します。[設定](../../configuration.md) 参照。
