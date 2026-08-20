# キャラクターパッケージ

コンパニオンは **soul**（人格）と任意の **body**（VRM / モーション）を
キャラクターパッケージとして結びます。正規のインストール先は
`<data_dir>/characters/<id>@<version>/` です。フィールド契約は
`ene-companion` / `ene-card` の rustdoc です。

## 形式

| 形式 | 役割 |
|---|---|
| `.enechar` / `.enesoul` / `.enebody` | 正規パッケージ。`POST /api/v1/characters/import` で取り込む。 |
| Character Card V3（フォルダ、PNG `ccv3`、CHARX zip） | インポート専用。データディレクトリのパッケージへ変換する。 |

`assets/characters/Alicia/` は V3 の **開発用フィクスチャ**であり、実行時の
配置ではありません。インポートは既存インストールを上書きしません。
エントリ単位とアーカイブ合計のサイズ上限があります。

V3 の `data`（`name`、`description`、`personality`、lorebook など）と
`extensions.ene`（表情、`motion_catalog.motions`、speech）はインポート時に
写されます。未知フィールドは `ene-card` でラウンドトリップします。

## lorebook とテンプレート

`character_book` はキーワード一致で注入します（`before_char` /
`after_char`）。カード文は Character Book Spec マクロ（`{{char}}`、
`{{user}}`、`{{random:…}}`、`{{date}}` など）を使えます。W++ / AliChat /
YAML のペルソナ文はアイデンティティカーネルに平坦化されます。

フォルダカードは `character.<lang>.json` を隣に置けます。stage の表示
（`character_settings.json`: 位置、スケール、既定モーション / 表情）は
V3 フィクスチャまたはインストール済みパッケージの隣にあります。

[キャラクターエディタ](../guides/character-editor.md) も参照してください。
