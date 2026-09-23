# 実装進捗状況

現在の開発段階・完了済み Stage・blocker・次の作業の短い index です。実装順と Issue の対応は [実装ガイド](README.md)、個別計画は `stages/`、契約は `docs/requirements/` と `docs/design/`、作業履歴は Issues / PRs を参照します。

棚卸し: **2026-09-23、`main` `c03f7d18b4232ceb9883a0c912efa1cb1f218b2b`**。未マージの [#1697](https://github.com/pexisgle/ene/pull/1697) / [#1699](https://github.com/pexisgle/ene/pull/1699) の変更・検証は、この基準の完了実績に含めません。

## 現在進行中のステージ (Current Milestone)

**Stage 7: 管理画面・デスクトップアバター・最初の受け入れ検証 — 未完了。** A1 と B/C/E、D/F の production candidate は統合済みです。計画は [stage-7.md](stages/stage-7.md)、残件と統合後の完了判定は [#1706](https://github.com/pexisgle/ene/issues/1706) で追跡します。

通常 Host–Client 通信の WSS 統一は**設計済み・未実装**です（[A2](stages/stage-7.md#a2-通常-client-通信の-wss-統一未実装)、[#1705](https://github.com/pexisgle/ene/issues/1705)）。旧 transport の検証を WSS の合格に読み替えません。

既存の検証記録:

- [Windows 11 D/F runtime / platform acceptance](reports/stage-7-windows-df-acceptance-2026-09-22.md) と [KDE Wayland / Linux Secret Service / Linux Performance Gate の probe](reports/stage-7-kde-wayland-probe-2026-09-22.md) は実施済みです。各報告の tip・環境・アセット・transport の範囲に限る証拠であり、最終統合 tip や正式 NixOS 対象での合格ではありません。
- [#1677](https://github.com/pexisgle/ene/issues/1677) は修正の land、[#1679](https://github.com/pexisgle/ene/issues/1679#issuecomment-5766485295) は KDE 上の motion pack 最小実描画検証として closed です。公式 VRM での見た目は #1651 に残ります。
- [#1680](https://github.com/pexisgle/ene/issues/1680#issuecomment-5770099443) はユーザーの手動確認で UI 問題が確認されず closed です。追加の自動テストや全 OS の acceptance を実施した意味ではありません。

## 完了したステージ (Completed)

- **Stage 0**: リポジトリとビルド基盤。
- **Stage 1**: 最小限の共通規約と基盤。
- **Stage 2**: セットアップとテキスト会話。
- **Stage 3**: Experience Summary / Memory の形成・検索・更新。
- **Stage 4**: Task Agent とファイル操作。
- **Stage 5**: Client lifecycle と Host での作業継続。
- **Stage 6**: Targeted Deletion と利用量・機密安全（[完了確認 #1597](https://github.com/pexisgle/ene/issues/1597)）。

完了記録は当時の実装・検証範囲を示します。後続の WSS 移行や音声 Task UX の完成を意味しません。

## 未解決のブロッカー (Blockers)

Stage 7 の closeout [#1706](https://github.com/pexisgle/ene/issues/1706) に、次を残します。

- WSS 実装と両 OS の接続・安全性・性能回帰（[#1705](https://github.com/pexisgle/ene/issues/1705)）。
- 公式同梱 `ene` VRM と製品キャラクターの検証（[#1651](https://github.com/pexisgle/ene/issues/1651)）、物理 display scale=2 の HiDPI 検証（[#1678](https://github.com/pexisgle/ene/issues/1678)）。Seed-san や入れ子 KWin で代替しません。
- Support Matrix が指定する正式 NixOS 26.11 / KDE Wayland の最終 acceptance と、WSS・公式アセットを含む統合 tip の受け入れ・性能確認。

既存 Windows 報告に後発の WSS 移行は含まれません。「Windows 側は公式 VRM だけで完了」とは扱いません。外部アセットや指定 OS の検証待ちは、独立した後続開発の停止理由にしません。

## 次の作業 (Next)

**WSS 統一と並行して、Stage 10 の音声 Task UX の要件・設計を先に確定します。** [Stage 10 D0](stages/stage-10.md#2-d0-要件と設計を先に確定する) / [#1686](https://github.com/pexisgle/ene/issues/1686) は未完了です。音声操作・対象特定・Workspace/権限・報告の経路が成立する前に、既存の作業 GUI を削除しません。

基盤整理と後続機能の着手条件は [roadmap](README.md#4-最初の-milestone-後の-roadmap) を参照します。Stage 8 の Schedule は音声による設定・報告へ接続する次の拡張とし、Stage 番号の順だけでは着手を決めません。
