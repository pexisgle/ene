# GUI Automation (`ene-tools-app`)

**Binary:** `ene-tools-app` | **Stateful:** No

OS-level desktop automation using enigo, xcap, arboard, and xdg-desktop-portal (Wayland).

## Tool: `app`

Single mega-tool with action-based dispatch.

| Action | Parameters | Description |
|--------|-----------|-------------|
| `list_windows` | — | List all open windows |
| `focus_window` | `window_title`* | Focus a window by title |
| `get_active_window` | — | Get active window info |
| `list_monitors` | — | List available monitors |
| `type_text` | `text`* | Type text via keyboard |
| `press_key` | `key`* | Press a single key |
| `key_combo` | `combo_str`* (e.g. `ctrl+shift+s`) | Key combination |
| `mouse_move` | `x`*, `y`*, `relative?` | Move mouse cursor |
| `mouse_click` | `button?`, `count?` | Click (supports double-click) |
| `mouse_drag` | `x`*, `y`*, `x2`*, `y2`*, `button?` | Drag from (x,y) to (x2,y2) |
| `mouse_scroll` | `amount?`, `direction?` | Scroll mouse wheel |
| `screenshot` | `scale_percent?` (default 50) | Full screen screenshot (base64) |
| `capture_window` | `window_title`*, `scale_percent?` | Capture specific window |
| `clipboard_read` | — | Read clipboard contents |
| `clipboard_write` | `text`* | Write to clipboard |

**Keywords:** gui, automation, mouse, keyboard, clipboard, window, screenshot, screen

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
