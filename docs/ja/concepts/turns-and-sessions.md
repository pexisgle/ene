# ターンとセッション

## ターン

**ターン**は会話の 1 単位です。ユーザーメッセージ（またはプロアクティブ・
スケジュール・委譲によるトリガー）と、それに対するデーモンの一連の応答を
指します。すべてのターンは `TurnId` を持ちます。対話レーンは `ene-kernel`
です。

クライアントは `POST /api/v1/sessions/{id}/messages`（`prompt` / `steer` /
`follow_up`）でターンを開始します。カーネルは `TurnId` を返し、ライブバスに
ストリームします。実行中ターンがあるときの第二の `prompt` は `lane_busy` で
失敗します。`steer` は生成を切らずに訂正をキューし、`abort` は実行中ターンを
中断し、`compact` は履歴を圧縮します。

### ターンの起点（origin）

| 起点 | トリガー |
|---|---|
| `user` | クライアントからのメッセージ |
| `proactive` | コンパニオンが自発的に話す（`ene-companion`） |
| `scheduled` | 永続スケジュールが発火（`ene-work`） |
| `delegation` | 裏層ジョブからの報告 |
| `subagent` | 入れ子のワークターン |

### ターン内部で起きること

1. `ene-companion` で想起と感情のティック。
2. カーネルがセッションログからモデル可視プロンプトを構成。
3. 会話モデルがテキストをストリーム（プロバイダープラグインが来るまで Echo）。
4. 表層向けツールは `ene-registry` / `ene-plane` を通る。
5. イベントは `ene-session` にコミット（モデル可視 = ログ）。
6. ライブイベントは `surface` または `detail` の深さで送出。

シグネチャは `ene-kernel` と `ene-session` の rustdoc にあります。

## イベント

デーモンは HTTP と WebSocket のライブバスを出します。
`ene-kernel::LiveEvent` はサーバ側で深さフィルタされます。`surface` は発話、
`detail` は内面 / thinking / ツール引数も受け取ります。stage の主画面は
surface、別窓の詳細画面（と `ene-ctl --verbose`）は detail です。

会話履歴はクライアント側バッファではなく、`sessions.db` の追記専用ログです。

## セッション

**セッション**は 1 つの soul との連続した会話で、`SessionId` で識別されます。
`ene-ctl` は HTTP API に対して一覧・表示・作成・fork・エクスポート・compact・
検索・分割・終了ができます。

アイドル終了とトピック境界での分割はサーバ側です。compaction は要約をログに
書き、以降のターンが予算内に収まるようにします。
