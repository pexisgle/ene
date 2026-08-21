# App ツールのプラットフォーム表

`tool.app` は起動時にセッションを検出し（`app.capabilities`）、OS が支えられる
action だけを公開します。Wayland の screenshot は XDG Desktop Portal を優先し、
clipboard は native backend（`arboard`）を優先、`wl-copy` / `xclip` は明示的な
フォールバックです。

## 対応表

| Action | X11 | GNOME/KDE Wayland | Hyprland / Sway | native Windows |
|---|---|---|---|---|
| `app.capabilities` | 可 | 可 | 可 | 可 |
| `app.screenshot` | CLI（`import` / grim）。セッションバスがあれば portal | portal 優先、grim は CLI フォールバック | 同左 | プライマリモニタの GDI |
| `app.list_monitors` | `xrandr` | PNG サイズ / portal の論理サイズ | `hyprctl` / `swaymsg` | GDI プライマリ |
| `app.window_list` | `wmctrl` | 非対応（安定プロトコルなし） | `hyprctl` / `swaymsg` | Win32 |
| `app.active_window` | `xdotool` | 非対応 | `hyprctl` | Win32 |
| `app.clipboard_*` | `arboard`、だめなら `xclip` | `arboard`、だめなら `wl-paste` / `wl-copy` | 同左 | `arboard` |
| `app.click` / `type` / `key` | `xdotool` があるとき | **公開しない** | **公開しない** | 公開する（Wayland 級の注入は主張しない） |

portal の許可状態は `code` 付きの JSON エラーです。

| `code` | 意味 |
|---|---|
| `waiting` | 待ち時間内に portal が終わらなかった |
| `denied` | portal / OS が拒否した |
| `cancelled` | ユーザーが portal を閉じた |
| `unsupported` | portal も compositor プロトコルもなく、入力も公開していない |
| `unavailable` | backend 不在（grim なし、バスなし、clipboard 空） |

capture の JSON は `width`、`height`、`scale`、`format`、`backend`、
`permission` と、画素サイズが一致する `monitor` 行を含みます。

## 手動確認

1. `app.capabilities` で `session` と action ごとの `available` / `reason` を見る。
2. Wayland で `app.screenshot` の portal を許可し、`backend` が `portal`、
   サイズが `app.list_monitors` と一致することを確認する。
3. portal を閉じるとエラー JSON の `"code"` は `cancelled`。
4. `wl-copy` なしでも `arboard` が開けば clipboard は `fallback: false`。
5. GNOME Wayland では spec に `app.click` が出ない。
6. Windows ではプライマリモニタの GDI PNG と `arboard` の clipboard。
