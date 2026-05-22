# セキュリティサンドボックス

Sandbox 設定は `AiSandboxSettings` で管理され、IPC 経由で `ene-tools-fs` に配信される。

## 設定→IPC 伝達

1. `AiSandboxSettings` は `to_sandbox_config_data()` で `SandboxConfigData` に変換
2. `IpcRequest::Initialize { sandbox }` としてツールバイナリに送信
3. ツール側の `Sandbox` 型が受信、全ファイル操作に適用

## SandboxConfig（ene-tools-fs 内）

```rust
struct SandboxConfig {
    enabled: bool,
    allowed_directories: Vec<PathBuf>,     // 読み取り許可パス
    writable_directories: Vec<PathBuf>,    // 書き込み許可パス
    blocked_commands: Vec<String>,         // ブロックコマンド正規表現
    max_read_bytes: usize,                 // 50KB
    max_write_bytes: usize,                // 1MB
    shell_timeout_ms: u64,                 // 120秒
    max_shell_output_bytes: usize,         // 50KB
    max_shell_output_lines: usize,         // 2000行
}
```

## チェックフロー

```
コマンド実行リクエスト
  ↓
Sandbox 有効?
  ├── No → 直接実行
  └── Yes
       ├── パス正規化（canonicalize）
       ├── allowed_directories 内？ → No → 拒否
       ├── 書き込み操作？ → writable_directories 内？ → No → 拒否
       ├── ブロックコマンドパターン一致？ → Yes → 拒否
       └── リソース制限内？ → No → 拒否
```

## ブロックコマンドパターン

デフォルト設定でブロックされるパターン:

| パターン | 対象 |
|----------|------|
| `rm\s+-rf\s+/` | ルート削除 |
| `dd\s+if=` | ディスク破壊 |
| `mkfs` | フォーマット |
| `sudo\s+` | 権限昇格 |
| `:\s*\{\s*\|\s*&\s*;\s*\}` | フォークボム |

## Undo システム

`Sandbox` はファイル操作の Undo スタックを保持する:

| メソッド | 説明 |
|----------|------|
| `track_overwrite(path, content)` | 上書き前に元の内容を保存 |
| `track_creation(path)` | 新規作成を記録（Undo で削除） |
| `track_deletion(path, content)` | 削除前に内容を保存 |
| `track_patch(entries)` | パッチの全変更を1 Undo エントリに |
| `undo_last()` | 最新の操作をロールバック |
