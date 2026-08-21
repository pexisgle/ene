# App tool platform matrix

`tool.app` detects the session at process start (`app.capabilities`) and only
advertises actions the OS can support. Screenshot prefers the XDG Desktop
Portal on Wayland; clipboard prefers a native backend (`arboard`) and uses
`wl-copy` / `xclip` only as an explicit fallback.

## Support matrix

| Action | X11 | GNOME/KDE Wayland | Hyprland / Sway | native Windows |
|---|---|---|---|---|
| `app.capabilities` | yes | yes | yes | yes |
| `app.screenshot` | CLI (`import` / grim), portal when a session bus exists | portal first, grim CLI fallback | portal first, grim CLI fallback | GDI primary monitor |
| `app.list_monitors` | `xrandr` | PNG size / portal logical size | `hyprctl` / `swaymsg` | GDI primary |
| `app.window_list` | `wmctrl` | unsupported (no stable protocol) | `hyprctl` / `swaymsg` | Win32 (primary session) |
| `app.active_window` | `xdotool` | unsupported | `hyprctl` | Win32 |
| `app.clipboard_*` | `arboard`, then `xclip` | `arboard`, then `wl-paste` / `wl-copy` | same | `arboard` |
| `app.click` / `type` / `key` | `xdotool` when present | **not advertised** | **not advertised** | advertised (SendInput path is X11-equivalent host input; Wayland-class injection is not claimed) |

Portal permission states are structured JSON errors with `code`:

| `code` | Meaning |
|---|---|
| `waiting` | portal prompt did not finish before the wait budget |
| `denied` | portal / OS refused capture |
| `cancelled` | user dismissed the portal prompt |
| `unsupported` | no portal, no compositor protocol, or input is not advertised |
| `unavailable` | backend missing (no grim, no session bus, empty clipboard) |

Capture JSON includes `width`, `height`, `scale`, `format`, `backend`,
`permission`, and the matching `monitor` row when `list_monitors` agrees on
pixel size.

## Manual checks

1. `ene-ctl tool` / plugin test: `app.capabilities` lists `session` and
   per-action `available` / `reason`.
2. Wayland: trigger `app.screenshot`, accept the portal; confirm `backend` is
   `portal` and `width`/`height` match `app.list_monitors`.
3. Dismiss the portal; the tool error JSON has `"code":"cancelled"`.
4. Clipboard: `app.clipboard_set` then `app.clipboard_get` without `wl-copy`
   installed still works when `arboard` can open the seat (`fallback: false`).
5. GNOME Wayland: `app.click` is absent from the plugin spec list.
6. Windows: `app.screenshot` returns GDI PNG for the primary monitor; clipboard
   uses `arboard` only.
