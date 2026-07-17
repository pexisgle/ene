# ブラウザ自動化 (`ene-tool-browser`)

**バイナリ:** `ene-tool-browser` | **ステートフル:** はい (BrowserSessionStore)

Chrome DevTools Protocol (CDP) を使用した Chromium ブラウザ自動化。`chromiumoxide` を利用。

## ツール: `browser`

アクションベースのディスパッチを行う単一メガツール。ブラウザ状態はセッション単位で保持されます。

| アクション | パラメータ | 説明 |
|----------|-----------|------|
| `navigate` | `url`* | URL に遷移、title + readyState を返す |
| `click` | `selector`* (CSS) | 要素をクリック |
| `type` | `selector`* (CSS), `text`* | 要素にテキスト入力 |
| `wait` | `wait_ms?` (デフォルト 1000) | 指定ミリ秒待機 |
| `screenshot` | — | ページ全体のスクリーンショット (base64 PNG) |
| `get_content` | `format?`, `extract?`, `trim?` | DOM コンテンツを抽出 |
| `scroll` | `scroll_x?`, `scroll_y?` | ページをスクロール |
| `close` | — | ブラウザセッションを終了 |

### `get_content` オプション

| パラメータ | 型 | デフォルト | 値 |
|-----------|------|---------|-----|
| `format` | string | `"markdown"` | `"markdown"` または `"html"` |
| `extract` | string | `"body"` | `"body"`, `"main"`, または `"full"` |
| `trim` | boolean | `true` | `true` / `false` |

コンテンツは 15,000 文字に切り詰められます。

### 使用上の注意

- **CSS セレクタのみ** — XPath は非対応
- **クリックによるナビゲーションより `navigate` を推奨** — ページ遷移の信頼性が高い
- **無限スクロールページでは `scroll` を使用** — ドキュメントボディをスクロール
- **`close` でクリーンアップ** — ブラウザセッションは使い終わったら必ず閉じる

**キーワード:** browser, web, navigate, click, chrome, scrape

**カテゴリ:** Browser

## アーキテクチャ

```
BrowserToolProvider
  ├── BrowserSessionStore (DashMap<session_id, BrowserSession>)
  └── Chromium ブラウザインスタンス (chromiumoxide CDP クライアント)
```

各セッションは共有ブラウザプロセス内で独自の Chromium タブコンテキストを保持します。
