# Web Tools (`ene-tools-web`)

**Binary:** `ene-tools-web` | **Stateful:** No

Provides URL fetching and web search capabilities.

## Tools

### `webfetch`

Fetches content from a URL.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `url` | string | Yes | - | URL to fetch (must start with http:// or https://) |
| `format` | string | No | `"markdown"` | Output format: `"text"`, `"markdown"`, or `"html"` |
| `timeout` | integer | No | 30 | Timeout in seconds (max 120) |

**Behavior:**
- 5MB response size limit
- `text`/`markdown` formats convert HTML to Markdown
- `html` returns raw HTML
- HTTP URLs are automatically upgraded to HTTPS
- Default 30-second timeout

**Keywords:** fetch, url, web, download, html

**Category:** Browser

---

### `websearch`

Searches the web using configurable backends.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `query` | string | Yes | - | Search query |
| `backend` | string | No | `"duckduckgo"` | Search backend |
| `limit` | integer | No | 5 | Max results (1-10) |

**Backends:**

| Backend | Requires API Key |
|---------|:---:|
| `duckduckgo` | No |
| `arxiv` | No |
| `tavily` | `TAVILY_API_KEY` |
| `brave` | `BRAVE_API_KEY` |
| `exa` | `EXA_API_KEY` |

**Keywords:** search, web, google, internet, lookup

**Category:** WebSearch

## Configuration

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

The config schema is dynamically registered via `config_schema()`. API keys can also be set via environment variables.
