# GUI Automation (`ene-tool-app`)

**Binary:** `ene-tool-app` | **Stateful:** No

OS-level desktop automation using enigo, xcap, arboard, and xdg-desktop-portal (Wayland).

## Tools

### Clipboard

| Tool | Parameters | Description |
|------|-----------|-------------|
| `app.clipboard_read` | — | Read clipboard contents |
| `app.clipboard_write` | `text`* | Write to clipboard |

### Window Management

| Tool | Parameters | Description |
|------|-----------|-------------|
| `app.list_windows` | — | List all open windows |
| `app.focus_window` | `window_title`* | Focus a window by title |
| `app.get_active_window` | — | Get active window info |
| `app.list_monitors` | — | List available monitors |
| `app.capture_window` | `window_title`*, `scale_percent?` | Capture specific window (base64) |

### Keyboard

| Tool | Parameters | Description |
|------|-----------|-------------|
| `app.type_text` | `text`* | Type text via keyboard |
| `app.press_key` | `key`* | Press a single key |
| `app.key_combo` | `key_combo`* (e.g. `ctrl+shift+s`) | Key combination |

### Mouse

| Tool | Parameters | Description |
|------|-----------|-------------|
| `app.mouse_move` | `x`*, `y`*, `relative?` | Move mouse cursor |
| `app.mouse_click` | `button?`, `count?` | Click (supports double-click) |
| `app.mouse_drag` | `x`*, `y`*, `x2`*, `y2`*, `button?` | Drag from (x,y) to (x2,y2) |
| `app.mouse_scroll` | `amount?`, `direction?` | Scroll mouse wheel |

### Screenshot

| Tool | Parameters | Description |
|------|-----------|-------------|
| `app.screenshot` | `scale_percent?` (default 50) | Full screen screenshot (base64) |

**Category:** App

## Platform Support

| Feature | X11 | Wayland |
|---------|:---:|:---:|
| Keyboard input | enigo | enigo |
| Mouse control | enigo | enigo |
| Screenshots | xcap | xdg-desktop-portal (ashpd) |
| Clipboard | arboard | arboard |
| Window listing | active-win-pos-rs | active-win-pos-rs |

Wayland screenshot/capture uses `xdg-desktop-portal` via ashpd with PipeWire for screencast.
