# ツールプロバイダ一覧

各バイナリは `ToolProvider` trait を実装し、`run_tool_server()` で UDS サーバーを起動する。

## ene-tools-fs（FsToolProvider）

ファイルシステム操作、シェル実行、Undo。ステートフル（Sandbox + UndoManager）。

| ツール | アクション / 引数 | 説明 |
|--------|------------------|------|
| `filesystem` | `action: read` + `filePath`, `offset`, `limit` | ファイル読み取り（行範囲指定可）、50KB制限 |
| | `action: write` + `filePath`, `content` | ファイル書き込み、1MB制限、Undoバックアップ |
| | `action: edit` + `filePath`, `oldString`, `newString`, `replaceAll` | テキスト置換（複数戦略でロバストマッチ）、Undo対応 |
| | `action: delete` + `path`, `recursive` | ファイル/ディレクトリ削除 |
| | `action: glob` + `pattern`, `path` | パターンファイル検索、サンドボックス制限 |
| | `action: grep` + `pattern`, `path`, `include` | ファイル内容検索 |
| | `action: patch` + `patchText` | Unified diff 適用、複数ファイル操作を1 Undo に |
| `shell` | `command`, `description`, `timeout`, `workdir` | シェルコマンド実行、ブロックチェック、120秒タイムアウト |
| `undo` | なし | 直前のファイル操作を取り消し（UndoStack） |

## ene-tools-web（WebToolProvider）

Web 検索・フェッチ。ステートレス。

| ツール | 引数 | 説明 |
|--------|------|------|
| `webfetch` | `url`, `format`（text/markdown/html）, `timeout` | URL コンテンツ取得、5MB制限、HTML→Markdown変換 |
| `websearch` | `query`, `backend`（duckduckgo/tavily/brave）, `limit` | Web 検索、結果タイトル＋スニペット＋URL |

## ene-tools-utility（UtilityToolProvider）

ユーティリティ。ステートフル（TodoStore）。

| ツール | 引数 | 説明 |
|--------|------|------|
| `question` | `questions: Vec<String>` | ユーザーへの質問（複数可） |
| `todo` | `todos: Vec<{content, status, priority}>` | セッションごとのタスクリスト管理 |
| `get_current_time` | なし | 現在時刻（YYYY-MM-DD HH:MM:SS） |
| `get_system_info` | なし | OS + アーキテクチャ |

## ene-tools-app（AppToolProvider）

OS レベル GUI 自動化（enigo, xcap, xdg-desktop-portal）。ステートレス。

| アクション | 引数 | 説明 |
|-----------|------|------|
| `list_windows` | なし | 全ウィンドウ列挙 |
| `focus_window` | `window_title` | ウィンドウフォーカス |
| `get_active_window` | なし | アクティブウィンドウ情報 |
| `list_monitors` | なし | モニター一覧 |
| `type_text` | `text` | キーボード入力 |
| `press_key` | `key` | 単一キー押下 |
| `key_combo` | `combo_str`（例: ctrl+shift+s） | キーコンビネーション |
| `mouse_move` | `x`, `y`, `relative` | マウス移動 |
| `mouse_click` | `button`, `count` | マウスクリック（ダブルクリック対応） |
| `mouse_drag` | `x`, `y`, `x2`, `y2`, `button` | マウスドラッグ |
| `mouse_scroll` | `amount`, `direction` | スクロール |
| `screenshot` | `scale_percent` | スクリーンショット（base64） |
| `capture_window` | `window_title`, `scale_percent` | 特定ウィンドウのキャプチャ |
| `clipboard_read` | なし | クリップボード読み取り |
| `clipboard_write` | `text` | クリップボード書き込み |

## ene-tools-browser（BrowserToolProvider）

Chromium ブラウザ自動化（CDP、chromiumoxide）。ステートフル（BrowserSessionStore）。

| アクション | 引数 | 説明 |
|-----------|------|------|
| `navigate` | `url` | URL 遷移、タイトル＋readyState 返却 |
| `click` | `selector`（CSS） | 要素クリック |
| `type` | `selector`, `text` | 要素にテキスト入力 |
| `wait` | `wait_ms`（デフォルト1000） | 待機 |
| `screenshot` | なし | ページ全体のスクリーンショット（base64） |
| `get_content` | `format`, `extract`, `trim` | DOM コンテンツ抽出（Markdown/HTML）、15000文字制限 |
| `scroll` | `scroll_x`, `scroll_y` | ページスクロール |
| `close` | なし | ブラウザセッション終了 |

## ene-tool-proto（ToolProvider trait）

```rust
#[async_trait]
pub trait ToolProvider: Send + Sync {
    fn list_tools(&self) -> Vec<ToolDefinition>;
    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError>;
    fn set_session_id(&self, session_id: &str);
    fn set_sandbox(&self, _sandbox: &SandboxConfigData) {}  // デフォルト no-op
}
```

HostRegistry は複数の ToolProvider を束ね、ツール名でディスパッチするコンポジット実装。
