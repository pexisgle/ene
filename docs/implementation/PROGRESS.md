# 実装進捗状況

## 現在進行中のステージ (Current Milestone)

- **Stage 5: クライアントのライフサイクルとホストでの作業継続**
  - クライアント切断時もホスト側でタスクをそのまま継続する。
  - 再接続時に進捗・結果を自動表示する。
  - ホスト再起動時はタスクを勝手に自動再開せず、保存済み進捗を示してユーザーの再開指示を待つ（`Cancelled` のタスクは再開せず、再実行は新しい Task と新しい delegation で行う）。
  - パートナーの存在場所（presence）の整合性を維持する。


## 完了したステージ (Completed)

- ✅ **Stage 0**: リポジトリとビルド基盤の構築（workspace, CI, lint）
- ✅ **Stage 1**: 最小限の共通規約と基盤の作成（primitive, config, api contract）
- ✅ **Stage 2**: セットアップとテキスト会話の最小開通（Host↔Client接続、OpenAI連携、履歴保存）
- ✅ **Stage 3**: 経験の要約と記憶機能（Experience Summary、記憶の形成・検索・更新）
- ✅ **Stage 4**: 作業用エージェントとファイル操作タスク (Task / Action)
  - タスクのライフサイクル（開始、進行中、完了、キャンセル）と方針指示（steering）
  - 対象フォルダ（Workspace）との紐付け、作業用エージェントへの委任、権限チェック
  - ワークスペース境界（一覧・読込・作成・編集のみ）とパス脱出・シンボリックリンク経由の遮断
  - Stage 4 前提スライス A: 送信 currentness のための最小 erasure-currentness foundation
  - スライス B: 採用指示本文の bounded read と Task Agent prompt 配線（provider send）
  - スライス C: 中断（Cancel、AU16）の durable な受理と、実行中処理のベストエフォート停止の配線
  - スライス D: 推論 → ツール呼び出し → Action → 観測 → 次 turn → final result の自律ループ
  - スライス E: Stage 4 全体を通す Host 結合の E2E（レポート生成、パス脱出の拒否、再起動読み戻し、キャンセル、遅延結果の帰属）
  - 実行中の外部作用の確定度（`ConfirmedSuccess` / `ConfirmedFailure` / `Unknown`）の保持と、不明な作用の自動再実行禁止
  - ドメイン境界と Host 実行ループは完成しており、会話・管理画面からタスクの作成・追加指示・キャンセルを操作するクライアント向けコマンド面は、これらの境界の上に後続の作業で追加します。


## 未解決のブロッカー (Blockers)

- **現在ブロッカーはありません。**
  - Stage 5 の設計フォローアップ（クライアント切断・再接続、ホスト再起動時の再開境界）は Issue #1384, #1385, #1387, #1389, #1390 にて追跡中です。
  - Stage 3 の設計フォローアップは Issue #1508、依存関係の整理は Issue #1509 にて対応中（いずれも現在の作業をブロックしません）。
  - Stage 4 で追加した canonical current-condition store は Stage 6 の Targeted Deletion が同じ store を拡張して利用します（別 store / 別 gate を新設しません）。


## 次のステージ (Next Stage)

- **Stage 6: 指定データの完全削除 (Targeted Deletion) と利用量・機密安全**
  - 指定したキーワードに関連する会話履歴、要約、記憶、検索データの完全消去と、消去中の新規記憶復活の排他制御。
  - OpenAI のトークン消費量・費用の記録と表示、および認証情報の非漏洩の検証。
