# キャラクターエディタ

製品 GUI（`ene-stage`）にアプリ内 CCv3 フォームはありません。パッケージまたは
V3 フィクスチャを編集し、デーモンに再読込させます。旧 `ene-desktop` の
Character editor はローカル `character.json` を書きますが、v1.0 の経路では
ありません（[製品境界](../concepts/product-boundaries.md)）。

## 1. モーションと表情

正規パッケージは `<data_dir>/characters/<id>@<version>/` に置きます。
V3 サンプル `assets/characters/Alicia/character.json` は `extensions.ene`
を使います:

```json
{
  "extensions": {
    "ene": {
      "motion_catalog": {
        "motions": [
          { "name": "wave", "path": "motions/wave.vrma" }
        ]
      },
      "expressions": [
        { "name": "smile", "vrm": { "happy": 0.8 }, "enabled": true }
      ]
    }
  }
}
```

`.vrma` はカードの隣に置き、`path` で参照します。`vrm` はモデル上の
ブレンドシェイプ名です。翻訳は `character.ja.json` を隣に置きます。

## 2. キャラクターごとの表示

`character_settings.json` に stage の配置（位置、スケール、視線、既定
モーション / 表情、言語）を置けます。読むのは stage です。

```json
{
  "character_position": [0.0, 0.0, 0.0],
  "model_scale": 1.0,
  "look_at_strength": 0.6,
  "default_motion": "idle",
  "default_expression": "neutral",
  "language": ""
}
```

`default_motion` は `motion_catalog.motions` と、`default_expression` は
`expressions`（または VRM 組み込みの `neutral` など）と一致する必要があります。

## 3. インポート

`ene-card` / `POST /api/v1/characters/import` は PNG（ccv3/chara）と CHARX
を読みます。インポート先はデータディレクトリで、既存インストールは
上書きしません。Alicia フォルダはローカル編集用の V3 フィクスチャであり、
インストール先ではありません。

`assets/schema/` の `character.schema.json`（設定初期化時に再生成）で
エディタのカード JSON を検証できます。
