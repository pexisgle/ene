# クレートリファレンス

このページはワークスペース内のクレート・アプリ・プラグインバイナリの地図です。
依存規則もここにあります。動きの説明は [アーキテクチャ](../concepts/architecture.md)。
シグネチャは rustdoc (`cargo doc -p <crate> --open`) が正です。

## アプリケーション

| パッケージ | パス | 役割 |
|---|---|---|
| `ene-daemon` | `apps/ene-core`（バイナリ `ene-core`） | コアデーモン: データディレクトリのロック、HTTP/WS API、session + kernel + companion + work + plane + fiber |
| `ene-ctl` | `apps/ene-ctl` | 同一 HTTP/WS API の CLI クライアント |
| `ene-stage` | `apps/ene-stage` | 製品 GUI: wgpu オーバーレイ、チャット、8セクションの詳細、トレイ。表層 + 詳細の 2 本のソケット |

## ライブラリクレート

| クレート | 役割 | 主な内部依存 |
|---|---|---|
| `ene-session` | 追記専用の会話ログ、usage 台帳、履歴投影 | config |
| `ene-kernel` | 対話レーン: prompt / steer / follow_up / abort / compact、可視性、可観測性 | config, session |
| `ene-companion` | soul・感情・記憶・内面・能動発話・キャラパッケージ | card, config, plane, registry, session |
| `ene-body` | パフォーマンスキュー、感情→表情、全二重音声 | config, session |
| `ene-work` | 委譲、ジョブ、スケジュール、skill、MCP | companion, kernel, plane, registry, session |
| `ene-plane` | 承認 plane、hash chain 監査、資格情報ボールト | config |
| `ene-fiber` | プラグインファイバー合成: 巻き戻し可能な effect、プロファイル reconcile、サンドボックス spawn | plugin-ipc, registry, sandbox |
| `ene-registry` | 統一ツールレジストリ: side_effects フィルタ、deny-by-default パイプライン | plugin-ipc, plane |
| `ene-plugin-ipc` | 分割 IPC: core / tool 副プロトコルの length-prefixed MessagePack | （内部依存なし） |
| `ene-api` | HTTP/WS 型、OpenAPI、Rust クライアント | （内部依存なし） |
| `ene-card` | Character Card V3 / PNG / CHARX インポート | config |
| `ene-config` | 設定の load/save/schema、パス、`define_config!` | （内部依存なし） |
| `ene-sandbox` | OS サンドボックス（Linux では Landlock + seccomp + rlimits） | （内部依存なし） |
| `ene-vrm` | VRM 1.0 ローダ + wgpu レンダラ | （内部依存なし） |

## 依存規則（レビューで強制）

```text
ene-session     ↛ kernel, companion, work, daemon
ene-kernel      ↛ companion, work, fiber, daemon
ene-companion   ↛ daemon, fiber
ene-plugin-ipc  ↛ business logic
ene-card        → ene-config のみ（逆辺は禁止）
ene-vrm         ↛ kernel, companion, work, session
ene-api         ↛ daemon types
```

クライアントは `ene-api` 経由でのみデーモンと話します。`ene-stage` の本番
コードから `ene-daemon` をリンクしないでください（`ene-ctl` のテストは
デーモンを spawn してよい）。

## プラグインバイナリ

### ツールプラグイン (`plugins/tool/*`)

`fs`、`exec`、`web`、`utility` — [同梱ツール](../guides/tools/builtin-tools.md)。
`exec` は `fs` とは別プラグインです（D-24）。

Python のダミー (`plugins/tool/dummy-py`) は IPC 用フィクスチャのみで、Cargo
ワークスペースからは除外されています。
