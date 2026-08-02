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
| `/prompt` | AI に送信されるメッセージ列をそのままプレビュー（`build_messages` の出力を直接表示） |
| `/memory list` | アクティブセッションで想起された記憶ファクトを表示 |
| `/memory clear` | アクティブセッションの記憶をリセット |
| `/tool list` | 登録済み IPC ツールプラグインおよび MCP サーバーを表示 |
| `/tool call <名> <json>` | REPL から直接ツールアクションを実行 |
| `/auth list` | 保存済み・宣言済みの資格情報を一覧表示 |
| `/auth status <id>` | 1 つの資格情報の状態を表示 |
| `/auth revoke <id>` | 保存済みの資格情報を取り消し |
| `/auth authorize <id>` | CLI では非対応（ブラウザが必要）。案内を表示 |
| `/session list` | SQLite 内の過去・現在のセッション一覧を表示 |
| `/session split` | 手動で即時にセッション境界を分割 |
| `/quit` または `/exit` | `ene-runtime` を安全にシャットダウンして終了 |

---

## OAuth 資格情報

OAuth 認可フローはブラウザを開くため、CLI（特にヘッドレス環境）では実行でき
ません。OAuth サービスの認可はデスクトップアプリの設定画面（資格情報）で行って
ください。CLI では保存済みの資格情報の確認・取り消しを
`/auth list`・`/auth status <id>`・`/auth revoke <id>` で行えます。
詳細は [資格情報](../concepts/credentials.md) を参照してください。

---

## 自発発話（プロアクティブ）

`mind.proactive.enabled = true` の場合、Ene はあなたが入力中でないときでも自発的に発話することがあります。REPL はチャットイベントバスを常時購読しているため、待機中に発生した自発発話はプロンプトの上部に描画されます（デスクトップアプリと同じ挙動です）。入力中に自発発話のターンが始まった場合、編集中の行はキャンセル（入力内容は破棄）され、発話の終了後にプロンプトが再開します。
