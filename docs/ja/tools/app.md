# GUI 自動化 (`ene-tool-app`)

**バイナリ:** `ene-tool-app` | **ステートフル:** いいえ

enigo, xcap, arboard, xdg-desktop-portal (Wayland) を使用した OS レベルのデスクトップ自動化。

## ツール: `app`

アクションベースのディスパッチを行う単一メガツール。

| アクション | パラメータ | 説明 |
|----------|-----------|------|
| `list_windows` | — | 全ウィンドウを列挙 |
| `focus_window` | `window_title`* | タイトルでウィンドウをフォーカス |
| `get_active_window` | — | アクティブウィンドウ情報を取得 |
| `list_monitors` | — | 利用可能なモニターを一覧 |
| `type_text` | `text`* | キーボードでテキスト入力 |
| `press_key` | `key`* | 単一キーを押下 |
| `key_combo` | `combo_str`* (例: `ctrl+shift+s`) | キーコンビネーション |
| `mouse_move` | `x`*, `y`*, `relative?` | マウスカーソルを移動 |
| `mouse_click` | `button?`, `count?` | クリック (ダブルクリック対応) |
| `mouse_drag` | `x`*, `y`*, `x2`*, `y2`*, `button?` | (x,y) から (x2,y2) へドラッグ |
| `mouse_scroll` | `amount?`, `direction?` | マウスホイールスクロール |
| `screenshot` | `scale_percent?` (デフォルト 50) | 全画面スクリーンショット (base64) |
| `capture_window` | `window_title`*, `scale_percent?` | 特定ウィンドウをキャプチャ |
| `clipboard_read` | — | クリップボードの内容を読み取り |
| `clipboard_write` | `text`* | クリップボードに書き込み |

**キーワード:** gui, automation, mouse, keyboard, clipboard, window, screenshot, screen

**カテゴリ:** App

## プラットフォーム対応

| 機能 | X11 | Wayland |
|------|:---:|:---:|
| キーボード入力 | enigo | enigo |
| マウス制御 | enigo | enigo |
| スクリーンショット | xcap | xdg-desktop-portal (ashpd) |
| クリップボード | arboard | arboard |
| ウィンドウ一覧 | active-win-pos-rs | active-win-pos-rs |

Wayland のスクリーンショット/キャプチャは、PipeWire によるスクリーンキャストと ashpd 経由の `xdg-desktop-portal` を使用します。
