# AI-Core 概要

Ene の AI-Core は LLM との対話、ツール呼び出し、長期記憶、セッション管理を統合した Rust 製コアライブラリ。`ene-ai-core` クレートを中心に、ツール実行基盤・プロトコル・アプリケーションで構成される。

## クレート構成

| クレート | 役割 |
|----------|------|
| `ene-ai-core` | LLM連携、プロンプト/ストリーム、セッション/記憶の統合 |
| `ene-tool-host` | ツールプロセス管理、IPC接続、ToolRegistry |
| `ene-tool-proto` | IPC プロトコル定義（IpcRequest/IpcResponse）、ToolProvider trait |
| `ene-tools/*` | 個別ツールバイナリ（fs/web/utility/app/browser）、各1プロセス |
| `ene-desktop` | Bevy ベース GUI（VRM キャラクター表示、オーバーレイ） |
| `ene-cli` | インタラクティブ CLI（テスト・直接対話用） |

## 依存関係

```
ene-desktop ──┐
ene-cli ──┼── ene-ai-core ──── ene-tool-host ──── ene-tool-proto
          │                        │
          │                   ene-tools/* 各バイナリ（IPC接続）
          │
     ene-ai-core external deps:
       ├── async-openai  (LLM API)
       ├── tokio / tokio-stream / async-stream
       ├── diesel + sqlite-vec (memory)
       ├── rmcp (MCPクライアント)
       └── candle (GGUF埋め込み)
```
