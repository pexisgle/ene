# Implementation Policy

> product convergence を横断する実装方針。

## crate 責務
- ene-session: 会話ログ所有。ene-kernel: lane 非依存。ene-companion: daemon 非依存。ene-work: plane ゲート。ene-plane: 承認/監査。ene-fiber: 監督。

## 型/状態
- typed state/error/ID、normal table と event log 分離、REST 正規投影/WS 差分。

## runner
- scope/cancel/verification、stale-safe computer action、Task Grant/hard confirmation。

## Attention
- priority/action_required/dedupe/expiry、speech/card/notification/delivery、計測。

## UI/安全
- stage copy/a11y、privacy-safe spans、raw path/secret/thinking を surface へ出さない。

## DoD
- 新stateは transition test、新side effect は scope/deny/cancel test、API/doc/i18n は同変更で同期、Echo/Scripted だけで閉じない。

