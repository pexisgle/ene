# 実装進捗状況

## 現在進行中のステージ (Current Milestone)

- **Stage 5: クライアントのライフサイクルとホストでの作業継続 — 未着手**
  - Stage 4 は下記のとおり完了しました。Stage 5 の実装はまだ開始していません。
  - クライアント切断時もホスト側でタスクをそのまま継続する。
  - 再接続時に進捗・結果を自動表示する。
  - ホスト再起動時はタスクを勝手に自動再開せず、保存済み進捗を示してユーザーの再開指示を待つ（`Cancelled` のタスクは再開せず、再実行は新しい Task と新しい delegation で行う）。
  - パートナーの存在場所（presence）の整合性を維持する。

## 完了したステージ (Completed)

- ✅ **Stage 0**: リポジトリとビルド基盤の構築（workspace, CI, lint）
- ✅ **Stage 1**: 最小限の共通規約と基盤の作成（primitive, config, api contract）
- ✅ **Stage 2**: セットアップとテキスト会話の最小開通（Host↔Client接続、OpenAI連携、履歴保存）
- ✅ **Stage 3**: 経験の要約と記憶機能（Experience Summary、記憶の形成・検索・更新）
- ✅ **Stage 4**: 作業用エージェント (Task Agent) とファイル操作タスク
  - 完了済みスライス:
    - ✅ Stage 4 前提スライス A: 送信 currentness のための最小 erasure-currentness foundation
    - ✅ スライス B: 採用指示本文の bounded read と Task Agent prompt 配線（provider send）
    - ✅ スライス C: 中断（Cancel、AU16）の durable な受理と、実行中処理のベストエフォート停止の配線
    - ✅ スライス D: 推論 → ツール呼び出し → Action → 観測 → 次 turn → final result の自律ループ
    - ✅ スライス E: Host 結合の Stage 4 E2E（レポート生成、パス脱出の拒否、再起動読み戻し、キャンセル、遅延結果の帰属）
    - ✅ 実行中の外部作用の確定度（`ConfirmedSuccess` / `ConfirmedFailure` / `Unknown`）の保持と、不明な作用の自動再実行禁止
    - ✅ タスクのライフサイクル（開始、進行中、完了、失敗、キャンセル）と方針指示（steering）、対象フォルダ（Workspace）との紐付け、委任、権限チェック、ワークスペース境界（一覧・読込・作成・編集のみ）とパス脱出の遮断
    - ✅ スライス F（会話・第一者管理経路）:
      - 会話の production 経路: companion が provider 出力の最終行に付す closed-world の `[task-control]` directive を dialogue の `finish_turn` が解釈し、composition root の `DialogueTaskControlPort` が既存 Task owner 境界（Task 作成・委任・steering・progress/report・cancel）へ写す。directive 行は presentation と History の双方から隠し、返信は owner の typed outcome から構成した最終表示と History が一致する。通常の Owner message だけで一連の操作が起動し、テスト側の直接 API 呼び出しを必要としない。
      - Workspace authority: Task の Workspace association は第一者管理経路で Owner が選択し Host が canonical directory として検証した trusted premise からのみ成立する。provider / model 出力の任意 path は association / delegation scope / Action authority にならない。
      - Owner currentness: 会話起点の Task 作成・steering・cancel は、依拠した Owner message がまだ最新の accepted Owner input であることを書き込みと同じ不分区間で比較し、superseded turn を `Superseded`（書き込みなし）として拒否する（check-then-act の窓を作らない）。
      - 実行開始: Task 作成 + delegation の受理後、serving composition（shared handle と provider transport）が install した launcher が既存 `run_task_agent` を background 起動する。会話 turn は実行完了を await せず、runner の technical failure を `TaskProgress::Failed` に写さない。
      - 会話からの Task 提案（`ProposeTaskCommand` → 作業担当の Task 作成 orchestration。Task / context / Workspace association 識別子は作業担当が発行）と、既存 AU3 委任・既存 Task Agent 実行経路への接続。会話層から SQL / Workspace / delegation 行を直接書かない。
      - 会話からの追加指示を既存 `propose_steering` / `orchestrate_steering`（AU4）へ接続。stale な依拠 revision は既存 outcome のまま扱い、会話側で retry / 上書きしない。
      - 会話および第一者管理経路（`ManagementIntentKind::CancelTask`）の cancel を既存 `HostHandle::cancel_task`（AU16）へ接続。受理と停止完了を混同せず、cancel 専用の gate / state を追加しない。
      - 進捗・キャンセル・完了報告を既存 canonical facts（`task.progress` / `task_result` 本文 / `task_result_attempt` 相関 / `action_attempt.certainty`）から構成。報告専用の durable master を作らない。
      - 受け入れシナリオ 4.3（実行中の通常チャット）・4.4（キャンセル時点までの完了内容と不明な作用の区別）・4.5（完了報告の変更ファイル名・保存場所・やり残し）・4.7（完了・キャンセル後も既存ファイルが消えない）を会話経路の E2E で確認。
    - ✅ スライス F（sealed result の再採用）:
      - `WithheldByEffectFacts` 後に Action 確定度が客観的証拠で確定したとき、同じ sealed result を既存 `adopt_result` で bounded に再評価する producer を配線（確定度の CAS commit と再採用を 1 つの transaction に統合しない）。
      - AU15a（到着 record / seal）後・AU15b（採用）前に停止した result を、Host の明示的な startup reconciliation（`adopted_revision IS NULL` の bounded 候補列挙）から回収。provider / filesystem Action / Task Agent を再実行しない。
      - 再評価は既存 `adopt_result` を唯一の completion gate とし、`pending_adoption` 等の第二 master / retry queue を追加しない。cancel / moved revision / terminal semantics を維持し、duplicate trigger に対して冪等。
    - ✅ スライス G: 確認済み terminal failure の `TaskProgress::Failed` producer（`fail_task`）。`Started` / `InProgress` からのみ同一 SQLite master 上で atomic に確定し、stale な `TaskRef` / delegation は current Task を終端化できない。`Completed` / `Cancelled` / `Failed` の terminal winner は競合しても一意。provider の一時障害・`NotSent`・Action `Unknown`・`WithheldByEffectFacts`・cancel を `Failed` に写さず、failure 専用の shadow state / gate を追加しない。
  - 完了基準の確認:
    - 指定フォルダ内のファイルを読んで新しいレポートを生成するタスクが正常に完了し、不正なファイルアクセスが拒否される（スライス E の E2E）。
    - 会話経由の受け入れシナリオ 4 を F の E2E で確認し、G の `Failed` producer を実装。
    - `cargo test --workspace` / `cargo clippy --workspace --all-targets -- -D warnings` / `cargo fmt --all -- --check` を Linux / Windows CI で確認。

## 未解決のブロッカー (Blockers)

- **現在ブロッカーはありません。**
  - Stage 5 の設計フォローアップ（クライアント切断・再接続、ホスト再起動時の再開境界）は Issue #1384, #1385, #1387, #1389, #1390 にて追跡中です。
  - Stage 3 の設計フォローアップは Issue #1508、依存関係の整理は Issue #1509 にて対応中（いずれも現在の作業をブロックしません）。
  - Stage 4 で追加した canonical current-condition store は Stage 6 の Targeted Deletion が同じ store を拡張して利用します（別 store / 別 gate を新設しません）。

## 次のステージ (Next Stage)

- **Stage 5: クライアントのライフサイクルとホストでの作業継続**
  - クライアント切断時もホスト側でタスクをそのまま継続する。
  - 再接続時に進捗・結果を自動表示する。
  - ホスト再起動時はタスクを勝手に自動再開せず、保存済み進捗を示してユーザーの再開指示を待つ（`Cancelled` のタスクは再開せず、再実行は新しい Task と新しい delegation で行う）。
  - パートナーの存在場所（presence）の整合性を維持する。
- **Stage 6: 指定データの完全削除 (Targeted Deletion) と利用量・機密安全**
  - 指定したキーワードに関連する会話履歴、要約、記憶、検索データの完全消去と、消去中の新規記憶復活の排他制御。
  - OpenAI のトークン消費量・費用の記録と表示、および認証情報の非漏洩の検証。
