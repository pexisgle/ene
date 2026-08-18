# キャラクターカード

キャラクターカードは 1 人のキャラクターの定義です: 誰で、どう話し、何を
知っていて、どう見えてどう動くか。Ene は
[Character Card Spec V3](https://github.com/kwaroran/character-card-spec-v3)
を実装し、Ene 独自の拡張ブロックを追加します。

## カードの形式とインポート

Ene は 3 種類のコンテナからカードを読み込み、`assets/characters/<name>/`
のフォルダ形式に正規化します:

| 形式 | 内容 | ディスク上の展開 |
|---|---|---|
| フォルダ形式 | `character.json` + `avatar.png` + アセットファイル | ネイティブ形式。その場で編集 |
| PNG カード | PNG のテキストチャンク（V3 は `ccv3`、V2 は `chara`）にカード JSON を埋め込んだもの | フォルダとしてインポート。PNG は `avatar.png` として保持 |
| CHARX | カード JSON + アセット（VRM・VRMA・画像）を含む ZIP | エントリ単位で検証しながらフォルダに展開 |

インポートは既存のキャラクターフォルダを上書きしません。`ene-card` は PNG
と CHARX を受け付けます。`ene-ctl import` はまだありません —
`assets/characters/<name>/` にフォルダを置くか、ホストから `ene_card` を
呼んでください。素の JSON ファイルはインポートされません
（キャラクターフォルダに直接置くのが正しい形式です）。

インポート時のサイズは検証されます（エントリ単位・アーカイブ合計の上限）。
悪意あるアーカイブでディスクやメモリが枯渇することはありません。

## カードに含まれるもの

コアの `data` オブジェクト（V3 規格）:

| フィールド | 意味 |
|---|---|
| `name`, `nickname` | キャラクター名。`nickname` があれば優先 |
| `description`, `personality`, `scenario` | 誰・どんな性格・どこで — アイデンティティカーネルにコンパイル |
| `system_prompt`, `post_history_instructions` | システム指示。PHI は履歴の後に追加 |
| `first_mes`, `alternate_greetings` | 開始メッセージ |
| `mes_example` | 初回ターンに示す例示会話 |
| `character_book` | lorebook（下記参照） |
| `authors_note`, `authors_note_depth` | 履歴の指定深度に注入する持続指示 |
| `creator_notes`, `tags`, `source`, `creation_date` | 由来・メタデータ |
| `assets` | `ccdefault:` URI による外部ファイル（VRM・VRMA・アイコン）参照 |
| `extensions.ene` | Ene 固有ブロック（下記参照） |

未知のフィールドは編集・保存時も保持されるため、他ツールで作ったカードも
データ損失なく往復できます。

## `extensions.ene` ブロック

Ene 固有の挙動はここに置きます:

| キー | 意味 |
|---|---|
| `expressions` | アバターが表示できる名前付き表情（ブレンドシェイプ）定義 |
| `motion_catalog` | レイヤー分類付きの名前付きモーションクリップ（VRMA） |
| `affect_baseline` | 感情の減衰が収束する静止 PAD 感情 |
| `speech` | アイデンティティカーネルに描画される話し方定義（長さ・丁寧さ） |
| `ng_expressions` | キャラクターが絶対に言ってはいけないフレーズ（出力契約） |
| `style_examples` | 状況ラベル付きの応答例 |
| `relationship_stages` | 親密度に応じた話し方 |
| `time_periods` | ローカル時刻で切り替わる行動（朝/夕/夜） |
| `scene_behaviors` | アクティブシーンのキーワードで切り替わる行動 |
| `locales` | 言語別カード差分（PNG 配布カード用） |

## CBS マクロ

カードのテキストはモデルに届く前に Character Book Spec のテンプレート
マクロが展開されます:

| マクロ | 展開 |
|---|---|
| `{{char}}`, `<char>`, `<bot>` | キャラクター名 |
| `{{user}}` | ユーザー名 |
| `{{random:a,b,c}}` | 毎回振り直すランダム選択 |
| `{{pick:a,b,c}}` | セッション内で安定した選択 |
| `{{roll:d20}}` | 1..N のダイス |
| `{{reverse:text}}` | 反転テキスト |
| `{{comment:...}}`, `{{//...}}` | 削除 |
| `{{description}}`, `{{personality}}`, `{{scenario}}` | カードのフィールド |
| `{{persona}}` | ユーザーペルソナのテキスト |
| `{{user_persona}}` | 構造化ユーザーペルソナのフィールド |
| `{{date}}`, `{{time}}`, `{{isodate}}`, `{{isotime}}`, `{{weekday}}` | 現在時刻 |
| `{{idle_duration}}` | 前回ユーザー操作からの経過時間 |

`{{pick}}` はセッション単位のシードを使うため、同じチャットでは毎ターン
同じ選択になります。`{{random}}` は毎回振り直します。

## Lorebook

lorebook（`character_book`）はキーワードと内容を持つエントリのリストです。
エントリはキーワードが会話と一致したときに活性化します。Ene は 2 つの
注入位置を区別します:

- `before_char` — アイデンティティカーネルの前に置く保証エントリ。
- `after_char`（デフォルト）— キャラクター説明の後に置く保証エントリ。
  残りの一致エントリはセマンティックコンテキストセクションへ流れます。

毎ターンの注入に加えて、lorebook の内容はカード読み込み時に**セマンティック
メモリとしてメモリストアへ同期**されます（埋め込み付き）。そのため、
キーワードが逐語的に現れなくても関連エントリを想起できます。

## ペルソナ形式

`personality`/`description` が疑似構造化形式のとき、属性行にパースされて
アイデンティティカーネルが内容だけを保持します（形式の構文は落とします）:

- **W++** — `[character("Name"){Attribute("value")…}]` ブロック。
- **AliChat** — 標準 AliChat キーセットを使う `Key: value` テキスト。
- **YAML** — フラットな `key: value` マッピング。

認識されないテキストはそのまま使われます。

## ローカライズ

カードは言語別バリアントを同梱できます:

- フォルダ/CHARX カード: カードの隣の `character.<lang>.json` サイドカー
  （ローカライズ対象フィールドだけを含む）。
- PNG カード: カードに埋め込まれた `extensions.ene.locales` バッグ。

アクティブなロケールは stage の言語 / `character_settings.json` の
`language` オーバーライドから選ばれ、差分がベースカードの上に重ねられます。

## キャラクターごとの表示設定

カードの隣の `character_settings.json` に stage の表示を置けます:
モデルの位置/スケール・視線追従の強さ・デフォルトモーション・デフォルト
表情・カード言語。詳細は
[キャラクターエディタ](../guides/character-editor.md) です。
