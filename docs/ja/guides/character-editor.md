# キャラクターエディタ

プロセス内の設定フォームはありません。カードファイルを編集し、デーモンに
キャラクターパッケージを再読込させます。

## 1. カードファイル

カードは `character.json`（V3 規格 + `extensions.ene`）です。フィールドは
[キャラクターカード](../concepts/character-cards.md) を参照してください。

よく使う編集:

- **モーションを追加** — `.vrma` をキャラクターフォルダに置き、それを参照
  する `motion_catalog` エントリを追加します:

```json
{
  "extensions": {
    "ene": {
      "motion_catalog": {
        "entries": [
          { "name": "wave", "file": "wave.vrma", "layer": "overlay" }
        ]
      }
    }
  }
}
```

- **表情を追加** — VRM モデルのターゲット名でブレンドシェイプ表情を定義
  します:

```json
{
  "extensions": {
    "ene": {
      "expressions": [
        { "name": "smile", "target": "happy", "weight": 0.8 }
      ]
    }
  }
}
```

- **カードをローカライズ** — 翻訳したフィールドだけを含む
  `character.ja.json`（または `character.en.json`）をカードの隣に置きます。

## 2. キャラクターごとの表示

カードの隣の `character_settings.json` に stage の表示（位置、スケール、
視線、デフォルトモーション / 表情、言語）を置けます。読むのは stage です。
`ene-core` は第二のコピーを持ちません。

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

`default_motion` は `extensions.ene.motion_catalog` と、
`default_expression` は `extensions.ene.expressions`（または VRM 組み込みの
`neutral` など）と一致する必要があります。

## 3. 他からカードを取り込む

`ene-card` は PNG（ccv3/chara チャンク）と CHARX（zip）を読みます。
インポートは既存フォルダを上書きしません。`ene-ctl import` はまだありません。
`assets/characters/<name>/` にフォルダを置くか、ホストから `ene_card` を
呼んでください。

`assets/schema/` の `character.schema.json`（設定初期化時に再生成）で
エディタのカード JSON を検証できます。
