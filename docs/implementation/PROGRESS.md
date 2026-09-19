# 実装進捗状況

この文書は、現在の開発段階・完了済みステージ・ブロッカー・次のステージを把握するためのインデックスです。Stage ごとの実装計画は `stages/`、実装の詳細な契約は `docs/design/`、受け入れ条件は `docs/requirements/`、個別の作業履歴やフォローアップは GitHub Issues / Pull Requests で管理し、完了済みステージの内部スライスや実装履歴はここでは列挙しません。

## 現在進行中のステージ (Current Milestone)

- **Stage 7: 管理画面・デスクトップアバター・最初の受け入れ検証 — A0 設計整理済み、A1 は PR、B (`ene-desktop`) 実装中**
  - 設計契約・実装順・完了条件は [Stage 7 実装計画](stages/stage-7.md) を参照する。
  - 次の production slice は **F: Milestone 1 の実機 acceptance / performance**。E は GUI の提示・一時データ・障害経路を実装。Stage 7 は完了しない。
  - D renderer-core は `apps/ene-body` に投影 IPC と headless pose/health を追加するが、overlay / VRM runtime / SpringBone / IME / NixOS 26.11 の技術成立 probe は未実施。acceptance §2 と Stage 7 完了にはしない。
  - GUI・秘密入力・OS 保護ストア・overlay / VRM の技術成立 probe は未実施。slice F まで Stage 7 を完了としない。
  - 公式同梱 `ene` VRM は未投入（[#1651](https://github.com/pexisgle/ene/issues/1651)）。placeholder を製品キャラクターとして扱わない。

## 完了したステージ (Completed)

- ✅ **Stage 0**: リポジトリとビルド基盤の構築（workspace, CI, lint）
- ✅ **Stage 1**: 最小限の共通規約と基盤の作成（primitive, config, api contract）
- ✅ **Stage 2**: セットアップとテキスト会話の最小開通（Host↔Client 接続、OpenAI 連携、履歴保存）
- ✅ **Stage 3**: 経験の要約と記憶機能（Experience Summary、記憶の形成・検索・更新）
- ✅ **Stage 4**: 作業用エージェント (Task Agent) とファイル操作タスク
- ✅ **Stage 5**: クライアントのライフサイクルとホストでの作業継続
- ✅ **Stage 6**: 指定データの完全削除 (Targeted Deletion) と利用量・機密安全（[完了確認 #1597](https://github.com/pexisgle/ene/issues/1597)）

## 未解決のブロッカー (Blockers)

- **現在ブロッカーはありません。**
- 個別の非ブロッキングな設計・実装フォローアップは GitHub Issues で管理します。
- Stage 7 の実機 acceptance / 性能検証は未実施です。正式版 NixOS 26.11 KDE Wayland での最終 acceptance は slice F の完了条件であり、それ以前の slice の着手条件ではありません。

## 次のステージ (Next Stage)

- **Stage 8: 定期 schedule 実行**
  - Stage 7 / Milestone 1 完了後、Task・permission・usage 管理を前提に進める。
  - 着手時に `stages/stage-8.md` を作成し、実装順と完了条件を確定する（[roadmap](README.md#4-最初の-milestone-後の-roadmap)）。
