# セキュリティサンドボックス

サンドボックスシステムは、ツール操作を設定されたディレクトリセットに制限し、危険なシェルコマンドをブロックします。

## 設定の受け渡し

1. `settings.json` → `sandbox` セクションから `SandboxConfigData` を作成
2. `IpcRequest::Handshake` に含めて各ツールバイナリへ送信（sandbox と `tool_config` は Handshake に含まれる）
3. ツール側の `Sandbox` 型がすべてのアクセス制御を適用

Handshake / ツール起動時に `SandboxConfigData::sanitize()` が既定の危険コマンド blocklist を union し、サイズ／タイムアウトが `0` ならコード既定へ戻し、`enabled` かつ `allowed_directories` が空なら `["."]` を入れます。サンドボックス自体を強制オンにはしません。

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
    pub db_socket: Option<String>,            // ツールごと DB IPC ソケットへのパス
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
       ├── 許可ディレクトリに含まれるか確認 → 含まれなければ拒否
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

Undo はツールごと DB IPC サーバー (`db_socket`) 経由でアクセスされる SQLite データベースに保存されます。元のファイル内容は zlib 圧縮で保存されます。

## エラー型

サンドボックス違反は `ene-plugin-proto` から `ToolError::SandboxViolation { message }` を返します。これは全ツールクレートで共有される統一エラー型であり、境界マッピングは不要です。

```rust
pub enum ToolError {
    SandboxViolation { message: String },
    PermissionDenied { message: String },
    PermissionRequired { request_id: String, action: String, target: String, description: String },
    FileNotFound { path: String },
    FileTooLarge { path: String, size: u64, limit: u64 },
    CommandBlocked { command: String, reason: String },
    ShellTimeout { command: String, timeout_ms: u64 },
    ShellOutputTooLarge { size: u64, limit: u64 },
    // ... 他のバリアント (全一覧は SDK ガイドを参照)
}
```

削除などの破壊的な操作にユーザー承認が必要な場合、ツールは `request_id` 付きの `ToolError::PermissionRequired` を返し、`ToolProvider::approve_permission()` で承認できます。