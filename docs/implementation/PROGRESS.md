# 実装進捗状況

現在の開発段階と未完了 gate の短い index です。実装順は [実装ガイド](README.md)、後続作業は [計画一覧](README.md#43-後続作業の実装計画)、製品契約は `docs/requirements/` と `docs/design/` を参照します。

棚卸し: **2026-09-23、`main` `1428deaaf3f449fc427d8d9e9148f573b380f9d4`**。この文書更新は実装・実機検証の完了を意味しません。未マージの PR の変更や検証は現在の `main` の合格実績に含めません。

## 現在進行中のステージ

**Stage 7: 管理画面・デスクトップアバター・最初の受け入れ検証 — 未完了。** A1 と B/C/E、D/F の production candidate は統合済みです。[Stage 7 計画](stages/stage-7.md)の A2 と F、および [Desktop 後続計画](follow-ups/desktop.md)を参照します。

通常 Host–Client 通信の WSS 統一は**設計済み・未実装**です。旧 transport の検証を WSS の合格に読み替えません。

既存の [Windows 11 D/F runtime / platform acceptance](reports/stage-7-windows-df-acceptance-2026-09-22.md) と [KDE Wayland / Linux Secret Service / Linux Performance Gate probe](reports/stage-7-kde-wayland-probe-2026-09-22.md) は、各報告の tip・環境・アセット・transport に限る証拠です。KDE 上の motion pack 最小実描画は Seed-san で確認済みです。公式 `ene`、物理 HiDPI、最終統合 tip・正式 NixOS 対象の合格ではありません。

## 完了したステージ

- **Stage 0**: リポジトリとビルド基盤。
- **Stage 1**: 最小限の共通規約と基盤。
- **Stage 2**: セットアップとテキスト会話。
- **Stage 3**: Experience Summary / Memory の形成・検索・更新。
- **Stage 4**: Task Agent とファイル操作。
- **Stage 5**: Client lifecycle と Host での作業継続。
- **Stage 6**: Targeted Deletion と利用量・機密安全。

完了記録は当時の実装・検証範囲を示します。後続の Learning 設定、費用契約変更、WSS、音声 Task UX の完成を意味しません。

## Stage 7 の未完了 gate

- [A2](stages/stage-7.md#a2-通常-client-通信の-wss-統一未実装): WSS 実装と両 OS の接続・安全性・性能回帰。未知 wire 値と round 指定は [基盤計画](follow-ups/foundation.md)で判断する。
- [D/F](follow-ups/desktop.md): 公式同梱 `ene` VRM の製品検証、物理 display scale=2 の HiDPI 検証、モーション中の framing 修正と公式アセットでの確認。
- [F](stages/stage-7.md#f-milestone-1-の実機-acceptance--performance-と-closeout): 正式 NixOS 26.11 / KDE Wayland と Windows 11 の最終 acceptance、WSS・公式アセットを含む統合 tip の性能・安全性確認。

既存 Windows 報告に後発の WSS 移行は含まれません。外部アセットや指定 OS の検証待ちは独立した後続開発の停止理由にしません。

## 次の作業

WSS 統一と並行して、[Stage 10 D0](stages/stage-10.md#2-d0-要件と設計を先に確定する) の音声 Task UX の要件・設計を確定します。音声操作・対象特定・Workspace/権限・報告の経路が成立する前に既存の作業 GUI を削除しません。

Learning、費用・provider 障害、Desktop、基盤整理の個別の作業順と gate は [後続計画](README.md#43-後続作業の実装計画) に置きます。Stage 8 の Schedule は音声による設定・報告へ接続する次の拡張とし、番号順だけで着手を決めません。
