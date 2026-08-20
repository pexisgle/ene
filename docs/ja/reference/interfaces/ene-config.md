# `ene-config` インターフェース

## 役割

設定の一元管理・スキーマ生成・パス解決。設定セクションはここでマクロにより
*宣言*され、*所有*は定義元クレートです。キャラクターパッケージは
[`ene-card`](ene-card.md) / `ene-companion` にあります。

## 公開モジュール

| モジュール | 内容 |
|---|---|
| `config` | `EneConfig`・ローダー（`load_config`・`load_full_config`・`get_global_config`）・`ConfigStore`・`register_config_schema`・`write_schemas`・`HasConfigKey`・`ConfigTarget` |
| `migration` | `CURRENT_CONFIG_VERSION`・`apply_migrations`・`register_migration`・`MigrationFn` |
| `paths` | `assets_dir`・`data_dir`・`app_data_dir`・`config_file_path`・`models_dir`・ソケットディレクトリ・`IS_DEV_BUILD` |
| `prompts` / `patterns` | `PromptLibrary`・`PatternLibrary`・`SUPPORTED_LANGUAGES`・`resolve_system_language`・`substitute` |
| `resources` / `store` | `ensure_resource_dirs`・`ConfigStore`（ダーティ追跡・自動保存） |
| `user_persona` | `UserPersona`（`{{user_persona}}` CBS マクロが展開する構造化ペルソナ） |
| `error` | `ConfigError`・`EneConfigError` |

## 主要マクロ

| マクロ | 目的 |
|---|---|
| `define_config!` | 設定セクション構造体（`core`、`harness`、`mind`、`body`、`voice`、`store`、`approval`、`characters` など）と JSON スキーマ・登録を宣言（settings / character / ネストの各バリアント） |
| `define_label_enum!` | 一貫した `label()` API を持つラベル付き enum を宣言 |

プラグインのプロファイル行はファイバー状態（`ene-fiber`）であり、
`define_config!` セクションではありません。

## 依存関係

- 依存: 内部なし。
- 利用: 全クレート・アプリ（セクションは所有クレート側に定義）。

## リファクタリングの注目点

- **設定の追加** = 所有クレートに新しい `define_config!`。
- **設定の削除/リネーム** = `ene-config::migration` にマイグレーションを追加
  （`CURRENT_CONFIG_VERSION` を更新）し、ドキュメントも更新。
- `EneConfig` は未知のトップレベルキーを保存時に保持します。
- `assets/schema/` の生成 JSON スキーマは gitignore 済みです。
