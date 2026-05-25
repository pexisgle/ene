# セキュリティサンドボックス

サンドボックスシステムは、ツール操作を設定されたディレクトリセットに制限し、危険なシェルコマンドをブロックします。

## 設定の配信

1. `settings.json` → `sandbox` セクションから `SandboxConfigData` を作成
2. `IpcRequest::Initialize { sandbox, tool_config }` として各ツールバイナリに送信
3. ツール側の `Sandbox` 型がすべてのアクセス制御を適用

## SandboxConfigData

```rust
pub struct SandboxConfigData {
    pub enabled: bool,
    pub allowed_directories: Vec<String>,     // 読み取り許可パス
    pub writable_directories: Vec<String>,    // 書き込み許可パス
    pub blocked_commands: Vec<String>,        // ブロックコマンド正規表現
    pub max_read_bytes: usize,                // 50KB デフォルト
    pub max_write_bytes: usize,               // 1MB デフォルト
    pub shell_timeout_ms: u64,                // 120s デフォルト
    pub max_shell_output_bytes: usize,        // 50KB デフォルト
    pub max_shell_output_lines: usize,        // 2000 デフォルト
    pub undo_db_path: Option<String>,
}
```

## チェックフロー

```
ファイル/シェル操作リクエスト
  ↓
Sandbox 有効?
  ├── いいえ → 直接実行
  └── はい
       ├── パス正規化 (read/write のみ)
       ├── allowed_directories / writable_directories? → いいえ → 拒否
       ├── シェル: blocked_commands パターン一致? → はい → 拒否
       └── サイズ/出力制限付きで実行
```

## ブロックコマンドパターン

同梱されるデフォルトのブロックパターン:

| パターン | 対象 |
|---------|------|
| `rm\s+-rf\s+/` | ルートファイルシステム削除 |
| `dd\s+if=` | ディスク破壊 |
| `mkfs` | ファイルシステムフォーマット |
| `sudo\s+` | 権限昇格 |
| `:\s*\{\s*\|\s*&\s*;\s*\}` | フォークボム |

## Undo システム

`Sandbox` は全ファイル変更の Undo スタックを保持します:

| メソッド | 説明 |
|---------|------|
| `track_overwrite(path, content)` | 上書き前に元の内容を保存 |
| `track_creation(path)` | 作成を記録 (Undo で削除) |
| `track_deletion(path, content)` | 削除前に内容を保存 |
| `track_patch(entries)` | パッチの全変更を 1 つの Undo エントリにグループ化 |
| `undo_last()` | 最新の操作をロールバック |

Undo は SQLite データベースに zlib 圧縮で保存されます (`undodb_path`/`undo.db`)。
