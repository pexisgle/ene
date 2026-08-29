# 実装済みPR置換計画（2026-08-29 修正版）

## 目的
前回並列作成した 18 本の PR（#1230-#1247）が `plans/backlog/issue-N.md` 1ファイルのみの placeholder で、issueの完了条件を満たしていないため、実装→検証→push の順で全件を close可能なレベルへ置換する。

## 現状監査
- open issue 27件、open PR 27件で 1:1 `Closes #N` は成立（厳密・大文字小文字含め重複なし、missing 0）
- base 検証: 26/27 が base=main で close可能、1件（#1227 base=feat/stage-avatar-reactions）は要修正
- mergeable: 21 MERGEABLE / 6 CONFLICTING（#1214 #1218 #1223 #1225 #1228 #1230）
- 内容監査: #1231-#1247（+一部既存）のうち 17本が placeholder（files=1, plans/backlogのみ）→ 実装不足

## 置換方針
- placeholder branch を force-push で中身を実装へ置換（historyは残すが空コミットは上書きしない、追加コミットで上乗せ）
- 各 PRは 1 issue = 1 branch, `Closes #N` のみ、Conventional Commits、base=main を徹底
- 実装レベル定義: issue本文の「完了条件」チェックボックスが観測可能なテスト/コード/文書で証明できること。少なくとも1つの自動検証（cargo test / lint / 文書整合）を持つ

## 並列トラック（実装順）

### Track A: Stage v1.0 bug（コード小規模、即 close）
- #1177 readiness矛盾: Home/Conversation/Voice readiness を保存有無ではなく probe最小条件で判定、active badge、Alicia-B誤文言、Micガード（STT未設定はONせずCTA）、再起動整合テスト。対象 apps/ene-stage/src/detail/mod.rs + apps/ene-stage/src/app.rs + テスト
- #1179 dialog背面: rfd FileDialog に Detail window を parent として渡す、export filename に日時+companion名。対象 apps/ene-stage/src/detail/mod.rs, apps/ene-stage/src/app.rs（既存 #1220 と統合・conflict解消）
- #1181 responsive/a11y: chrome minimum_inner_size、Detail/Home scroll、provider picker virtualized、StatusTone contrast、raw値隔離、MCP折りたたみ。対象 apps/ene-stage/src/chrome.rs, detail/mod.rs, primitives.rs

### Track B: 製品決定・要件・方針（docs中心、相互依存なしで並列）
- #1200 product決定: plans/harness-redesign/product/vision.md, decisions.md, features.md, done.md を D番号付きで同期、PC-D1..D6反映、v1境界を10体験で言語化、EN/JA同期の雛形
- #1208 要件再分類: P-1xx..P10xx を V1-Core/V1-Safety/Presence/Learning/Later/Form-only へ再分類、新P番号付与、features.md/done.md同期、Later→v1復帰禁止規則
- #1209 実装方針: crate境界/typed state/REST投影/WS差分/スコープ検証等のポリシー文書 plans/harness-redesign/implementation-policy.md
- #1210 epic統合: plans/product-convergence.md 相当の統合文書と実装順固定、代表E2E gate定義

### Track C: harness v1.0 完成定義
- #717 tracker: plans/harness-redesign/product/harness-v1-tracker.md で done.md の各未チェックを issue or 対象外+後継milestoneへマッピング、gate定義
- #1187 blocking: plans/harness-redesign/product/blocking-observability.md で offline GGUF等の受入観測手順を個別定義

### Track D: Task/Attention/Computer（runtime、並行）
- #1198 Task Contract: crates/ene-work/src/task.rs（TaskContract/State/verifying/evaluator/artifact registry/workspace confinement/mailbox revision/follow-up/cancel/Interrupted）+ /tasks API移行、テストで6条件を観測
- #1199 Attention: crates/ene-companion/src/attention.rs（Item/Store/state/priority/action_required/dedupe/expiry/adapter/gate/delivery）+ Task Center API、raw直出禁止等をテスト
- #1203 Computer: crates/ene-work/src/computer.rs（WindowIdentity/ObservationId/UIA backend/stale-safe/postcondition/Grant/hard confirmation）+ 監査追跡テスト
- #1201 vertical slice: VS-01..07 を実モデル+実toolで Markdown生成→Attention→表層報告のパイプライン、Task Center open、手動手順+自動テスト

### Track E: 横断
- #1204 presence: body idle/look-at→TTS/lip-sync→memory→affect→proactive→STT/barge-inの順接続、矛盾テスト
- #1205 learning: crates/ene-work/src/learning.rs（Candidate/store/validator/replay/canary/rollback）
- #1206 verification: plans/harness-redesign/product/verification-harness.md + 実provider/UIA/metricsの検証体系
- #1207 migration: plans/harness-redesign/migration-rollout.md で直列順序と共存禁止等
- #1202 stage IA: apps/ene-stage IA再構成（Conversation/Tasks&Attention/Companion、setup wizard、scoped approval、Advanced隔離、keyboard/UIA）

## 実装手順（各issue共通）
1. `git checkout <branch> && git rebase origin/main` で conflict解消
2. 対象ファイルを追加/編集（crate境界を守る、unwrap禁止、SAFETYコメント、workspace deps）
3. `cargo fmt --all -- --check` / `cargo clippy --workspace --all-targets -- -D warnings` / `cargo test -p <pkg> --lib` を該当crateで実行
4.  `git add && git commit && git push`（force-with-leaseは使わず追加コミット）
5. `gh pr view <n> --json files,mergeable` で files>1 かつ close可能を確認

## 優先順位
1. Track A/B/C の docs+stage小規模を最優先で並列（高速にPRを実装済みへ）
2. Track D/E の runtimeは依存を待ちつつ draft実装を先行、確定後に追従

## 検証観点
- [ ] 27/27 に1:1 Closes と base=main が保持されているか
- [ ] files>1 かつ issue完了条件に対応するコード/文書/テストが存在するか
- [ ] architecture boundaries違反なし
- [ ] lint/test/doc gateが通るか
