# Web ツール (`ene-tool-web`)

**バイナリ:** `ene-tool-web` | **ステートフル:** いいえ

URL からのコンテンツ取得と Web 検索機能を提供します。

## ツール

### `web.fetch`

URL からコンテンツを取得します。

| パラメータ | 型 | 必須 | デフォルト | 説明 |
|-----------|------|------|---------|------|
| `url` | string | はい | - | 取得する URL (http:// または https:// で始まる必要あり) |
| `format` | string | いいえ | `"markdown"` | 出力形式: `"text"`, `"markdown"`, `"html"` |
| `timeout` | integer | いいえ | 30 | タイムアウト (秒、最大 120) |

**動作:**
- 5MB レスポンスサイズ制限
- `text`/`markdown` 形式は HTML→Markdown 変換
- `html` は生の HTML を返す
- HTTP URL は自動的に HTTPS にアップグレード
- デフォルト 30 秒タイムアウト

**キーワード:** fetch, url, web, download, html

**カテゴリ:** WebFetch

---

### `web.search`

設定可能なバックエンドで Web 検索を実行します。

| パラメータ | 型 | 必須 | デフォルト | 説明 |
|-----------|------|------|---------|------|
| `query` | string | はい | - | 検索クエリ |
| `backend` | string | いいえ | `"duckduckgo"` | 検索バックエンド |
| `limit` | integer | いいえ | 5 | 最大結果数 (1-10) |

**バックエンド:**

| バックエンド | API キー必要 |
|------------|:---:|
| `duckduckgo` | いいえ |
| `arxiv` | いいえ |
| `tavily` | `TAVILY_API_KEY` |
| `brave` | `BRAVE_API_KEY` |
| `exa` | `EXA_API_KEY` |

**キーワード:** search, web, google, internet, lookup

**カテゴリ:** WebSearch

## 設定

```json
{
  "tools": {
    "tools": {
      "web": {
        "enable": true,
        "config": {
          "tavily_api_key": "",
          "brave_api_key": "",
          "exa_api_key": ""
        }
      }
    }
  }
}
```

設定スキーマは `config_schema()` で動的に登録されます。API キーは環境変数でも設定できます。
