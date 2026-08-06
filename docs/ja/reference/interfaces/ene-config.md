# `ene-config` インターフェース

## 役割

設定の一元管理・スキーマ生成・キャラクターカード・パス解決。各クレートの
設定セクションはここでマクロにより*定義*されますが、*所有*は定義元
クレートです。

## 公開モジュール

| モジュール | 内容 |
|---|---|
| `config` | `EneConfig`・ローダー（`load_config`・`load_full_config`・`get_global_config`）・`ConfigStore`・`register_config_schema`・`write_schemas`・`HasConfigKey`・`ConfigTarget` |
| `character_card` | `CharacterCardV3`・`CharacterCardData`・`EneExtension`・`Lorebook`/`LorebookEntry`・`UserPersona`・表情/感情/話し方/関係型・CBS マクロ展開（`expand_cbs_macros*`・`MacroContext`・`session_pick_seed`） |
| `card_import` | `import_character_file`・`ImportedCharacter`（PNG/CHARX → フォルダ） |
| `character_assets` | `ResolvedAssetUri`・`resolve_asset_uri`・`EneAssetKind`・`decode_data_payload`・`DEFAULT_VRM_PATH` |
| `characters` | `discover_characters`・`load_character_card(_localized)`・`export_character_card`・`resolve_character_path`・`CharacterEntry` |
| `character_config` | `CharacterConfig`・`MotionCatalog`・`MotionEntry`・`MotionLayer` |
| `locale` | `LocalizedCharacterFields` とフィールド別ローカライズ型・マージロジック |
| `migration` | `CURRENT_CONFIG_VERSION`・`apply_migrations`・`register_migration`・`MigrationFn` |
| `paths` | `assets_dir`・`app_data_dir`・`config_file_path`・`models_dir`・ソケットディレクトリ・`IS_DEV_BUILD` |
| `prompts` / `patterns` | `PromptLibrary`・`PatternLibrary`・`SUPPORTED_LANGUAGES`・`resolve_system_language`・`substitute` |
| `resources` / `store` | `ensure_resource_dirs`・`ConfigStore`（ダーティ追跡・自動保存） |
| `error` | `ConfigError`・`EneConfigError` |

## 主要マクロ

| マクロ | 目的 |
|---|---|
| `define_config!` | 設定セクション構造体と JSON スキーマ・登録を宣言（settings / character / ネストの各バリアント） |
| `define_tool_config!` | `tools.list.<tool>` 配下のツール設定スキーマを宣言 |
| `define_label_enum!` | 一貫した `label()` API を持つラベル付き enum を宣言 |

## 依存関係

- 依存: 内部なし。
- 利用: 全クレート・アプリ（設定セクションは所有クレート側に定義。例:
  `ene-ai::AiConfig`・`ene-mind::MindConfig`・`ene-store::StoreConfig`・
  `ene-plugin-host::PluginConfig`・`apps/ene-desktop`）。

## リファクタリングの注目点

- **設定の追加** = 所有クレートに新しい `define_config!`。スキーマレジストリ
  が起動時に自動で拾います。
- **設定の削除/リネーム** = `ene-config::migration` にマイグレーションを追加
  （`CURRENT_CONFIG_VERSION` を更新）し、ドキュメントも更新。
- `EneConfig` は未知のトップレベルキーを保存時に保持します（ラウンドトリップ
  安全）。設定面を再構成するときはこの挙動を保ってください。
- `assets/schema/` の生成 JSON スキーマはビルド生成物（gitignore 済み）。
  手編集しないでください。
- キャラクターカードは外部仕様（CCv3）が制約する共有フォーマットです。
  `CharacterCardData` の形状は仕様 + Ene 独自の拡張名前空間
  `extensions.ene` に従います。
