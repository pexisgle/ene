# `ene-card` インターフェース

## 役割

キャラクターカードコンテナとインポート/エクスポート: V3 カードモデル
（CCv3）・PNG/CHARX コンテナ解析・キャラクター別設定・ローカライズ済み
カード差分。`ene-config` には共有エラー・パス・言語エイリアスのみ依存
します。

## 公開モジュール

| モジュール | 内容 |
|---|---|
| `character_card` | `CharacterCardV3`・`CharacterCardData`・`EneExtension`・`Lorebook`/`LorebookEntry`・表情/感情/話し方/関係型・CBS マクロ展開（`expand_cbs_macros*`・`MacroContext`・`session_pick_seed`） |
| `card_import` | `import_character_file`・`ImportedCharacter`（PNG/CHARX → フォルダ） |
| `character_assets` | `ResolvedAssetUri`・`resolve_asset_uri`・`EneAssetKind`・`decode_data_payload`・`DEFAULT_VRM_PATH` |
| `characters` | `discover_characters`・`load_character_card(_localized)`・`export_character_card`・`resolve_character_path`・`CharacterEntry` |
| `character_config` | `CharacterConfig`・`MotionCatalog`・`MotionEntry`・`MotionLayer` |
| `character_store` | `CharacterConfigStore`（`character_settings.json` のダーティ追跡・自動保存） |
| `locale` | `LocalizedCharacterFields` とフィールド別ローカライズ型・マージロジック |
| ルート | `save_character_card`・`generate_character_schema_json`・`generate_character_card_schema_json`・`write_character_schemas` |

## 依存関係

- 依存: `ene-config`（エラー・パス・言語エイリアスのみ）。
- 利用: `ene-companion`（V3 インポート）。

## リファクタリングの注目点

- `UserPersona` は `ene-config` に残ります: 設定レベルの型
  （`EneConfig.user_persona`）であり、CBS マクロが消費します。
- キャラクターカードは外部仕様（CCv3）が制約する共有フォーマットです。
  `CharacterCardData` の形状は仕様 + Ene 独自の拡張名前空間
  `extensions.ene` に従います。
- `card_import` は CHARX アーカイブのデコンプレッションボムとパストラバーサル
  （`..`）を防いでいます。触るときはこの防御を維持してください。
