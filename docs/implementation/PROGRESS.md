# 実装進捗状況

現在の開発段階と未完了 gate の短い index です。実装順は [実装ガイド](README.md)、後続作業は [計画一覧](README.md#43-後続作業の実装計画)、製品契約は `docs/requirements/` と `docs/design/` を参照します。

棚卸し: **2026-09-23、`main` `1428deaaf3f449fc427d8d9e9148f573b380f9d4`**。この文書更新は実装・実機検証の完了を意味しません。未マージの PR の変更や検証は現在の `main` の合格実績に含めません。

## 現在進行中のステージ

**Stage 7: 管理画面・デスクトップアバター・最初の受け入れ検証 — 未完了。** A1、A2、B/C/E、D/F の production candidate は統合済みです。[Stage 7 計画](stages/stage-7.md)の F、および [Desktop 後続計画](follow-ups/desktop.md)を参照します。

通常 Host–Client 通信の WSS 統一は **WSS production implementation 完了**（Linux / Windows の両 CI で green）で、**automated real-WSS tests** も完了しています。自動検証済みの範囲: 未知 wire 値の型付き拒否（接続維持）、Origin / トークン / 起動世代の拒否、過大・不正・Text frame の切断、runtime 情報の保護された原子公開と正常終了時の削除、保護不備 runtime / pin の拒否と秘密の非露出、Host pin 変更（`HostPinMismatch`）と Owner 再確認、偽 Host（pin 不一致）の拒否、stale runtime + 再利用 port に対する `host_is_serving()` の拒否、machine deadline と Owner confirmation 待機の分離、pending pairing 上限、liveness window 中の idle 継続、connection replacement、古い connection の close が current を消さないこと、presence reconnect、reconnect 後の自動 replay なし、stalled pre-auth client が次の接続を遅延させないこと、select branch の活動が進行中の upgrade を切断しないこと、shutdown が upgrading / serving task を停止・join すること、Windows で既存親 directory の ACL を変えず新規作成分のみ owner-only にすること。

自動検証できず **残る gate**（slice F / 実機）: Windows / Linux 実機での V-15 / V-16 確認（別 OS ユーザーによる runtime 読取・書換えの実環境拒否、実機の crash 後 runtime 残存と port 再利用）、実負荷での slow consumer、acceptance S5-01〜24 の GUI 実機証拠、TLS 追加後の性能計測、公式 asset・指定 OS の最終 acceptance。旧 transport の検証を WSS の合格に読み替えません。

過去の実機検証は次の範囲に限る。詳細な当時の記録は [Git 履歴](https://github.com/pexisgle/ene/tree/1f9cbfac0e74e237449c87364304bc317ce28181/docs/implementation/reports) にある。

- **Windows 11（2026-09-22）**: build 26200、Seed-san、`5bc0b12818b65ea800a43aa9b0de1beafdf53fb2` を base とする修正 tree で D/F の表示・入力・性能を検証。公式 `ene` と後発の WSS を含む統合 tip の合格ではない。
- **KDE Wayland（2026-09-22）**: NixOS 26.11 開発版、Seed-san、`24dd163cd1b62a6eb5efc7b6065e09a8d00a6091` を base とする修正 tree で overlay、Linux Secret Service、IME、性能と motion pack の最小実描画を probe。物理 display scale=2、公式 `ene`、正式 NixOS 26.11、後発の WSS は未検証。

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

- [A2](stages/stage-7.md#a2-通常-client-通信の-wss-統一): 実装・自動テスト・未知 wire 値と round 指定の決定は完了済み。残るは両 OS **実機**での V-15 / V-16 回帰（crash 後 runtime 残存・port 再利用、別 OS ユーザー拒否を含む）と acceptance S5 の実機証拠、TLS 追加後の性能。[基盤計画](follow-ups/foundation.md)の F1 は確定済みです。
- [D/F](follow-ups/desktop.md): 公式同梱 `ene` VRM の製品検証、物理 display scale=2 の HiDPI 検証、モーション中の framing 修正と公式アセットでの確認。
- [F](stages/stage-7.md#f-milestone-1-の実機-acceptance--performance-と-closeout): 正式 NixOS 26.11 / KDE Wayland と Windows 11 の最終 acceptance、WSS・公式アセットを含む統合 tip の性能・安全性確認。

既存 Windows 報告に後発の WSS 移行は含まれません。外部アセットや指定 OS の検証待ちは独立した後続開発の停止理由にしません。

## 次の作業

[Stage 10 D0](stages/stage-10.md#2-d0-要件と設計を先に確定する) の音声 Task UX の要件・設計は確定済みです（現行の source of truth は Stage 10 計画）。音声操作・対象特定・Workspace/権限・報告の経路が成立する前に既存の作業 GUI を削除しません。Stage 10 の production 実装は Stage 7 の残 gate と独立に、Stage 10 計画の順で進めます。

Learning、費用・provider 障害、Desktop、基盤整理の個別の作業順と gate は [後続計画](README.md#43-後続作業の実装計画) に置きます。Stage 8 の Schedule は音声による設定・報告へ接続する次の拡張とし、番号順だけで着手を決めません。
