# 残り3件 実装計画 (2026-08-30)

## 対象
- #1250 bug(stage): セッション書き出しのヒントが「セッション ID をファイル名にします」のまま (area:desktop, priority:low, scope:post-v1)
- #1251 bug(stage): core の status 行が Companion transcript に描画されない (area:desktop, priority:medium, scope:v1.0) — v1.0 ブロッカー
- #1198 feat(work): Task Contractと検証付きtask runnerへ移行する (無ラベル, #1178 関連) — 6完了条件を持つ中核機能

## 現状分析

### #1250 詳細
- 実ファイル名: `session_export_filename_with_timestamp` が `YYYY-MM-DD_HHMM_<companion>_<session>.json` を生成 (detail/mod.rs:4244)
  例: `2026-08-29_1009_Alicia_01a04c7f-....json`
- ヒント文が古い:
  - en-US: "Saves session JSON. The dialog opens in Documents or Downloads with the session id as the file name."
  - ja: "セッション JSON を保存します。ダイアログはドキュメントまたはダウンロードを開き、セッション ID をファイル名にします。"
- 期待: ヒントが実際の命名 (日付 + コンパニオン名 + セッション) と一致
- 対象ファイル: `apps/ene-stage/i18n/en-US/stage.ftl:125`, `apps/ene-stage/i18n/ja/stage.ftl:125`
- 影響範囲: 文言のみ、コード変更なし

### #1251 詳細 (v1.0 ブロッカー)
- 再現: chat plugin が provider unavailable (`ai.tasks.chat`) のとき、core 履歴 (depth=surface) に `status model: provider unavailable: ai.tasks.chat` があるのに Companion 中央履歴に出ない。送信後の user 行は残るようになった (#1215) が status/error バブルが出ない。
- 投影: `crates/ene-session/src/project.rs:264` で `TurnEnd(Failed)` → `Role::Status` に投影 (`include_turn_failures: true` のとき)。history API は depth=surface で返す。
- 正規化: `apps/ene-stage/src/surface/chat.rs:60` で `"status" | "error" => TranscriptKind::Error` にマッピング → バブルは描画可能。
- 根本原因候補:
  1. `app.rs: RefreshHistory` の分岐 `has_assistant` 判定: assistant が無い failed ターンでは `surface.history` が更新されず stale のまま。status 行だけでは `has_assistant==false` なので `"pending_completion_refreshes>0 && !has_assistant"` で session のみに書いて surface を更新しない。
  2. `ReconcileHistory` も `progressed` を assistant 行だけで判定 (`m.role=="assistant"`) — status だけの failed ターンを完了とみなさない。
  3. `chat.rs` の normalize 自体は status を扱えるが、そもそも history が届かなければ表示されない。
- 対象: `apps/ene-stage/src/app.rs` (RefreshHistory / ReconcileHistory), 必要なら `chat.rs` は触らない
- 制約: 成功ターンでは従来の has_assistant ガードを壊さない。status を「進捗」とみなす条件を足す。

### #1198 詳細
- 目的: job/delegation を goal だけでなく success criteria / artifacts / constraints / 権限範囲を持つ Task Contract へ置換
- 実装範囲 (issue 本文):
  - Task Contract, Task state, mailbox revision
  - verifying を含む状態機械
  - success criteria evaluator
  - artifact registryとworkspace confinement
  - follow-up, question, answer, cancel
  - restart時のInterrupted確定
  - /jobs から /tasks へのAPI移行 (段階的に /jobs 互換を残すか要検討)
- 完了条件:
  - incomplete contractがrunnerへ入らない
  - modelのdoneだけでCompletedにならない
  - scope拡大follow-upが再承認される
  - workspace外artifactを拒否
  - cancel後に新side effectを開始しない
  - restart後にtaskが無言で消えない
- 参照: `plans/product-convergence/02-task-runtime.md` は存在せず、`plans/harness-redesign` が正。代替として `harness-redesign/tasks/jobs-and-schedules.md`, `core/delegation.md`, `decisions.md` が仕様
- 現状: 前回 PR #1233 は plans/backlog のみで close (実コードは未マージ)。`crates/ene-work/src/task.rs` は不在。`crates/ene-work/src/lib.rs` に未配線。
- 境界: ene-work が責務、ene-session のログ所有を侵さない、ene-kernel 非依存を保つ
- リスク: /tasks への全面移行は破壊的なので、まず WorkStore 内部で Task 型と Job 型を共存させ、API は /jobs 互換を保ちつつ internal に Task へ検証を足す段階移行が安全

## 実装方針 (並列トラック)

### Track A: #1250 ヒント修正 (最速, 独立)
- FTL 更新:
  - en-US: "Saves session JSON. The dialog opens in Documents or Downloads as YYYY-MM-DD_HHMM_<companion>_<session>.json."
    より正確に: "Saves session JSON. The dialog opens in Documents or Downloads with a dated file name including the companion and session (e.g. 2026-01-02_0304_Alicia_<session>.json)."
  - ja: "セッション JSON を保存します。ダイアログはドキュメントまたはダウンロードを開き、日付・コンパニオン名・セッションを含むファイル名 (例: 2026-01-02_0304_Alicia_<session>.json) で保存します。"
- 代案: 短く "日付／コンパニオン名を含むファイル名" とする。レビューで長すぎれば調整。
- テスト: 既存 `export_names_are_safe_and_typed` が命名を保証。FTL 変更はcargo test不要だが fmt/clippy 通過と en-US/ja 対を保つ。

### Track B: #1251 status描画 (v1.0, 要慎重)
- app.rs 修正:
  - RefreshHistory: `has_assistant` と並列に `has_status` (`role=="status"`|`"error"`) を算出。surface 更新条件を `has_assistant || has_status` に拡張。status も completion とみなす。
  - ReconcileHistory: `progressed` を `role=="assistant" || role=="status"` で判定 (terminal seq 比較も status 対応)。
  - surface.status への既存 map_turn_err は維持 (status行をstatusバーにも反映)。
- 追加: status 行を transcript に出すことは chat.rs が既に対応済みなので app 側の history 反映だけで表示される。
- テスト: 既存 reconcile/refresh テストに status ケースを足すか、手動再現手順 (issue本文 1-5) で観測。clippy deny系に触れない。

### Track C: #1198 Task Contract (中核, 工数大)
- 新規 `crates/ene-work/src/task.rs`:
  - TaskContract { goal: String, success_criteria: Vec<String>, artifacts: Vec<String>, constraints: Vec<String>, allowed_tools: Vec<String>, workspace: String } + validate()
  - TaskState { Pending, Running, Verifying, Completed, Failed, Cancelled, Interrupted } + transition() で Running->Completed は Verifying 経由を強制
  - ArtifactRef { path, workspace } + verify_artifact() で workspace 外を WorkspaceViolation
  - MailboxRevision, follow-up の scope 拡大検出 (allowed_tools 外があれば再承認要)
  - cancel後の新side effect拒否、restartで Running→Interrupted へ
  - success criteria evaluator: artifacts空なら false (model doneだけではCompletedにならない)
  - 6完了条件をそれぞれ unit test で観測
- 配線: `crates/ene-work/src/lib.rs` に mod task + pub use
- API移行: このPRでは /jobs 互換を保ちつつ内部で Task 検証を呼ぶ。/tasks への全面置換は後続PRで段階移行 (decisions.md D-11 depth分離を尊重)
- テスト: task.rs 内に 6件の条件を直接テスト。cargo test -p ene-work で通す。lints は expect(reason) で narrow scope。

## 並列実行順
1. Track A と Track B は独立・小規模 → 即並列着手 (どちらも main から branch)
2. Track C は独立だが工数大 → 並列で開始、A/B が先に PR 化されても block しない
3. 各PRは 1 issue = 1 branch, Closes #N のみ, Conventional Commits, base=main

## 検証観点
- [ ] 3件すべてに 1:1 Closes と base=main
- [ ] #1250: en-US/ja が同じ意味で実際の YYYY-MM-DD_HHMM_<companion>_<session>.json 命名と一致
- [ ] #1251: status/error 行が Companion transcript に Error バブルとして表示され、Refresh/Reconcile の has_status 分岐で stale にならない
- [ ] #1198: 6完了条件が unit test で観測可能、cargo fmt/clippy/test が通る、architecture boundaries 違反なし
- [ ] cargo fmt --all -- --check / cargo clippy --workspace --all-targets -- -D warnings 相当を該当crateで確認

