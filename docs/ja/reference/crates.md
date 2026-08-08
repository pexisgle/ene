# クレートリファレンス

このページはワークスペース内の全クレート・アプリ・プラグインバイナリと、
アーキテクチャを維持する依存ルールの正規マップです。全体の仕組みは
[アーキテクチャ](../concepts/architecture.md) を参照してください。
各クレートの**公開インターフェース**（モジュール・型・トレイト・
リファクタリングの継ぎ目）は[クレートインターフェース](interfaces/overview.md)を
参照してください。

## アプリケーション

| パッケージ | パス | 役割 |
|---|---|---|
| `ene-desktop` | `apps/ene-desktop` | GUI アプリ: winit + wgpu + egui + bevy_ecs。アバター・チャット・音声・トレイ・設定。`voice` フィーチャー（デフォルト）が cpal/rodio の音声をゲート。 |
| `ene-cli` | `apps/ene-cli` | 対話型 REPL + スクリプト用非対話サブコマンド。 |

## ライブラリクレート

| クレート | 役割 | 主な内部依存 |
|---|---|---|
| `ene-runtime` | アクターベースのホストファサード: `EneHandle`・ターン制御・3 チャネルイベントバス・ツール/スケジュール/undo/ワークスペースハンドル・API v1 ミラー | mind, store, ai, plugin-host, rag, config, card, connector, core |
| `ene-mind` | 認知エンジン: プロンプトパケット・想起・メモリ書き込み/仲裁・感情・プロアクティブ・セッション・約束・要約 | core, config, card, ai, rag, util |
| `ene-store` | SQLite/SeaORM の唯一の所有者: スキーマ・マイグレーション・sqlite-vec 検索・バックアップ・監査・DB IPC サーバー（`db` ホストサービス） | config, core, rag, plugin-db, plugin-proto |
| `ene-core` | 永続化非依存のドメイン語彙 + `MemoryPort`/`EmbeddingStorePort`/`WorkspaceDocumentPort` トレイト | （内部なし） |
| `ene-card` | キャラクターカードコンテナ（V3）・PNG/CHARX インポート/エクスポート・キャラクター別設定・ローカライズ済みカード差分 | config |
| `ene-config` | 設定の読み書き・スキーマ・パス・プロンプト/パターン・`define_config!` マクロ | （内部なし） |
| `ene-ai` | LLM/埋め込み/STT/TTS/VAD トレイト・タスクルーティング・リトライ・コンテキスト窓計算・モデル取得 | config, infer, plugin-proto |
| `ene-infer` | シングルスレッドローカルモデルフレームワーク（`LocalModel`・`EngineHandle`）: ワーカースレッド・有界キュー・協調キャンセル・パニック回復 | （内部なし） |
| `ene-rag` | RAG ポリシー: ハイブリッドスコアリング・減衰・ワークスペースチャンク化。`tool` フィーチャーでツール選択パイプライン（ene-ai が必要） | core, config |
| `ene-connector` | 外部サービス接続フレームワーク: 資格情報・権限ゲート・ポリシー・webhook | （内部なし） |
| `ene-plugin-proto` | ワイヤ ABI: IPC プロトコル v7・ツール型・capability・サンドボックス設定 | （内部なし） |
| `ene-plugin` | プラグイン作成ファサード: `run_plugin_server`・`PluginDispatch`・トレイト・`prelude` | proto, infer, macros |
| `ene-plugin-macros` | プロシージャルマクロ: `ToolAction`・`ToolSpec`・`tool_action`・プロバイダー derive | proto |
| `ene-plugin-host` | プラグイン監視: 起動/ハンドシェイク/capability/ヘルス/サーキットブレーカー・IPC プロバイダーブリッジ・MCP クライアント・資格情報レジストリ | proto, ai, config, connector |
| `ene-plugin-db` | ホスト `db` サービス上のプラグイン向け型付き CRUD クライアント | proto |
| `ene-voice` | ローカル音声エンジン: whisper STT・Kokoro TTS・Silero VAD（フィーチャー: `local-stt`・`local-tts`・`silero-vad`） | ai, config, infer |
| `ene-vrm` | VRM 1.0 ローダー + wgpu レンダラー。独立（ene-desktop が使用） | （内部なし） |
| `ene-util` | 純粋ヘルパー: truncate・HTML→Markdown（フィーチャー `html`） | （内部なし） |

## 依存ルール（レビューで強制・CI で検証）

```text
ene-core    ← ene-store, ene-mind, ene-rag     （語彙とポート）
ene-store   ↛ ene-ai, ene-mind, ene-runtime    （永続化は純粋のまま）
ene-mind    ↛ ene-runtime, ene-plugin-host     （本番コード）;
              永続化は ene_core::MemoryPort 経由のみ
ene-rag     ↛ ene-store, ene-mind              （ポリシー層。循環は構造的に不可能）
ene-card    → ene-config                       （エラー/パス/言語エイリアスのみ。
                                                逆方向の辺は作らない）
ene-plugin-proto ↛ ビジネスロジック             （ワイヤ ABI のみ）
ene-vrm     ↛ ene-mind, ene-runtime, ene-store （レンダラーは独立）
```

このエッジの違反は、このリポジトリで最もよく起きる破壊の原因です。

## プラグインバイナリ

### ツールプラグイン（`plugins/tool/*`）

`app`・`browser`・`calc`・`calendar`・`counter`・`fs`・`geo`・`git`・
`homeassistant`・`random`・`utility`・`web` — [同梱ツール](../guides/tools/builtin-tools.md)参照。

### プロバイダープラグイン（`plugins/provider/*`）

`openai`・`anthropic`・`local-llm`（llama.cpp。バイナリ名
`ene-plugin-llama-cpp`）・`llama-server`・`onnx`（Silero VAD）・`whisper`
（whisper.cpp）・`kokoro`（ONNX TTS）・`edge-tts`・`elevenlabs`・`openai-tts`・
`voicevox` — [プラグインと MCP](../concepts/plugins-and-mcp.md)参照。

## 重要なフィーチャーフラグ

| フィーチャー | 所有者 | 効果 |
|---|---|---|
| `tool` | `ene-rag` | ツール選択 RAG パイプライン（ene-ai/plugin-proto を導入）。`ene-runtime` が有効化。 |
| `local-stt` / `local-tts` / `silero-vad` | `ene-voice` | ネイティブ whisper/ONNX エンジン。プロバイダープラグインが消費。 |
| `voice` | `ene-desktop` | マイクキャプチャ+再生（cpal/rodio）。オフ ⇒ 無機能スタブ。 |
| `test-util` | `ene-infer` | `LocalModel` 実装の適合性テスト一式。 |
| `html` / `truncate` | `ene-util` | HTML→Markdown / truncate ヘルパー。 |

## ビルドと CI

- `default-members = ["apps/ene-cli"]` — 素の `cargo test`/`cargo clippy` は
  CLI だけを対象にします。常に `--workspace` か `-p <pkg>` を指定してください。
- CI ゲート: `cargo clippy --workspace --all-targets -- -D warnings`・
  `cargo test --workspace`・`cargo doc --workspace --no-deps`。
- リントが仕様です: `all`/`pedantic`/`cargo` を deny し、さらに
  `unwrap_used`・`expect_used`・`panic`・`todo`・`dbg_macro` などを deny。
  例外は `#[expect(lint, reason = "...")]` に限られます（`#[allow]` は
  拒否されます）。
- ネイティブ依存はチェックイン済み Nix flake から。Windows は Linux から
  クロスコンパイル（mingw）。macOS は非対応。
