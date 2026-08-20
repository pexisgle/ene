# メモリ台帳

台帳は `ene-companion` のメモリを検査する面です。プロセス内の設定ページは
ありません。HTTP API か `ene-ctl` を使います。

## CLI

```sh
ene-ctl memory list <soul>
ene-ctl memory edit <id> "<content>"
ene-ctl memory delete <id>
```

保留候補（`mind.memory_approval.require_approval` がオンのとき）は
`GET /api/v1/memories/pending` と
`POST /api/v1/memories/candidates/{id}/resolve` です。

## できること

| 操作 | 場所 | 効果 |
|---|---|---|
| soul のメモリ一覧 | `ene-ctl memory list` | `companions.db` の行 |
| 内容 / スコープの編集 | `PATCH /api/v1/memories/{id}` | ジャーナル付き更新 |
| メモリを忘れる | `ene-ctl memory delete` | forgotten フラグ。操作はジャーナルに残る |
| 保留候補の解決 | pending / resolve エンドポイント | 行になる前に書き込み・却下・編集 |

抽出と想起の仕組みは [メモリ](../concepts/memory.md) です。
