# GUI 自動化 (`ene-tool-app`)

**バイナリ:** `ene-tool-app` | **ステートフル:** いいえ

enigo、xcap、arboard、および xdg-desktop-portal (Wayland) を使用した OS レベルのデスクトップ自動化。

## ツール (Tools)

### クリップボード (Clipboard)

| ツール | パラメータ | 説明 |
|------|-----------|-------------|
| `app.clipboard_read` | — | クリップボードの内容を読み取り |
| `app.clipboard_write` | `text`* | クリップボードに書き込み |

### ウィンドウ管理 (Window Management)

| ツール | パラメータ | 説明 |
|------|-----------|-------------|
| `app.list_windows` | — | すべての開いているウィンドウを列挙 |
| `app.focus_window` | `window_title`* | タイトルでウィンドウをフォーカス |
| `app.get_active_window` | — | アクティブウィンドウの情報を取得 |
| `app.list_monitors` | — | 利用可能なモニターを一覧表示 |
| `app.capture_window` | `window_title`*, `scale_percent?` | 特定のウィンドウをキャプチャ (base64) |

### キーボード (Keyboard)

| ツール | パラメータ | 説明 |
|------|-----------|-------------|
| `app.type_text` | `text`* | キーボードでテキストを入力 |
| `app.press_key` | `key`* | 単一のキーを押下 |
| `app.key_combo` | `key_combo`* (例: `ctrl+shift+s`) | キーコンビネーションの実行 |

### マウス (Mouse)

| ツール | パラメータ | 説明 |
|------|-----------|-------------|
| `app.mouse_move` | `x`*, `y`*, `relative?` | マウスカーソルを移動 |
| `app.mouse_click` | `button?`, `count?` | クリック (ダブルクリック対応) |
| `app.mouse_drag` | `x`*, `y`*, `x2`*, `y2`*, `button?` | (x,y) から (x2,y2) へのドラッグ |
| `app.mouse_scroll` | `amount?`, `direction?` | マウスホイールスクロール |

### スクリーンショット (Screenshot)

| ツール | パラメータ | 説明 |
|------|-----------|-------------|
| `app.screenshot` | `scale_percent?` (デフォルト 50) | 全画面スクリーンショット (base64) |

**カテゴリ:** App

## プラットフォーム対応

| 機能 | X11 | Wayland |
|---------|:---:|:---:|
| キーボード入力 | enigo | enigo |
| マウス制御 | enigo | enigo |
| スクリーンショット | xcap | xdg-desktop-portal (ashpd) |
| クリップボード | arboard | arboard |
| ウィンドウ一覧 | active-win-pos-rs | active-win-pos-rs |

Wayland のスクリーンショット/キャプチャは、PipeWire によるスクリーンキャストと ashpd 経由の `xdg-desktop-portal` を使用します。
