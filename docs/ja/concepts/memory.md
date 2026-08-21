# メモリ

メモリは明示的で、型があり、検査可能です。会話は追記専用のセッションログ
（`ene-session`）に残ります。事実は `ene-companion`（`companions.db`）に
あり、履歴を書き換えずに一覧・編集・忘却できます。

## 型付きメモリ

各行は次を持ちます:

- **種別（Kind）** — `episodic`・`semantic`・`user_profile`・`preference`・
  `commitment`。
- **スコープ** — `private`（書いた soul）または `shared`。
- **ソース** — `extraction`・`user_stated`・`tool`・`import`・`shared`。
- **確信度と重要度（confidence / salience）** — どちらも想起に影響します。
- **ジャーナル** — 作成 / 更新 / 忘却 / 上書き / 復元は追記専用です。

忘却した行もジャーナルに残ります。矛盾する書き込みは古い行を supersede
できます。

## メモリの生成

ターンの後、コンパニオンのメモリライターが:

1. 構造化シグナル（約束・ユーザー明示の事実・ツール結果）を抽出します。
2. 分類モデルが接続されていれば、追加の候補を分類します。
3. 仲裁者が重複と矛盾をスコアし、書き込み・却下・保留を決めます。

`mind.memory_approval.require_approval` が true（既定）のとき、保留候補は
キューで待ちます。`/api/v1/memories/pending` か対応する `ene-ctl memory`
コマンドで解決します。

## メモリの想起

各ターン、`ene-companion` の想起は title/content の重なり・新しさ・salience を
スコアします。埋め込みのクエリベクトルがあるとき（`ai.tasks.embedding` または
chat タスクのフォールバック）は、`memories.embedding` との cosine を同じ
ランカーに足します。埋め込みが未設定なら語彙想起のままです。トークンが重ならない
クエリは、以前にベクトルを保存していてもヒットしません。ヒットは
`ene-kernel::ContextRegistry` の `memory.semantic` に載ります。常設の
プロフィールと好みは `memory.user_profile`、期限切れでない約束は
`memory.commitments` です。読んだメモリは `access_count` が上がります。

## 忘却

`mind.forgetting.*` が salience を減衰します。忘却した行は通常の想起から
外れます。ピン留めはユーザー編集（salience を上げる / 行を残す）です。

## メモリの確認

```sh
ene-ctl memory list <soul>
ene-ctl memory edit <id> "<content>"
ene-ctl memory delete <id>
```

[メモリ台帳ガイド](../guides/memory-ledger.md)を参照してください。
