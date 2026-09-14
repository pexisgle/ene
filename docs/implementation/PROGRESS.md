# 実装進捗状況

この文書は、現在の開発段階・完了済みステージ・ブロッカー・次のステージを把握するためのインデックスです。実装の詳細な契約は `docs/design/`、受け入れ条件は `docs/requirements/`、個別の作業履歴やフォローアップは GitHub Issues / Pull Requests で管理し、完了済みステージの内部スライスや実装履歴はここでは列挙しません。

## 現在進行中のステージ (Current Milestone)

- **Stage 5: クライアントのライフサイクルとホストでの作業継続 — 未着手**
  - クライアント切断時もホスト側でタスクをそのまま継続する。
  - 再接続時に進捗・結果を自動表示する。
  - ホスト再起動時はタスクを勝手に自動再開せず、保存済み進捗を示してユーザーの再開指示を待つ（`Cancelled` のタスクは再開せず、再実行は新しい Task と新しい delegation で行う）。
  - パートナーの存在場所（presence）の整合性を維持する。

## 完了したステージ (Completed)

- ✅ **Stage 0**: リポジトリとビルド基盤の構築（workspace, CI, lint）
- ✅ **Stage 1**: 最小限の共通規約と基盤の作成（primitive, config, api contract）
- ✅ **Stage 2**: セットアップとテキスト会話の最小開通（Host↔Client 接続、OpenAI 連携、履歴保存）
- ✅ **Stage 3**: 経験の要約と記憶機能（Experience Summary、記憶の形成・検索・更新）
- ✅ **Stage 4**: 作業用エージェント (Task Agent) とファイル操作タスク
  - Owner が指定した Workspace 内で、Task Agent が一覧・読込・作成・編集を安全に実行できる。
  - 会話から Task の作成・steering・進捗確認・cancel・結果報告まで行え、Task Agent は Host 上でバックグラウンド実行される。
  - Task lifecycle、Action certainty、結果の採用・再評価を durable facts に基づいて管理し、`Unknown` な作用を自動再実行しない。
  - `docs/requirements/acceptance.md` の Stage 4 受け入れシナリオと Linux / Windows CI を通過済み。

## 未解決のブロッカー (Blockers)

- **現在ブロッカーはありません。**
- 個別の非ブロッキングな設計・実装フォローアップは GitHub Issues で管理します。

## 次のステージ (Next Stage)

- **Stage 6: 指定データの完全削除 (Targeted Deletion) と利用量・機密安全**
  - 指定したキーワードに関連する会話履歴、要約、記憶、検索データの完全消去と、消去中の新規記憶復活の排他制御。
  - OpenAI のトークン消費量・費用の記録と表示、および認証情報の非漏洩の検証。
