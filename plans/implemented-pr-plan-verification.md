# 実装済みPR置換計画 検証（2026-08-29）

## 検証対象
`plans/implemented-pr-plan.md`（修正版）

## 検証項目
- [x] 27/27 網羅と 1:1 Closes 言及あり
- [x] 6件 CONFLICTING と #1227 base問題を明記、rebase 手順あり
- [x] placeholder（files=1）の実態と置換方針（追加コミットで上乗せ）を明記
- [x] close可能の定義を観測可能なテスト/文書/コードで証明すると定義、自動検証を必須化
- [x] Track A-E で全18件の対象ファイルと完了条件を対応付け（stage/docs/runtime 横断）
- [x] architecture boundaries（session所有、kernel非依存、workゲート等）を手順に含む
- [x] lint/test/doc gate を共通手順化（fmt/clippy/test）
- [x] 優先順位: Track A/B/C を並列最優先、D/E は依存追従
- [x] 検証観点: 27/27, files>1, boundaries, gate の4点を再監査項目として明記

## 指摘と反映
- 指摘1: dependencyに `cargo doc --workspace --no-deps` が必要（CI gate）→ 各PRの検証手順に含める
- 指摘2: docs変更は `docs/ja/` 同期を必須（Stage/docs系）→ Track B/E の手順に EN/JA 対を明記済み
- 指摘3: placeholder branch の history は潰さず追加コミットで上乗せするため、レビューで diff が追える

## 判定
実装移行可能。Track A/B/C から並列で実装開始する。
