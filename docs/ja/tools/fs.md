# ファイルシステムツール (`ene-tools-fs`)

**バイナリ:** `ene-tools-fs` | **ステートフル:** はい (Sandbox + UndoManager)

ファイルシステム操作、シェル実行、Undo を提供します。全ファイル操作はサンドボックス設定を尊重します。

## ツール

### `filesystem`

ファイル操作用の統合メガツール。アクションでディスパッチ。

| アクション | パラメータ | 説明 |
|----------|-----------|------|
| `read` | `filePath`*, `offset?`, `limit?` | ファイル読み取り (行範囲指定可)、50KB 制限 |
| `write` | `filePath`*, `content`* | ファイル書き込み/作成、1MB 制限、Undo バックアップ |
| `edit` | `filePath`*, `oldString`*, `newString`*, `replaceAll?` | テキスト置換 (9 戦略)、Undo 対応 |
| `delete` | `path`*, `recursive?` | ファイル/ディレクトリ削除 |
| `glob` | `pattern`*, `path?` | パターンファイル検索 |
| `grep` | `pattern`*, `path?`, `include?` | コンテンツベース正規表現検索 |
| `patch` | `patchText`* | Unified diff 適用、複数ファイルを 1 Undo に |

**キーワード:** file, read, write, edit, delete, search, glob, grep, patch, directory, replace

**カテゴリ:** Filesystem

---

### `shell`

セキュリティ制限付きシェルコマンド実行。

| パラメータ | 型 | 必須 | デフォルト | 説明 |
|-----------|------|------|---------|------|
| `command` | string | はい | - | 実行するシェルコマンド |
| `description` | string | はい | - | コマンドの説明 (5-10 単語) |
| `timeout` | integer | いいえ | 120000 | タイムアウト (ミリ秒) |
| `workdir` | string | いいえ | カレント | 作業ディレクトリ |

**セキュリティ:**
- 全コマンドは `blocked_commands` パターンと照合
- 出力は `max_shell_output_bytes` (50KB) と `max_shell_output_lines` (2000) で制限
- デフォルト 120 秒タイムアウト
- `cd &&` パターンの代わりに `workdir` パラメータを使用

**キーワード:** shell, command, execute, terminal, bash

**カテゴリ:** Shell

---

### `undo`

最後のファイル操作を取り消します。

| パラメータ | 型 |
|-----------|------|
| (なし) | - |

**動作:**
- write, edit, delete, patch 操作を取り消し
- 複数回呼び出して複数操作を順に取り消し可能
- シェル操作は取り消し不可
- SQLite ベースの Undo スタック (zlib 圧縮) を使用

**キーワード:** undo, revert, rollback

**カテゴリ:** Utility

## 編集戦略

`filesystem` の `edit` アクションは 9 つのマッチング戦略を順に試行:

1. `trimmed_boundary` — 境界の空白をトリム
2. `simple` — 完全一致
3. `whitespace_normalized` — 空白の違いを吸収
4. `escape_normalized` — エスケープシーケンスを正規化
5. `line_trimmed` — 各行の空白をトリム
6. `multi_occurrence` — 複数出現を処理
7. `indentation_flexible` — インデントの違いを無視
8. `context_aware` — 周辺コンテキストでマッチ
9. `block_anchor` — コードブロック検出でアンカリング

## サンドボックス統合

全ファイル操作は `Sandbox::check_readable()` または `Sandbox::check_writable()` を通過:

```
リクエスト → Sandbox 有効?
  ├── いいえ → 直接実行
  └── はい
       ├── パス正規化
       ├── ディレクトリ許可リスト確認 → 許可外なら拒否
       ├── シェル: blocked_commands パターン確認 → 一致なら拒否
       └── サイズ/出力制限付きで実行
```
