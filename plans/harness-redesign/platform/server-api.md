# サーバー API

> 実現する要件: **P-702**(HTTP API+WS)、**P-703**(生成 SDK)、
> P-707(同時接続の API 面)、P-711(後継のオートメーション接点)。

## 1. 設計の立場

- コアデーモンは **HTTP API + WebSocket** を公開する(確定済み決定)。
  REST 的な一覧・取得・操作は HTTP、リアルタイムのイベント・
  ストリームは WS。
- API は**クライアント種別に依存しない**(desktop 専用 API を作らない、
  P-705 の対等性)。desktop/CLI/Web/将来モバイルは同一 API を使う。
- OpenAPI 仕様を**単一の正**とし、Rust/TS の SDK を生成する(P-703)。

## 2. HTTP API

共通規則:

- パスは `/api/v1/...`。JSON(MessagePack は IPC 専用、API は JSON)。
- 認証: ローカルバインドを前提に**トークン認証**
  (起動時に生成したトークンを desktop が保持、Web は接続時に
  トークンを入力/QR 受領)。TLS は Web 外部接続時のみ。
- エラー: RFC 9457 風の problem+json(`type`/`title`/`status`/
  `error_class`)。`error_class` は内部のエラー分類と一致
  ([core/agent-loop.md §8](../core/agent-loop.md#8-ガードとエラー回復))。
- ページング: カーソル方式。一覧系すべて。
- 「送信」系は**冪等キー**(`Idempotency-Key`)を受け、
  複数クライアントの同時送信で重複しない。

エンドポイント群:

| 領域 | エンドポイント(例) | 内容 |
|---|---|---|
| soul | `GET /souls`, `GET /souls/{id}`, `PATCH /souls/{id}/body` | soul 一覧・詳細・body 結合(hot swap) |
| session | `GET /sessions`, `POST /sessions`, `GET /sessions/{id}`, `POST /sessions/{id}/fork`, `POST /sessions/{id}/export`, `PATCH /sessions/{id}` | セッション操作 |
| message | `POST /sessions/{id}/messages`, `GET /sessions/{id}/history`, `DELETE /sessions/{id}/queued/{entry_id}` | 発話と履歴・キュー取消。messages はモード指定(`prompt\|steer\|follow_up`、既定 `prompt`)で [../core/lane-api.md](../core/lane-api.md) の3コマンドに対応 |
| session-op | `POST /sessions/{id}/compact` | 手動 compaction([../core/lane-api.md §5](../core/lane-api.md#5-httpws-との対応))。`POST /sessions/{id}/resume` は後継(P-525) |
| turn | `POST /turns/{id}/cancel` | キャンセル |
| memory | `GET /souls/{id}/memories`(`scope` で絞り込み), `PATCH /memories/{id}`, `DELETE /memories/{id}` | 記憶の閲覧/編集/削除。`scope` は `private\|shared`([../companion/memory.md §4](../companion/memory.md)) |
| job | `GET /jobs`, `GET /jobs/{id}`, `POST /jobs/{id}/cancel` | タスクの一覧/詳細/キャンセル(API 上の名は job)。一覧は `companions.db` の `jobs` を読む([../tasks/jobs-and-schedules.md §2](../tasks/jobs-and-schedules.md#2-jobp-605)) |
| schedule | `GET /schedules`, `POST /schedules`, `PATCH /schedules/{id}`, `DELETE /schedules/{id}` | スケジュール CRUD |
| artifact | `GET /artifacts`, `GET /artifacts/{id}/content` | 成果物の一覧/取得/ダウンロード |
| tool | `GET /tools`, `POST /tools/{name}/test`(開発用) | ツール面 |
| plugin | `GET /plugins`, `POST /plugins/{id}/restart` | プラグイン状態/再起動 |
| approval | `GET /approvals`, `POST /approvals/{id}/respond` | 承認 plane の応答 |
| character | `GET /characters`, `POST /characters/import`, `GET /characters/{id}/export` | キャラ管理 |
| setting | `GET /settings`, `PATCH /settings`, `GET /settings/schema` | 設定 |
| audit | `GET /audit` | 監査ログの閲覧 |
| usage | `GET /usage` | LLM 使用量(トークン・コスト)の集計。soul/セッション/タスク/期間で絞る([../core/storage-model.md](../core/storage-model.md) §4 の台帳) |
| diag | `GET /diag/spans` | ローカルスパン計測の閲覧(開発用途、[../core/observability.md](../core/observability.md))。コンテンツは載らない |
| system | `GET /health`, `POST /backup`, `POST /restore` | ヘルス・保全 |

## 3. WebSocket(イベント購読)

接続規則:

- `WS /api/v1/events`(カーソル指定可)で接続。トークン認証。
- 1接続 = 1クライアント。クライアントは `client_id` を持ち、
  排他資源の調停に使う([clients.md §3](clients.md))。
- 切断時は最終 cursor を返し、再接続でカーソル指定により
  欠落イベントを再送(永続ログから投影、欠損しない)。

### 購読の深さ(D-11)

**接続時に `depth` を指定する。** サーバは指定された深さのイベントだけを
送る。深い情報を全クライアントに流してクライアント側で隠す方式は取らない。
表示漏れが一箇所のバグで起きるうえ、常時流す帯域も無駄になるため。

| depth | 送るもの | 使う面 |
|---|---|---|
| `surface` | 発話・音声・承認・質問・body・作業中の1行要約 | 表層UI(主画面) |
| `detail` | 上記 + 内面・thinking・ツール引数と生出しの参照・感情の内部状態・層間メッセージ | 詳細画面(desktop / Web)、CLI の `--verbose` / `ene debug` |

- 深さの判定はサーバ側の責務。`depth: surface` の接続に
  `inner.delta` は届かない。
- 同一クライアントが両方の面を持つ場合(desktop の主画面+詳細画面、
  Web のチャット+詳細)、
  **接続を2本張る**。1本で受けて振り分けると、上の保証が
  クライアント実装に移ってしまう。
- 詳細画面向けの接続は監査ログに残す(何が見られたかの記録)。

### イベント種別

ライブバスの**非永続**イベントと、永続イベントの通知を混ぜる。

| 種別 | depth | 内容 |
|---|---|---|
| `text.delta` | surface | 発話テキストのチャンク |
| `inner.delta` / `inner.message` | detail | 内面のチャンク/確定([companion/inner-channel.md](../companion/inner-channel.md)) |
| `thinking.delta` | detail | モデルの thinking |
| `tool.call` / `tool.progress` / `tool.result` | surface は要約、detail は引数と生出しの参照([tools/registry.md §5](../tools/registry.md#5-表示の投影p-613--d-11)) | ツールの可視化 |
| `approval.asked` / `approval.resolved` | surface | 承認の要求/解決 |
| `question.asked` | surface | ask-user([core/agent-loop.md §9](../core/agent-loop.md#9-人間協調面plan--ask-userp-511-p-512)) |
| `audio.chunk` | surface | TTS 音声(購読クライアントのみ。バイナリフレーム) |
| `voice.state` | surface | 音声状態機械の変化([body/voice.md §4](../body/voice.md#4-割り込み状態機械p-103)) |
| `body.*` | surface | 表情・動作([body/body-and-performance.md](../body/body-and-performance.md)) |
| `affect.state` | detail | 感情の内部状態(PAD)。表層へは body 経由でのみ出る(D-19) |
| `job.progress` / `job.completed` | detail | タスクの進捗。**表層への伝達はコンパニオンの発話**であってこのイベントではない(D-13) |
| `delegation.*` | detail | 層間メッセージ |
| `notify.hint` | surface | OS 通知の材料。出し手はクライアント([clients.md §3.1](clients.md#31-os-通知d-25))。コアは出さない(D-25) |
| `lifecycle.*` | detail | プラグイン/プロバイダの状態変化・警告 |
| `session.event` | 種別による | 永続イベントの通知(カーソル付き) |

- `audio.chunk` は**排他資源**(再生は1クライアントが主、
  [clients.md §3](clients.md))。他クライアントは購読だけ。

## 4. 音声のストリーミング(P-102 の API 面)

- **入力**: クライアントのマイクフレームを WS のバイナリフレームで
  送る。コア側で VAD/ASR に掛ける
  ([body/voice.md §1](../body/voice.md#1-所有と配置))。
- **出力**: `audio.chunk` で受け、クライアントが再生する。
- 音声フレームのコーデック: Opus を既定(帯域・遅延)。PCM はフォールバック。

## 5. SDK 生成(P-703)

- OpenAPI 仕様(`openapi.json`)をビルドで生成し、そこから
  **Rust SDK**(コア自身も使う)と **TS SDK**(Web)を生成。
- SDK は「型付きクライアント+イベント購読のヘルパ」を提供。
  イベントの型は session-log のイベント語彙と一致させる
  (永続イベントの型を共有)。
- API の変更は OpenAPI の差分として CI で検知(破壊的変更の監視)。

## 6. Embedded モード(後継の接点)

- opencode の Embedded に倣い、クライアントを**同プロセス**で動かす
  モードの接点を残す。v1.0 では desktop も別プロセスのクライアントと
  する(単純性)が、API 面が同一なので embedded 化は輸送の差し替えで済む。

## 7. 外部公開(P-706, P-711)

- Web の外部接続(同一 LAN・トンネル)は、コアが**ローカルバインド+
  トークン**を維持したまま、リバースプロキシ/トンネルを通じて行う。
  コア自体は直接のグローバル公開をしない(安全既定)。
- オートメーション用の API 駆動(P-711)は、同じエンドポイントを
  「自動化専用プロファイル」(発話の抑制・承認の自動寄り)で使う形で
  後継に実現。接点として、認証に `client_kind: automation` を予約する。

## 8. 障害モード

| 障害 | 挙動 |
|---|---|
| トークン不正 | 401。試行は監査ログに記録 |
| WS の切断 | cursor で再接続・再送。永続ログが正なので欠損しない |
| 同時送信の競合 | Idempotency-Key で重複排除。キーなしの送信は先着を採用 |
| 大量イベントの背圧 | WS の送信バッファ上限で、古いライブチャンクを間引き(永続通知は間引かない) |
| `depth: detail` の接続が滞留 | 詳細向けの送信が遅れても `surface` の配信を妨げない。深い情報の配信は best-effort |
| OpenAPI と実装の乖離 | CI の差分検出で失敗(生成 SDK が壊れるため) |

## 9. 設定キー

| キー | 説明 |
|---|---|
| `server.bind` | バインド(既定は `127.0.0.1` のランダムポート。desktop が実ポートを取得) |
| `server.auth.token_file` | トークンの保存先 |
| `server.ws.send_buffer` | WS 送信バッファ |

---

- 前: [process-model.md](process-model.md) / 次: [clients.md](clients.md)
