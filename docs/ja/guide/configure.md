# 設定

最初に触ることが多い設定の短い案内です。全フィールド表は [設定リファレンス](../reference/configuration/settings.md) にあります。

## 設定の場所

読み込み順（`figment`）:

1. コンパイル時デフォルト
2. OS のユーザー設定ディレクトリ、または開発時のローカル `assets/settings.json`
3. `ENE_` 環境変数

`assets/` 配下の schema JSON は CLI 実行時に自動生成されます。手編集・コミットしないでください。

## よく使うセクション

| セクション | 用途 |
|------------|------|
| `provider` | チャットモデル、base URL、API キー、埋め込み |
| `character` / カードパス | 読み込むキャラクターカード |
| `store` | 永続化の on/off と DB パス |
| `mind.*` | 想起、圧縮、感情、Performance ポリシー |
| `tools` | バイナリ有効化、サンドボックス、MCP、Tool RAG |

トップレベルの `memory.*` ポリシーや二重の「cognition」スイッチはありません。ストリーミング経路は mind のみです。永続化は `store`、想起・書き込み方針は `mind.*`。

## プロバイダの例

```json
{
  "provider": {
    "name": "openai-compatible",
    "model": "gpt-4o-mini",
    "base_url": "https://api.openai.com/v1",
    "api_key": { "source": "env", "env": "OPENAI_API_KEY" }
  }
}
```

## キャラクター解決

- 空 → assets 配下のデフォルト Alicia カード
- 名前のみ → `assets/characters/{name}/character.json`
- パス → そのまま使用

## 設定フィールドを追加する（コントリビューター）

1. `crates/ene-config/src/config.rs` の構造体を編集（`define_config!`）
2. `cargo run -p ene-cli` を一度実行して schema を再生成
3. [設定リファレンス](../reference/configuration/settings.md) を EN + JA で更新

## 次へ

- [はじめに](getting-started.md)
- [CLI](apps/cli.md) / [Desktop](apps/desktop.md)
- [設定リファレンス全文](../reference/configuration/settings.md)
