# Built-in tools

Every built-in tool is a plugin binary under `plugins/tool/`. Enable or
disable them with `tools.list.<name>.enable` (or `plugins.list.<name>`).
Namespaces and actions below match what `/tool list` shows.

| Plugin | Namespace actions | Notes |
|---|---|---|
| `app` | `app.list_windows`, `app.focus_window`, `app.get_active_window`, `app.list_monitors`, `app.capture_window`, `app.type_text`, `app.press_key`, `app.key_combo`, `app.mouse_move`, `app.mouse_click`, `app.mouse_drag`, `app.mouse_scroll` | Desktop control; Wayland uses XDG portals where needed |
| `browser` | `browser.navigate`, `browser.click`, `browser.type`, `browser.wait`, `browser.screenshot`, `browser.get_content`, `browser.scroll`, `browser.close` | Drives Chrome via CDP; session state kept in the plugin |
| `calc` | `calc.evaluate`, `calc.unit_convert`, `calc.currency_convert`, `calc.color_convert` | Currency needs exchange-rate config |
| `calendar` | `calendar.list_calendars`, `calendar.add_calendar`, `calendar.set_permission`, `calendar.remove_account`, `calendar.list_events`, `calendar.create_event`, `calendar.update_event`, `calendar.cancel_event`, `calendar.find_free_slots` | Stateful, approval-gated, multiple accounts (see [Connectors](../../concepts/connectors.md)) |
| `counter` | `counter.get`, `counter.increment`, `counter.reset` | Reference stateful tool (DB IPC via `ene-plugin-db`) |
| `fs` | `fs.read`, `fs.write`, `fs.edit`, `fs.delete`, `fs.glob`, `fs.grep`, `fs.regex_test`, `fs.patch`, `fs.shell`, `fs.undo` | Path-sandboxed; destructive actions permission-gated; shell is separate |
| `geo` | `geo.location`, `geo.weather`, `geo.timezone`, `geo.sun` | Needs provider configuration for location/weather |
| `git` | `git.status`, `git.diff`, `git.log`, `git.branch`, `git.remote`, `git.blame` | Read-only by default |
| `homeassistant` | `homeassistant.state`, `homeassistant.turn_on`, `homeassistant.turn_off`, `homeassistant.set_temperature` | Config: `base_url`, `token` |
| `random` | `random.number`, `random.uuid`, `random.pick`, `random.color` | |
| `utility` | `utility.notify_send`, `utility.todo_*` (list/add/update/complete/delete), `utility.get_current_time`, `utility.get_system_info`, `utility.timer_start`, `utility.timer_stop`, `utility.question` | Notifications run as background tasks; questions ask the user inline |
| `web` | `web.webfetch`, `web.websearch` | Search providers: Brave, Exa, Tavily, DuckDuckGo, arXiv; keys under `tools.list.web` |

## Tool-specific configuration

```json
{
  "tools": {
    "list": {
      "web": {
        "enable": true,
        "brave_api_key": "",
        "exa_api_key": "",
        "tavily_api_key": ""
      },
      "homeassistant": {
        "enable": true,
        "base_url": "http://homeassistant.local:8123",
        "token": ""
      }
    }
  }
}
```

Each tool's config schema is registered at build time
(`define_tool_config!`), so `assets/schema/settings.schema.json` validates
the exact keys.

## Permissions

Destructive or external-state actions (`fs.write`, `fs.delete`,
`fs.shell`, `calendar.create_event`, `homeassistant.turn_on`, …) emit a
permission request before execution. You can inspect grants with
`/permissions list` and revoke them at any time. The audit log records
every decision.

## Calling tools from the CLI

```sh
ene tool call fs.read '{"path": "Cargo.toml"}'
ene tool call web.websearch '{"query": "rust async"}'
```

Tools can also be called from the REPL with `/tool call <name> <json>`.
