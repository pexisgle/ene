# Review対応 実装計画 (PR #1257 / #1198)

## 指摘
1. Migration未配線: task moduleはene-workから再exportのみで、既存job/delegation runtimeや /jobs APIは無改修。success-criteria evaluatorやfollow-up/question/answer連携なし。6条件はunit testの隔離のみでrunnerに強制されていない。→ 実行経路に配線しAPI/pathを移行、integration testで実runnerがverifying/cancel/restart/scope再承認を迂回できないことを証明せよ。
2. verify_artifactが文字列 starts_with のため /tmp/ws が /tmp/ws2/out.md を誤受理。Path::starts_with + 正規化で直し、prefix sibling と .. 脱出をテストせよ。
3. public transition() が不変条件を迂回: Pending->Completed等が素通りで state を直接代入。汎用bypassを除去するか網羅的transition tableで不正遷移を拒否せよ。
4. CI #2318 Check/clippy失敗: unnested_or_patterns, manual_string_new を exact-head で green に。

## 現状
- crates/ene-work/src/task.rs は単独モジュールで検証ロジックを持つが host/store/routes から呼ばれていない。
- apps/ene-core/src/http/mod.rs は /api/v1/jobs のみ。
- clippyエラー: task.rs:201 unnested_or_patterns, 228/234 manual_string_new。

## 対応方針

### Track 1: task.rs 修正 (clippy + 正しさ)
- workspace閉じ込め: string prefix → Pathベースに。normalizeはcomponentsを辿り CurDirはskip、ParentDirはpop、RootDir/Prefixは保持。検証後に Path::starts_with で判定。prefix sibling (/tmp/ws vs /tmp/ws2) と .. 脱出をテスト追加。
- transition: 汎用 transition を削除し、メソッド経由のみを正とするか、網羅表に置換。許可辺: Pending->Running/Cancelled/Failed, Running->Verifying/Cancelled/Failed/Interrupted, Verifying->Completed/Failed/Cancelled/Interrupted, Completed/Failed/Cancelled/Interruptedは終端（遷移不可）。Running->CompletedはVerificationFailed。Cancelledからの脱出はCancelled。網羅matchで _ は IllegalTransition。
- clippy: or-patternを (Cancelled, Running | Verifying | Completed) にネスト、 "".into() → String::new()。

### Track 2: 実行経路への配線
- WorkStore: jobsテーブルへ success_criteria / allowed_tools を保持するか、別途 task_contracts テーブルを追加。まずは後方互換のため jobs に success_criteria TEXT 列を追加するmigrationを検討するが、最小では DelegationHost::start にて TaskContract::validate を呼び、incompleteなら WorkError::InvalidContract として失敗させる。NewJob に optional contract フィールドを足し、既存 caller は空で通るが新規 task 経路では必須化。
- API移行: ene-api に CreateTaskRequest / TaskView を追加（JobView互換だが success_criteria等を含む）。routes.rs に create_task / get_task / list_tasks / cancel_task を追加し、内部で TaskContract 検証後に host.start を呼ぶ。http/mod.rs に /api/v1/tasks 系ルートを /jobs と並列に登録（段階移行、/jobsは互換維持）。
- Runner: runner.rs / host.rs の complete が verifying 未経由なら失敗することを host.complete 内で TaskState 検証を呼ぶ形で強制。storeのinterrupt_running は既にRunning/VerifyingをInterrupted化していることを再利用し、task側の mark_interrupted_on_restart と整合。

### Track 3: Integration test
- crates/ene-work/tests.rs へ real WorkStore (tempfile) + DelegationHost を使ったテストを追加: incomplete contractで作成失敗、verifying未経由complete失敗、workspace外artifact拒否、cancel後のside effect拒否、restartでInterrupted化、scope拡大で再承認要を、実際のstore/host経由で検証。

## 検証
- cargo fmt --all -- --check
- cargo clippy --workspace --all-targets -- -D warnings (unnested/manual_string_new 解消)
- cargo test -p ene-work (unit + integration)
- gh pr view 1257 で CI Check が success になること

