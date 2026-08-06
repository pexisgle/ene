# `ene-plugin-macros` インターフェース

## 役割

プロシージャルマクロクレート: プレーンな構造体をツールアクションと
プロバイダー仕様に変える derive。

## 公開マクロ

| マクロ | 生成物 |
|---|---|
| `#[derive(ToolAction)]` | `ToolAction` 実装一式（仕様 + 引数パース + ディスパッチ） |
| `#[derive(ToolSpec)]` | `ToolSpec`（スキーマ + メタデータ）のみ |
| `#[tool_action]` | `ToolAction` の属性マクロ形式 |
| `#[derive(LlmPlugin)]` / `#[derive(TtsPlugin)]` / `#[derive(SttPlugin)]` / `#[derive(VadPlugin)]` | トレイトごとの静的仕様コンストラクタ + kind 定数（例: `llm_spec()`・`LLM_PROVIDER_KIND`） |

## 解釈される属性

| 属性 | 適用先 | キー |
|---|---|---|
| `#[tool(...)]` | ツール構造体 | `namespace`・`name`・`summary`・`description`・`category`・`keywords_primary/secondary`・`side_effects`・`background_capable`・`internal` |
| `#[arg(...)]` | ツールフィールド | `internal`・`enum_values`・`default`・`minimum`/`maximum`・`min_length`/`max_length`・`min_items`/`max_items`・`description` |
| `#[provider(...)]` | プロバイダー構造体 | `kind`・`models`・`voices`・`formats`・`streaming`・`vision`・`context_window`・`max_in_flight`・`queue_depth`・`frame_size`・`sample_rate`・`resource_class`・`provides`・`requires` |

完全なリファレンスは [derive マクロ](../tools/derive-macro.md) を参照。

## 依存関係

- 依存: `ene-plugin-proto`（capability 文法検証）。
- 利用: `ene-plugin`（prelude へ再エクスポート）・プラグインバイナリ。

## リファクタリングの注目点

- capability 文字列（`provides`/`requires`）は**コンパイル時に**ワイヤ文法へ
  照合されます。タイポはハンドシェイクではなくビルド失敗になります。この
  性質を維持してください。
- 1 つの `#[provider(...)]` 属性が複合 derive
  （`#[derive(LlmPlugin, TtsPlugin)]`）を支えます。`provides`/`requires`
  メソッドは `LlmPlugin` 展開からのみ生成されます。この分割の変更は複合
  プロバイダーへの破壊的変更です。
- 非同期ハンドラは生成されません。作者がトレイト impl を書きます。derive は
  トレイトごとの inherent 仕様コンストラクタのみ生成します（1 構造体に
  2 つの derive があると inherent 名を共有できないため）。
