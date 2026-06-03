# Browser Automation (`ene-tools-browser`)

**Binary:** `ene-tools-browser` | **Stateful:** Yes (BrowserSessionStore)

Chromium browser automation via Chrome DevTools Protocol (CDP), powered by `chromiumoxide`.

## Tools

| Tool | Parameters | Description |
|------|-----------|-------------|
| `browser.navigate` | `url`* | Navigate to URL, returns title + readyState |
| `browser.click` | `selector`* (CSS) | Click an element |
| `browser.type` | `selector`* (CSS), `text`* | Type text into an element |
| `browser.wait` | `wait_ms?` (default 1000) | Wait for specified milliseconds |
| `browser.screenshot` | — | Full page screenshot (base64 PNG) |
| `browser.get_content` | `format?`, `extract?`, `trim?` | Extract DOM content |
| `browser.scroll` | `scroll_x?`, `scroll_y?` | Scroll the page |
| `browser.close` | — | Close the browser session |

### `browser.get_content` Options

| Parameter | Type | Default | Values |
|-----------|------|---------|--------|
| `format` | string | `"markdown"` | `"markdown"` or `"html"` |
| `extract` | string | `"body"` | `"body"`, `"main"`, or `"full"` |
| `trim` | boolean | `true` | `true` / `false` |

Content is truncated to 15,000 characters.

### Usage Notes

- **CSS selectors only** — XPath is not supported
- **Prefer `navigate` over click navigation** — more reliable for page transitions
- **Use `scroll` for infinite-scroll pages** — scrolls the document body
- **`close` cleans up** — always close browser sessions when done

**Category:** Browser

## Architecture

```
BrowserToolProvider
  ├── BrowserSessionStore (DashMap<session_id, BrowserSession>)
  └── Chromium browser instance (chromiumoxide CDP client)
```

Each session maintains its own Chromium tab context within the shared browser process.
