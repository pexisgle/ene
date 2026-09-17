# 実装進捗状況

この文書は、現在の開発段階・完了済みステージ・ブロッカー・次のステージを把握するためのインデックスです。実装の詳細な契約は `docs/design/`、受け入れ条件は `docs/requirements/`、個別の作業履歴やフォローアップは GitHub Issues / Pull Requests で管理し、完了済みステージの内部スライスや実装履歴はここでは列挙しません。

## 現在進行中のステージ (Current Milestone)

- **Stage 6: 指定データの完全削除 (Targeted Deletion) と利用量・機密安全 — 未着手**
  - 設計契約・実装順・完了条件は [実装ガイドの Stage 6](README.md#stage-6-指定データの完全削除-targeted-deletion-と利用量機密安全) を参照する。
  - 指定したキーワードに関連する会話履歴、要約、記憶、検索データの完全消去と、消去中の新規記憶復活の排他制御。
  - OpenAI のトークン消費量（入力・出力・キャッシュ）と発生費用の記録・表示。
  - API キーなどの認証情報がログやプロンプトに漏洩しないことの検証。

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
- ✅ **Stage 5**: クライアントのライフサイクルとホストでの作業継続
  - Client を閉じても Host-only Task は継続し、再接続と正式な presence 成立の時点で、未伝達の進捗・結果を自動表示する。
  - Host 再起動では実行を自動再開せず、保存済みの進捗・結果を示して明示 resume を待つ（同じ Task の r+1・新しい delegation として続け、旧 attempt の結果は元の実行に残す）。
  - 同一 device の replacement は旧 connection の transient world（presentation receipt / Round / stream / 再試行 epoch / first-party Task selection）を無効化し、新 connection へ何も暗黙継承しない（CCT §10.4 / IPC §9.3 の connection currentness linearization）。
  - connection phase、client incarnation、pairing identity、単一 Host lock、presence の遷移と復旧表を、Linux の socket と Windows の named pipe の実 transport で検証する。
  - 受理後の Accepted/Open publication、ConfirmPresentation、presentation-start/ACK の durable CAS を connection ownership と直列化。deterministic replacement tests と mutation checks で退行を検出する。
  - `docs/requirements/acceptance.md` の Stage 5 受け入れシナリオ (S5-01〜24)、既存の全 review finding、replacement lifecycle invariants を再照合済み。修正後の D/F 両 stack で Linux / Windows CI を通過し、完了を確認した。

## 未解決のブロッカー (Blockers)

- **現在ブロッカーはありません。**
- 個別の非ブロッキングな設計・実装フォローアップは GitHub Issues で管理します。

## 次のステージ (Next Stage)

- **Stage 7: 管理画面・デスクトップアバター・最初の受け入れ検証**
  - 会話・記憶・タスク・費用を一覧できるテキスト管理画面と、VRM 1.0 のデスクトップアバター（停止時もチャットと管理画面は動く）。
  - 受け入れ条件の全シナリオの実施と、Windows / Linux 両環境での性能テスト（CPU・メモリ・FPS）。
