# `ene-ai` & `ene-ai-local` — API リファレンス

> **クレート**: `ene-ai` (コアトレイト & クラウドプロバイダ) | `ene-ai-local` (`llama-cpp-4` による GGUF ローカル推論)

`ene-ai` および `ene-ai-local` は、Ene 向けに LLM チャット補完およびテキスト埋め込みベクトルの抽象化を提供します。

---

## 1. `ene-ai` (コアプロバイダライブラリ)

### コアトレイト
- `LlmProvider`: トークンのストリーミング生成をサポートする非同期 LLM チャット補完トレイト。
- `EmbeddingProvider`: テキスト埋め込みベクトル生成トレイト。

### 実装
- `OpenAiProvider`: OpenAI モデル (GPT-4o, text-embedding-3 など) 向けクラウドプロバイダ。
- `IpcLlmProvider`: プロバイダプラグイン (`ene-plugin-anthropic` など) からの IPC Protocol v4 メッセージを `LlmProvider` トレイトへ変換するホストアダプタ。
- `LlmProviderRegistry`: 設定に基づいてプロバイダを動的に生成・登録するファクトリ。

---

## 2. `ene-ai-local` (ローカル GGUF LLM 推論)

`ene-ai-local` は `llama-cpp-4` をラップしたローカルモデル実行エンジンを格納しています：

- **ローカルモデル読み込み**: ディスクから `.gguf` 重みファイルを読み込みます。
- **ハードウェアアクセラレーション**: Cargo feature (`vulkan` / `cuda`) でビルドされた GPU バックエンドと CPU を、`llama-cpp-4` バインディング経由で利用します。
- **プロセス内推論**: 外部ネットワーク通信を行うことなく、 `LlmProvider` を実装する `LocalLlmProvider` を提供します。

---

## 関連ドキュメント
- [設定リファレンス](../configuration.md)
- [システムアーキテクチャ](../architecture.md)
