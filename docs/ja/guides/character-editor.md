# キャラクターエディタ

キャラクターは「ファイルに触らない」から「全制御」まで、3 段階で編集できます。

## 1. デスクトップのキャラクターエディタ

設定 → キャラクターエディタは、アクティブカードのフィールドをフォームで
編集します: 名前・説明・性格・シナリオ・最初のメッセージ・システム
プロンプト・PHI・例示会話・Ene 固有の表情/モーション。変更はカードの
`character.json` に保存され、次のターンから反映されます。

## 2. キャラクターごとの表示設定

`assets/characters/<name>/character_settings.json` がデスクトップシーンを
制御します（[設定 → キャラクターごとの設定](../configuration.md#キャラクターごとの設定-character_settingsjson)参照）:

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

`default_motion` はカードの `extensions.ene.motion_catalog` のエントリと、
`default_expression` は `extensions.ene.expressions`（または VRM 組み込みの
`neutral` など）と一致する必要があります。

## 3. カードファイル自体

カードは `character.json`（V3 規格 + `extensions.ene`）です。フィールドの
完全なリファレンスは[キャラクターカード](../concepts/character-cards.md)を
参照してください。よく使う編集パターン:

- **モーションを追加** — `.vrma` ファイルをキャラクターフォルダに置き、
  それを参照する `motion_catalog` エントリを追加:

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

- **表情を追加** — VRM モデルのターゲット名を持つブレンドシェイプ表情を定義:

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

- **カードをローカライズ** — カードの隣に `character.ja.json`
  （または `character.en.json`）を、翻訳フィールドだけ入れて追加。

## 4. 外部からカードをインポート

```sh
ene --character "" import /path/to/card.png
# または REPL で:
/import /path/to/card.charx
```

インポートは既存のキャラクターフォルダを上書きしません。インポート後は
`/card <name>` か設定 → キャラクターで新しいキャラクターに切り替えます。

## 変更の検証

- `/characters` で発見されたキャラクターとカードパスを確認。
- `/card <name>` で実行中の CLI のカードを再読み込み。
- `/doctor` で設定とストアの健康状態を確認。
- `assets/schema/` の `character.schema.json` がエディタでカード JSON を
  検証します（起動時に再生成）。
