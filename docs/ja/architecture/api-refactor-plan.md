# API リファクタリング計画

- **Status:** 進行中
- **Date:** 2026-07-09
- **最終更新:** 2026-07-09（P0–P3 の初回実装を反映。[完了分](#完了-2026-07-09) を参照）

## 背景

2026-07 の API ドキュメント監査で全ライブラリクレートのページを刷新し、`ene-mind` と `ene-vrm` を追加した。2026-07-09 に第一弾の実装を行った。本ドキュメントは**残作業**のみを追跡する。

## 目標

1. 意図したホスト API（`EneHandle`、ツール IPC ABI）を壊さずに、偶発的な公開面を縮小する。
2. クレート境界を [認知ランタイム ADR](cognitive-runtime.md) および [AGENTS.md](../../AGENTS.md) §4.1 に揃える。
3. クレート横断で async / sync とエラー命名を予測可能にする。
4. セッション分割 → 圧縮、およびイベントモデルの変更をアプリが安全に追従できる段階で進める。
5. ツールのワイヤープロトコルを安定化し、デスクトップが使う VRM 入口を絞り込む。

## 非目標

- クレート統合や、ツールバイナリのサンドボックスモデルの廃止。
- API 変更に追従する以外のアプリ UX（`ene-cli` / `ene-desktop`）の作り直し。
- sea-orm による SQLite 所有権の変更（引き続き `ene-store` 専有）。

---

## 完了（2026-07-09）

| 領域 | 成果物 |
|---|---|
| **公開面** | `ene-core::schema_link`（認知 ctor リンク #95）; `streaming` / `message_builder` にコントリビュータ向け注記 |
| **Async / エラー** | [`docs/api/index.md`](../api/index.md) の Error & async 規約; `run_tool_server` が `Result<(), ToolError>` を返す |
| **境界** | `ene-mind` / `ene-store` / `ene-tool-proto` の ADR ガードレール module docs |
| **ツール ABI** | `ene-tool-common` の `ActionSetProvider` / `SingleActionProvider`; [`docs/tools/sdk.md`](../tools/sdk.md) の ABI 表; `AGENTS.md` R1 修正; `tools/utility` 移行 |
| **イベント / セッション** | [`docs/core/streaming-events.md`](../core/streaming-events.md); 分割 API への圧縮優先 doc コメント |
| **VRM** | `ene_vrm::prelude`; 内部ローダー等の `#[doc(hidden)]`; [`docs/api/ene-vrm.md`](../api/ene-vrm.md) の Supported vs Internal |
| **API ドキュメント** | 14 ライブラリクレート + `ene-mind` / `ene-vrm` の EN+JA 全面刷新 |

---

## 残作業

### 1. 公開面の縮小（フォローアップ）

- 偶発的な `ene-core` ルート再エクスポートを整理し、アプリは所有クレートから import する方針へ。
- 可視性パス: `ene-mind`, `ene-store`, `ene-mind`, `ene-tool-host` で未使用の `pub` を `pub(crate)` に。
- クレートごとに `cargo doc --no-deps` で確認。

**影響クレート:** `ene-core`, `ene-mind`, `ene-store`, `ene-mind`, `ene-tool-host`

---

### 2. クレート境界の一貫性（フォローアップ）

- cognition 有効時、残っているレガシープロンプト組み立てを `CognitionEngine` へ移行。
- ADR 違反の新規依存を入れない（`cargo tree -p ene-mind -p ene-tool-proto`）。

**影響クレート:** `ene-core`, `ene-mind`, `ene-mind`

---

### 3. API 形状の統一（フォローアップ）

- `McpToolRegistry::connect_stdio` のエラーを型付け（現状 `Result<(), String>`）。
- `AGENTS.md` §8 の PR チェックリストに async + 型付きエラー確認を追加。

**影響クレート:** `ene-tool-host`, `ene-provider`

---

### 4. イベント / セッション API 移行（Phase B–C）

- **Phase B:** 圧縮実行時に圧縮志向のイベントを出す（または `SessionSplit` / status を拡充）。
- **Phase C:** 感情エンジン有効時はエンジン管理の `Expression` を主経路に; インライン `<|emo:name|>` はレガシー / 助言扱い。
- `ene-cli` / `ene-desktop` が `EneEvent::Terminal` と `Expression` をすべて処理しているか確認。
- 任意: 圧縮が権威のとき分割スコアリングを無効化する feature / 設定。

**影響クレート:** `ene-core`, `ene-mind`, `ene-mind`, `ene-cli`, `ene-desktop`

---

### 5. ツール ABI 安定化（フォローアップ）

- 残りのツールバイナリ（`ene-tool-fs`, `ene-tool-web`, `ene-tool-app`, `ene-tool-browser`）を `ActionSetProvider` に移行。
- ハンドシェイク / バージョン拒否の proto 単体テストを追加。

**影響クレート:** `tools/*`, `ene-tool-common`, `ene-tool-proto`

---

## 各項目の完了条件

- コード変更 + `direnv exec .` 配下でテスト / clippy が緑
- 同一変更内で `docs/api/` と `docs/ja/api/`（および関連チュートリアル）を更新
- 新規の循環依存なし。`ene-store` が唯一の sea-orm 所有者のまま
- ツールのワイヤー変更は additive、または `PROTOCOL_VERSION` 上げを伴う

## 関連ドキュメント

- [API 索引](../api/index.md)
- [ストリーミングイベント](../core/streaming-events.md)
- [認知ランタイム ADR](cognitive-runtime.md)
- [アーキテクチャ概要](overview.md)
- [ツール SDK](../tools/sdk.md)
- [AGENTS.md](../../AGENTS.md)
