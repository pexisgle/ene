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
| `proactive` | コンパニオンが自発的に話す（`ene-companion`）。開いているセッションをすべて `mind.proactive.observation_interval_seconds`（既定 60）で観測します。 |
| `scheduled` | 永続スケジュールが発火（`ene-work`） |
| `delegation` | 裏層ジョブからの報告 |
| `subagent` | 入れ子のワークターン |

### ターン内部で起きること

1. `ene-companion` で想起と感情のティック。
2. カーネルがセッションログからモデル可視プロンプトを構成。
   インストール済み skill は `skills.catalog` として System Context に載る。
   依頼に合う `SKILL.md` 本文は `skills.active` として注入される。
   一致した `ene.emotion_note` は affect 分類の入力に前置され、発動ブロック
   には `Tone:` 行として載る。有効 skill の `ene.proactive_hint` は能動発話
   判定の `user_instructions` に渡る。
   `PATCH /api/v1/souls/{id}/skills`（または `ene-ctl soul skills`）が soul
   の許可リストを更新する。空リストは導入済み skill すべてが対象、という意味。
3. 設定済みの会話モデルが、結び付けた `provider.*` プラグイン経由でテキストを
   ストリーム。
4. 表層向けツールは `ene-registry` / `ene-plane` を通る。
   `delegate.start` はすぐに戻り、`ene-work` が **ジョブレーン**
   （`origin: delegation`）を開き、作業ツールと `delegation.send` を使います。
   しおり依頼（`workflow.bookmark`）は `web.search` があれば調査し、
   Markdown を書いてジョブ成果物として交付する。
   `delegation.send kind=complete`（およびジョブランナーの暗黙 complete）は
   登録済み成果物を `<data>/workspace/jobs/<soul_id>/artifacts/` へコピーし、
   `GET /api/v1/artifacts` の `delivered` を立てます。失敗・キャンセルした
   ジョブの成果物は未交付のままです。設計書の `<data>/workspaces/<soul_id>/`
   配置は使っていません。実際の根は単数の `workspace` で、ジョブ作業
   ディレクトリは `jobs/<soul_id>/<job_id>/` です。
5. イベントは `ene-session` にコミット（モデル可視 = ログ）。
6. ライブイベントは `surface` または `detail` の深さで送出。

生成の前に、共有 `LoopHooks` の waterfall として `agent/pre-step` が走ります。
ホストと `ene-fiber` はガード付きで購読し、ファイバー unload で外れます。
`next` を呼ばないリスナーはターンを書き換え／停止できます。`emit` は通知のみです。
アウトプロセスプラグインに生の intercept IPC は渡しません。承認や quiet hours を
ツール副プロトコルから迂回できてしまうためです。

### システムコンテキスト

各ターン、`ene-kernel::ContextRegistry` が固定順で System Context を組み、
行ごとに `context/system_message`（`source_key` が安定名）としてログします。
対話レーンは `platform_contract` とフォールバックの `identity_kernel` を持ち、
残りはそのターンで `ene-core` が載せます:

| キー | 中身 |
|---|---|
| `platform_contract` | 出力契約と安全規則 |
| `identity_kernel` | キャラクターパッケージのペルソナ（あれば） |
| `character_state` | 感情の語彙（PAD 数値は出さない） |
| `memory.semantic` | この発話に対するランク付き想起 |
| `memory.user_profile` | 常設のプロフィール / 好み |
| `memory.commitments` | 期限切れでない約束 |
| `skills.catalog` | 導入済みスキルの名前と説明（soul の許可リストで絞る） |
| `skills.active` | この発話に合う `SKILL.md` 本文 |
| `mcp.resources` | `<workspace>/mcp-context/` のスナップショット |
| `inner_recent` | 直近のモデル可視な内面 |
| `interruption_note` | 中断の直後だけ |
| `delegation.active` | created / queued / running の公開ジョブ |

空のロードはキーごと省きます。ロード失敗時は直前の有効なペルソナを残します。
セッションログを再生すると、同じモデル可視面が再構築できます。

シグネチャは `ene-kernel` と `ene-session` の rustdoc にあります。

## イベント

デーモンは HTTP と WebSocket のライブバスを出します。
`ene-kernel::LiveEvent` はサーバ側で深さフィルタされます。`surface` は発話、
`detail` は内面 / thinking / ツール引数も受け取ります。stage の主画面は
surface、別窓の詳細画面（と `ene-ctl --verbose`）は detail です。

会話履歴はクライアント側バッファではなく、`sessions.db` の追記専用ログです。
プロバイダ失敗はターンを `failed` で終え、アシスタント発話としては書きません。
履歴はその失敗を `status` メッセージとして出すので、再接続後もエラーが残ります。

## セッション

**セッション**は 1 つの soul との連続した会話で、`SessionId` で識別されます。
`ene-ctl` は HTTP API に対して一覧・表示・作成・fork・エクスポート・compact・
検索・分割・終了ができます。

アイドル終了と明示的な分割はサーバ側です。compaction は要約をログに
書き、以降のターンが予算内に収まるようにします。
