# `ene-cli` ユーザーガイド

`ene-cli` は、Ene との対話、記憶の検査、セッションの管理、およびツールプラグインのテストを行えるコマンドライン REPL インターフェースです。

---

## CLI の起動

```bash
# デフォルト設定で起動
cargo run -p ene-cli

# カスタムキャラクターカードを指定して起動
cargo run -p ene-cli -- --character assets/cards/ene.json

# ログ詳細表示 (tracing) を有効化して起動
RUST_LOG=info cargo run -p ene-cli
```

---

## REPL スラッシュコマンド

`ene-cli` の対話型プロンプト内で `/` を入力することで各種コマンドを実行できます：

| コマンド | 説明 |
|---|---|
| `/help` | 利用可能なスラッシュコマンドの一覧を表示 |
| `/memory list` | アクティブセッションで想起された記憶ファクトを表示 |
| `/memory clear` | アクティブセッションの記憶をリセット |
| `/tool list` | 登録済み IPC ツールプラグインおよび MCP サーバーを表示 |
| `/tool call <名> <json>` | REPL から直接ツールアクションを実行 |
| `/session list` | SQLite 内の過去・現在のセッション一覧を表示 |
| `/session split` | 手動で即時にセッション境界を分割 |
| `/quit` または `/exit` | `ene-runtime` を安全にシャットダウンして終了 |
