# クレートリファレンス

このページはワークスペース内のクレート・アプリ・プラグインバイナリの地図です。
依存規則もここにあります。動きの説明は [アーキテクチャ](../concepts/architecture.md)。
シグネチャは rustdoc (`cargo doc -p <crate> --open`) が正です。

## アプリケーション

| パッケージ | パス | 役割 |
|---|---|---|
| `ene-core` | `apps/ene-core`（バイナリ `ene-core`） | コアプロセス: データディレクトリのロック、HTTP/WS API、session + kernel + companion + work + access-control + plugin-host |
| `ene-stage` | `apps/ene-stage` | 製品 GUI: wgpu オーバーレイ（Slint を GPU 合成）、チャット、9 セクションの詳細、トレイ。表層 + 詳細の 2 本のソケット（`client_id = stage`） |
| `ene-ctl` | `apps/ene-ctl` | 同一 HTTP/WS API の CLI クライアント |
| `ene-desktop` | `apps/ene-desktop` | 凍結した再設計前 GUI（#794 で復元）。機能追加なし。stage が代替できたと判断したら削除 |

## ライブラリクレート

| クレート | 役割 | 主な内部依存 |
|---|---|---|
| `ene-session` | 追記専用の会話ログ、usage 台帳、履歴投影 | config |
| `ene-kernel` | 対話レーン: prompt / steer / follow_up / abort / compact、可視性、可観測性 | config, session |
| `ene-companion` | soul・感情・記憶・内面・能動発話・キャラパッケージ | card, config, access-control, tool-registry, session |
| `ene-body` | パフォーマンスキュー、感情→表情、全二重音声 | config, session |
| `ene-work` | 委譲、ジョブ（ジョブレーンのランナー）、スケジュール、skill（カタログ/発動コンテキスト、しおり workflow）、MCP | companion, kernel, access-control, tool-registry, session |
| `ene-access-control` | 承認、hash chain 監査、資格情報ボールト | config |
| `ene-plugin-host` | プラグイン監督: 巻き戻し可能な effect、プロファイル reconcile、サンドボックス spawn | plugin-ipc, tool-registry, sandbox, kernel |
| `ene-tool-registry` | 統一ツールレジストリ: side_effects フィルタ、deny-by-default パイプライン | plugin-ipc, access-control |
| `ene-plugin-ipc` | 分割 IPC: core / tool 副プロトコルの length-prefixed MessagePack | （内部依存なし） |
| `ene-provider-assets` | provider 向け共有アセットカタログ、manifest、検証付きダウンロード | config, plugin-ipc |
| `ene-api` | HTTP/WS 型、OpenAPI、Rust クライアント | （内部依存なし） |
| `ene-card` | Character Card V3 / PNG / CHARX インポート | config |
| `ene-config` | 設定の load/save/schema、パス、`define_config!` | （内部依存なし） |
| `ene-sandbox` | OS サンドボックス（Linux では Landlock + seccomp + rlimits） | （内部依存なし） |
| `ene-vrm` | VRM 1.0 ローダ + wgpu レンダラ | （内部依存なし） |
| `ene-stage-ui` | Stage 用の生成 Slint バインディング。slint-build の unwrap が workspace clippy を落とさないよう隔離 | （内部依存なし） |

## 依存規則（レビューで強制）

```text
ene-session     ↛ kernel, companion, work, core
ene-kernel      ↛ companion, work, plugin-host, core
ene-plugin-host → kernel（共有 `LoopHooks` のみ。kernel ↛ plugin-host は維持）
ene-companion   ↛ core, plugin-host
ene-plugin-ipc  ↛ business logic
ene-card        → ene-config のみ（逆辺は禁止）
ene-vrm         ↛ kernel, companion, work, session
ene-api         ↛ core types
```

クライアントは `ene-api` 経由でのみコアと話します。`ene-desktop` と
`ene-stage` の本番コードから `ene-core` をリンクしないでください
（`ene-ctl` のテストはコアを spawn してよい）。

どのクライアントが製品 GUI か、旧ツールがどこへ行ったかは
[製品境界](../concepts/product-boundaries.md) にあります。

## プラグインバイナリ

### ツールプラグイン (`plugins/tool/*`)

`fs`、`exec`、`web`、`utility`、`app`、`mcp` — [同梱ツール](../guides/tools/builtin-tools.md) と
[MCP サーバー](../guides/tools/mcp-servers.md) を参照してください。
`exec` は `fs` とは別プラグインです（D-24）。

Python のダミー (`plugins/tool/dummy-py`) は IPC 用フィクスチャのみで、Cargo
ワークスペースからは除外されています。

### Provider プラグイン (`plugins/provider/*`)

現在のワークスペースには `openai-compat`、`anthropic`、`gguf`、`elevenlabs`、
`voicevox`、`edge-tts` が含まれます。いずれも同じ plugin IPC を介して
out-of-process で動作し、LLM・embedding・TTS・STT などの seam を公開します。
