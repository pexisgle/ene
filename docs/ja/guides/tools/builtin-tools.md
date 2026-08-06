# 同梱ツール

同梱ツールはすべて `plugins/tool/` 配下のプラグインバイナリです。
`tools.list.<name>.enable`（または `plugins.list.<name>`）でオン/オフします。
名前空間とアクションは `/tool list` の表示と一致します。

| プラグイン | 名前空間アクション | メモ |
|---|---|---|
| `app` | `app.list_windows`・`app.focus_window`・`app.get_active_window`・`app.list_monitors`・`app.capture_window`・`app.type_text`・`app.press_key`・`app.key_combo`・`app.mouse_move`・`app.mouse_click`・`app.mouse_drag`・`app.mouse_scroll` | デスクトップ操作。Wayland は必要な場所で XDG portal を使用 |
| `browser` | `browser.navigate`・`browser.click`・`browser.type`・`browser.wait`・`browser.screenshot`・`browser.get_content`・`browser.scroll`・`browser.close` | CDP で Chrome を操作。セッション状態はプラグイン内に保持 |
| `calc` | `calc.evaluate`・`calc.unit_convert`・`calc.currency_convert`・`calc.color_convert` | 通貨変換は為替レート設定が必要 |
| `calendar` | `calendar.list_calendars`・`calendar.add_calendar`・`calendar.set_permission`・`calendar.remove_account`・`calendar.list_events`・`calendar.create_event`・`calendar.update_event`・`calendar.cancel_event`・`calendar.find_free_slots` | 状態保持・承認ゲート付き・複数アカウント（[コネクタ](../../concepts/connectors.md)参照） |
| `counter` | `counter.get`・`counter.increment`・`counter.reset` | 状態保持ツールの参照実装（`ene-plugin-db` による DB IPC） |
| `fs` | `fs.read`・`fs.write`・`fs.edit`・`fs.delete`・`fs.glob`・`fs.grep`・`fs.regex_test`・`fs.patch`・`fs.shell`・`fs.undo` | パスサンドボックス。破壊的操作は権限ゲート、シェルは別アクション |
| `geo` | `geo.location`・`geo.weather`・`geo.timezone`・`geo.sun` | 位置/天気はプロバイダー設定が必要 |
| `git` | `git.status`・`git.diff`・`git.log`・`git.branch`・`git.remote`・`git.blame` | デフォルトで読み取り専用 |
| `homeassistant` | `homeassistant.state`・`homeassistant.turn_on`・`homeassistant.turn_off`・`homeassistant.set_temperature` | 設定: `base_url`・`token` |
| `random` | `random.number`・`random.uuid`・`random.pick`・`random.color` | |
| `utility` | `utility.notify_send`・`utility.todo_*`（list/add/update/complete/delete）・`utility.get_current_time`・`utility.get_system_info`・`utility.timer_start`・`utility.timer_stop`・`utility.question` | 通知はバックグラウンドタスク、質問はユーザーにインラインで尋ねる |
| `web` | `web.webfetch`・`web.websearch` | 検索プロバイダー: Brave・Exa・Tavily・DuckDuckGo・arXiv。キーは `tools.list.web` |

## ツール固有の設定

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

各ツールの設定スキーマはビルド時に登録されるため
（`define_tool_config!`）、`assets/schema/settings.schema.json` が正確な
キーを検証します。

## 権限

破壊的・外部状態を変えるアクション（`fs.write`・`fs.delete`・`fs.shell`・
`calendar.create_event`・`homeassistant.turn_on` など）は、実行前に権限要求を
発行します。付与は `/permissions list` で確認でき、いつでも失効できます。
監査ログがすべての判断を記録します。

## CLI からツールを呼ぶ

```sh
ene tool call fs.read '{"path": "Cargo.toml"}'
ene tool call web.websearch '{"query": "rust async"}'
```

REPL からも `/tool call <name> <json>` で呼べます。
