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
| `/characters` | `assets/characters/` 配下で見つかったキャラクターを一覧表示 |
| `/import <パス>` | キャラクターカード（PNG または CHARX）を `assets/characters/` へインポート |
| `/session list` | SQLite 内の過去・現在のセッション一覧を表示 |
| `/session split` | 手動で即時にセッション境界を分割 |
| `/quit` または `/exit` | `ene-runtime` を安全にシャットダウンして終了 |

---

## 非対話サブコマンド

### `ene characters list`

`assets/characters/` 配下で見つかったキャラクターを一覧表示します（デスクトップと同じ規則を使用: `character.json` を含むフォルダーをキャラクターとして扱います）。

```bash
# 人間向け表示
ene characters list

# 機械可読な JSON（name, folder, card/vrm/motion パス, デフォルトモーション）
ene characters list --json
```

### `ene characters import <パス>`

キャラクターカードを `assets/characters/` へインポートします。PNG カード（Chub.ai / JanitorAI 形式、`ccv3` または旧 `chara` tEXt チャンク）と CHARX アーカイブ（ルートに `card.json` を持つ zip）に対応しています。カードは `characters/{カード名}/` フォルダーとして展開されるため、次回スキャン時にデスクトップからも発見されます。既存フォルダーは上書きせずインポートを拒否します。

```bash
# PNG / CHARX カードをインポート
ene characters import path/to/card.png
ene characters import path/to/card.charx

# 機械可読な結果（name, folder, card_path）
ene characters import path/to/card.png --json
```

同じ操作は REPL の `/import <パス>` でも実行できます。リモート（`http(s)://`）アセット URI を持つカードは、そのアセットをダウンロードせずにインポートします（URI は検証され、カード上に保持されます）。

---

## 自発発話（プロアクティブ）

`mind.proactive.enabled = true` の場合、Ene はあなたが入力中でないときでも自発的に発話することがあります。REPL はチャットイベントバスを常時購読しているため、待機中に発生した自発発話はプロンプトの上部に描画されます（デスクトップアプリと同じ挙動です）。入力中に自発発話のターンが始まった場合、編集中の行はキャンセル（入力内容は破棄）され、発話の終了後にプロンプトが再開します。
