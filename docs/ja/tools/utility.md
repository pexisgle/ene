# ユーティリティツール (`ene-tools-utility`)

**バイナリ:** `ene-tools-utility` | **ステートフル:** はい (TodoStore)

ユーザーとの対話やタスク管理のためのヘルパーツールを提供します。

## ツール

### `question`

ユーザーに 1 つ以上の質問を投げかけます。

| パラメータ | 型 | 必須 | 説明 |
|-----------|------|------|------|
| `questions` | string[] | はい | ユーザーへの質問リスト |

**使用場面:** 要件が不明確な場合、コンテキストが不足している場合、ユーザーの確認が必要な場合。

**キーワード:** question, ask, clarify, confirm

**カテゴリ:** Utility

---

### `todo`

セッション単位のタスクリストを管理します。

| パラメータ | 型 | 必須 | 説明 |
|-----------|------|------|------|
| `todos` | object[] | はい | 更新後の完全な TODO リスト |

各 TODO 項目:

| フィールド | 型 | 必須 | 値 |
|-----------|------|------|-----|
| `content` | string | はい | タスク説明 |
| `status` | string | いいえ | `pending`, `in_progress`, `completed`, `cancelled` |
| `priority` | string | いいえ | `high`, `medium`, `low` |

**状態:** `TodoStore` (DashMap ベースのインメモリ) によるセッション単位の永続化。ツールバイナリの再起動でクリアされます。

**キーワード:** todo, task, track, plan

**カテゴリ:** Utility

---

### `get_current_time`

現在のシステム日時を返します。

| パラメータ | 型 |
|-----------|------|
| (なし) | - |

**出力形式:** `2026-05-26 14:30:00`

**キーワード:** time, date

**カテゴリ:** Utility

---

### `get_system_info`

OS とアーキテクチャの基本情報を返します。

| パラメータ | 型 |
|-----------|------|
| (なし) | - |

**出力形式:** `OS: linux, Architecture: x86_64`

**キーワード:** system, os, platform

**カテゴリ:** Utility
