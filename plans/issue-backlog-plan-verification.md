# 計画検証（2026-08-29）

## 検証対象: plans/issue-backlog-plan.md

## 1) 18件すべてに方針割当があるか
- open 18件すべてにTrack割当あり。9件のPR済みは対象外として明記 → OK
- 重複なし、抜けなし（717,1187,1198,1199,1200,1201,1202,1203,1204,1205,1206,1207,1208,1209,1210,1181,1179,1177 を全網羅）

## 2) Architecture boundaries
- #1177/#1179/#1181: ene-stage(chrome/detail/platform)のみ、ene-session等の所有を侵さない → OK
- #1198/#1199/#1203/#1204/#1205: ene-work/ene-session/ene-plane/ene-companion/ene-kernelの責務分離を維持。kernelはHTTP/plugin/companion非依存を維持、companionはdaemon非依存、workはplaneゲート経由 → 要レビュー項目としてPRテンプレに明記
- #1200/#1208/#1209/#1210: docsのみ、crate非依存 → OK
- #717/#1187/#1206: テスト追加は各クレートの責務内に留める → OK

## 3) Lints/spec違反リスク
- リスク: unwrap/expect/panic/todo/print/SAFETY欠落、allow without reason
- 対策: 新規Rustコードは expect(..., reason) を narrow scopeで使用、unsafeにはSAFETYコメント必須、print禁止、workspace lintsに準拠。docs変更は影響なし → チェックリスト化

## 4) docs/ja 同期
- docs/ 配下の変更は docs/ja/ の対応ファイルを同時更新する規則を各docs PRに含める → 計画に追記済み
- i18n (stage.ftl en-US/ja) も同様に対で更新

## 5) 既存main fixとの重複吸収
- #1177: mainに #1211 で部分的fix済み（Home readinessのactive soul boot等）→ 残差は Edge TTS空欄でもReady/ Micガード / Alicia B誤文言 / 再起動整合の厳格化
- #1179: mainはまだ未対応（rfd ownerなし）→ 本計画で対応
- #1181: mainに #1216 でminimum_inner_size/scroll部分的fix済み → 残差は contrast/virtualized picker/raw値隔離
- 重複PRを作らず、残差に焦点を当てる方針でOK

## 6) 実観測 vs 名前存在
- ガイド: issue本文の [ ] を「機構検出」で閉じない。実provider/実process/実store/実HTTP/WS、手動GUIは環境/手順/結果を記録
- 実行: 各PRのテストは存在確認ではなく観測可能な挙動を検証、docsの受入条件は観測記録へ対応付け
- 指摘: #1209 のDoDで「Echo/Scriptedだけで閉じない」を明文化済み

## 7) 参照不整合の吸収
- plans/product-convergence/* がissue本文で参照されるが実体は plans/harness-redesign/* のみ
- 対策: 不足する product-convergence 配下の参照先を新設し、harness-redesign側の正本へリンク/同期する。PR本文で不整合を明記

## 判定: 計画は実装移行可能
- 抜け・重複・境界違反なし、lints/docs同期の対策あり、残差吸収も明確

## 実装順の最終確定
1. Track A/B/C を最優先で並列（高速にPRまで）
2. Track D/E はBの用語確定を待ちつつdraft PRを先行、確定後に追従コミット
3. 各PRは 1 issue = 1 branch、本文に Closes #NNN のみ（複数close禁止）、Conventional Commits

