# キャラクターカードの多言語対応

CCv3 には LLM が実際に読むフィールドの多言語化手段がありません。`creator_notes_multilingual` は制作メモ（人間向けでプロンプトに送られない）だけを対象にしています。Ene は **差分ファイル** 方式でカードをローカライズします。基底の `character.json` の隣に置いた `character.{lang}.json` が翻訳対象フィールドだけを持ち、ロード時に基底へ重ねられます。翻訳されていないフィールドは基底言語にフォールバックします。

```
characters/Alicia/
  character.json        ← 基底（完全で有効な CCv3 カード）
  character.ja.json     ← 日本語差分: 翻訳対象フィールドのみ
  character_settings.json
  model.vrm
```

## 翻訳対象のフィールド

| 翻訳する | 翻訳しない |
|---|---|
| `description` / `personality` / `scenario` | `assets` |
| `first_mes` / `alternate_greetings` / `mes_example` | `extensions.ene.motion_catalog` |
| `system_prompt` / `post_history_instructions` | `extensions.ene.expressions`（VRM 重み） |
| `creator_notes` / `nickname` / `tags` | `creation_date` / `spec` / `spec_version` |
| `character_book` エントリの `content` / `keys` / `secondary_keys` | `insertion_order` / `priority` / `position` |
| `extensions.ene.speech` のテキスト項目（`first_person` / `second_person` / `verbal_tics`） | `extensions.ene.speech.length` / `politeness`（列挙選択） |
| `extensions.ene.ng_expressions`（リスト全体の置換） | `extensions.ene.relationship_stages` の閾値 |
| `extensions.ene.style_examples` の `label` / `text`（`id` で照合） | `extensions.ene.relationship_stages` の照合キー `threshold` |
| `extensions.ene.relationship_stages` の `label` / `tone`（`threshold` で照合） | `extensions.ene.time_periods` の照合キー `period` |
| `extensions.ene.time_periods` の `behavior`（`period` で照合） | `extensions.ene.scene_behaviors` の照合キー `name` |
| `extensions.ene.scene_behaviors` の `keywords` / `behavior`（`name` で照合） | |

ロアブックのトリガー（`keys` と `secondary_keys`）の翻訳は**必須**です。トリガーは会話テキストに対してマッチングされ、Ene のマッチャーはプライマリ 1 つ以上かつセカンダリ 1 つ以上の一致を要求するため、翻訳されていないトリガーは基底言語以外の会話では発火しません。

同じ規則が `extensions.ene` のロールプレイ定義にも適用されます。シーンの
行動の `keywords` はローカライズ済みの場面テキストに対して、ラベル付き応答例の
`label` はユーザー入力に対してマッチングされるため、どちらも翻訳対象です。
照合キー（`id` / `threshold` / `period` / `name`）は差分エントリを基底
エントリに紐付けるためだけに存在し、翻訳されません。ロアブックと同様に、
存在しないロールプレイブロックや未知のキーを参照する差分は警告付きでスキップ
されます。ロケール差分は既存の構造を上書きできても、新しい構造を追加することは
できません。

カードの `name` は意図的に翻訳対象外です。発見処理やフォルダー名に使うキャラクターの識別キーだからです。翻訳できるのは表示専用の `nickname` だけです。CCv3 の `creator_notes_multilingual` はレガシーカード用に引き続き対応しますが、新規カードは差分ファイルを使うべきです。

## 差分ファイルの形式

```json
// character.ja.json
{
  "description": "明るいデスクトップコンパニオン。",
  "first_mes": "やっほー、何してるの？",
  "nickname": "アリス",
  "character_book": {
    "entries": [
      {
        "id": "lore-1",
        "keys": ["猫", "ねこ"],
        "secondary_keys": ["ペット"],
        "content": "日本語のロアエントリ。"
      }
    ]
  }
}
```

全フィールドが省略可能です。値があれば基底を置き換え、キーが無ければ基底言語を維持します。ロアブックのエントリは `id` で基底カードのエントリと照合します。一致する id が無いエントリは警告してスキップし、追加はしません。`alternate_greetings` と `tags` は、存在する場合に基底のリスト全体を置き換えます。

差分ファイルが壊れていてもカードは壊れません。警告してスキップし、基底カードを返します。

未知のフィールドは黙って無視せず拒否します。`"first_mess"` のようなタイポは「未翻訳フィールド」と同じ見た目ではなく、差分全体が警告付きでスキップされます（新しいフィールドを追加した将来版 Ene 向けの差分は、古い Ene では同じ経路でスキップされます）。

## 実効ロケールの選び方

実効ロケール = **カード個別オーバーライド**（`character_settings.json` の `language`）→ **アプリ言語** → システムロケール。値は `resolve_language_alias` で正規化され（`ja-JP` と `jp` は `ja` に、未知の値は `en` になる）、差分ファイルは正規化コードで `character.{code}.json` として探します。

- デスクトップ: アプリ言語は `mind.language`。設定画面が UI 言語（`desktop.language`）と同期しています。
- CLI: アプリ言語は実効 i18n ロケール — `--lang` フラグがあればそれ、無ければシステムネゴシエーションのロケールです。

カード個別オーバーライドは `character_settings.json` に置きます:

```json
{ "language": "ja" }
```

空（デフォルト）はアプリ言語を継承します。オーバーライドはフォルダー形式のカードだけに適用されます。CHARX / PNG を直接読む場合は settings ファイルがありません。

## 配布形式との対応

| 形式 | レイアウト |
|---|---|
| フォルダー（作業形式） | `character.json` + `character.{lang}.json` サイドカー |
| CHARX | zip ルートに `card.json` + `character.{lang}.json` |
| PNG | 単一言語にマージ済み、または `extensions.ene.locales.{lang}` に差分を埋め込み |
| エクスポート | 基底 + 差分をマージした完全な単一言語 CCv3 カード |

ローダーはすべての形式を同じ in-memory 結果に正規化します。`extensions.ene.locales` を剥がした単一言語マージ済みカードになるため、PNG から読んだカードとフォルダーから読んだカードは、保存時に同一バイト列・同一メモリハッシュになります。ロケール指定なしの `load_character_card`（基底）は従来どおり基底カードを返します — `character.json` 単体は有効な CCv3 カードのままです。

PNG / CHARX カードのインポートはフォルダー作業形式をマテリアライズします。埋め込みロケールは `character.{lang}.json` サイドカーとして書き出され（CHARX 由来のサイドカーが優先）、`character.json` からは除去されます。

`export_character_card` は明示した言語で基底 + 差分をマージし、`save_character_card`（atomic）で完全な単一言語カードを書き出します。PNG への焼き込み自体は未実装です。エクスポートされた JSON が将来の焼き込み処理の入力になります。

## 実行時の言語切替

カードは起動時（CLI では `/card`）に一度だけ読み込まれます。アプリ言語を切り替えると、次にキャラクターを開いたときにカードを再読み込みします。実行中のセッションは開始時のカードを保持します（ランタイムが従来からキャラクターカードを再読み込みしないのと同じ扱いです）。
