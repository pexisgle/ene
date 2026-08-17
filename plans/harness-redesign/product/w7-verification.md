# W7 検証記録(v1.0 ハーネス)

> 実装ウェーブ W0–W6 の上で、[done.md](done.md) の総括条件と
> [process-model.md §6](../platform/process-model.md#6-性能予算) の
> 性能基準線を観測した記録。後継設計(resume / 3ストア / marketplace /
> MCP 導線 UI / 署名)は対象外。

測定日: 2026-08-17。対象は現行アプリと並べて追加した新クレート
(`ene-session` / `ene-kernel` / `ene-daemon` / `ene-api` / `ene-ctl` /
`ene-stage` ほか)。旧 `ene-desktop` の置き換えはしていない。

## 総括条件

| # | 条件 | 新ハーネスでの観測 |
|---|---|---|
| 1 | stage + CLI + Web が同一コアに接続 | 成立。`three_clients_share_one_core`。クライアント id `stage` / `cli` / `web` が同じ `GET /health` と OpenAPI を共有する |
| 2 | 1 体が done.md の全 P-xxx を満たす | **未達**。クレート単体テストは P-2xx/P-3xx/P-5xx/P-6xx/P-9xx の多くを固定するが、新 `ene-stage` はテキストの表層+詳細の 2 窓であり、VRM 同時表示・全二重 GUI・2 体 stage 表示は旧 desktop / `ene-body` 側に残る |
| 3 | ネットワークなしで会話 | 成立(`EchoModel`)。実ローカル GGUF 経路はプロバイダプラグインの後続接続。`spawned_core_offline_conversation_and_rss` |
| 4 | `nix develop` からのビルドと性能 | 性能観点は §基準線。Cloud Agent VM は Nix を使わず `cargo` 直接(AGENTS.md)。数値は許容範囲として記録した |
| 5 | 監査・バックアップ・エクスポート | 成立。hash chain、`POST /backup`、default export が inner を含まないこと、restore roundtrip |

## 固定した不変条件(テスト)

- 表層 WS / 履歴 / default export に inner が乗らない。詳細では読める
- 中断は検出・片付け・報告。自動 resume しない
- 同時 prompt は `lane_busy`。承認と mic は first-writer
- Web UI は memory/settings の PATCH/DELETE を持たない。stage は eframe+wgpu で WebView なし
- スパン属性にプロンプト内容が乗らない
- 空トークンは `/api/v1/health` 以外 `unauthorized`

## 性能基準線

正は `minimal`(描画なし / EchoModel / debug)。数値と CI 上限は
[process-model.md §6.1](../platform/process-model.md#61-v10-基準線d-29)。

## 意図的にやらないこと(W7)

- `resume` / `lane.last_result` / OperationState / effect sandwich
- キャラ署名・marketplace・MCP 接続導線 UI
- `docs/` と `AGENTS.md` の新レイアウトへの書き換え
- 旧 `ene-desktop` / `ene-cli` の削除
