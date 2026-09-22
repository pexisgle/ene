# 実装進捗状況

この文書は、現在の開発段階・完了済みステージ・ブロッカー・次のステージを把握するためのインデックスです。Stage ごとの実装計画は `stages/`、実装の詳細な契約は `docs/design/`、受け入れ条件は `docs/requirements/`、個別の作業履歴やフォローアップは GitHub Issues / Pull Requests で管理し、完了済みステージの内部スライスや実装履歴はここでは列挙しません。

## 現在進行中のステージ (Current Milestone)

- **Stage 7: 管理画面・デスクトップアバター・最初の受け入れ検証 — A1 と B/C/E は統合済み、D/F の実 desktop acceptance に blocker が残る**
  - 設計契約・実装順・完了条件は [Stage 7 実装計画](stages/stage-7.md) を参照する。
  - 通常 Host–Client 通信の WSS 統一は設計更新済み・実装未着手。[Stage 7 A2](stages/stage-7.md#a2-通常-client-通信の-wss-統一未実装) で置換と再検証を行う。既存の OS ローカル接続の実装・検証を WSS 対応済みとは扱わない。
  - Linux 検証は Cloud Agent 上の Ubuntu 24.04 X11 で実施済み（[報告](reports/stage-7-linux-2026-09-19.md)）。NixOS 26.11 を待たない。Stage 7 は完了しない。
  - D/F production candidate（KDE layer-shell / Windows DWM、wgpu surface、`vrm-runtime` 0.1、presentation/CPU/RSS/operation measurement）は実装済み（[実装報告](reports/stage-7-df-implementation-2026-09-21.md)）。Windows 11 D/F runtime / platform acceptanceは完了（[Windows D/F acceptance報告](reports/stage-7-windows-df-acceptance-2026-09-22.md)）。
  - KDE Wayland 実環境の技術成立 probe（layer-shell 表示、click-through、wp_presentation evidence、hide/restore/resize、入れ子 KWin scale=2、IME）、Linux Secret Service 実機 probe、Linux Performance Gate（release 5 分、全 PID、presented FPS、cancel 1 秒、click-through）を実施し、Pass と未実施を分離して記録した（[KDE Wayland probe 報告](reports/stage-7-kde-wayland-probe-2026-09-22.md)）。probe で見つけた実装欠陥 6 件（presentation output、clean exit segfault、input region、hide/restore、clippy gate、FPS 窓境界）は修正済みで、land を [#1677](https://github.com/pexisgle/ene/issues/1677) で追跡する。
  - 残件: 公式 VRM（[#1651](https://github.com/pexisgle/ene/issues/1651)）、物理 display scale=2 の HiDPI（[#1678](https://github.com/pexisgle/ene/issues/1678)）、motion pack 実描画（[#1679](https://github.com/pexisgle/ene/issues/1679)）、IME 確定後の送信の切り分け（[#1680](https://github.com/pexisgle/ene/issues/1680)）。正式リリース NixOS 26.11 は存在しないため Linux 最終 acceptance は open のまま。Windows側の残件は公式VRMだけ。placeholder や Seed-san を製品キャラクターとして扱わない。

  - 統合 tip は [#1664](https://github.com/pexisgle/ene/pull/1664) として `main` に land 済み。Windows 報告の GUI 再起動 P1、`CredentialStored` の早すぎる返却、A1 の trust boundary 再設計（requester listener と Host-spawned GUI の専用確認 channel の分離、`SeatHello` 経路の削除、`ene-core approve-*` の requester 化、offline mutation fallback の削除）、credential publication、OS-store adapter、Linux gate 修正、CI 高速化を含む。
  - credential の登録は version 公開として実装済み: OS item は version ごとに作り、activation transaction が sweep・active version・revision・outcome を一緒に commit する。実 OS store adapter は Windows Credential Manager と Linux Secret Service (ksecretd) の両方で実機 probe 済み（[KDE Wayland probe 報告](reports/stage-7-kde-wayland-probe-2026-09-22.md)）。

## 完了したステージ (Completed)

- ✅ **Stage 0**: リポジトリとビルド基盤の構築（workspace, CI, lint）
- ✅ **Stage 1**: 最小限の共通規約と基盤の作成（primitive, config, api contract）
- ✅ **Stage 2**: セットアップとテキスト会話の最小開通（Host↔Client 接続、OpenAI 連携、履歴保存）
- ✅ **Stage 3**: 経験の要約と記憶機能（Experience Summary、記憶の形成・検索・更新）
- ✅ **Stage 4**: 作業用エージェント (Task Agent) とファイル操作タスク
- ✅ **Stage 5**: クライアントのライフサイクルとホストでの作業継続
- ✅ **Stage 6**: 指定データの完全削除 (Targeted Deletion) と利用量・機密安全（[完了確認 #1597](https://github.com/pexisgle/ene/issues/1597)）

## 未解決のブロッカー (Blockers)

- Stage 7 D の製品キャラクター完成に対する外部 blocker は、公式 VRM（[#1651](https://github.com/pexisgle/ene/issues/1651)）のみ。
- Stage 7 完了には、公式VRM、正式リリース NixOS 26.11 での Linux 最終 acceptance、物理 display scale=2 の HiDPI、motion pack 実表示が必要。KDE Wayland 実 overlay、Linux Secret Service、Linux Performance Gate、IME は実環境 probe 済み（[報告](reports/stage-7-kde-wayland-probe-2026-09-22.md)）。Windows側の残件は公式VRM #1651だけ。
- 個別の非ブロッキングな設計・実装フォローアップは GitHub Issues で管理します。
- Stage 7 の Linux 自動テストと X11 GUI 操作は [Linux 報告](reports/stage-7-linux-2026-09-19.md) にある。Windows 11 D/F runtime / platform acceptanceは完了し、Windows側の残件は公式VRM #1651だけ。

## 次のステージ (Next Stage)

- **Stage 8: 定期 schedule 実行**
  - Stage 7 / Milestone 1 完了後、Task・permission・usage 管理を前提に進める。
  - 着手時に `stages/stage-8.md` を作成し、実装順と完了条件を確定する（[roadmap](README.md#4-最初の-milestone-後の-roadmap)）。
