# `ene-ai` & `ene-ai-local`

> **クレート**: `ene-ai` (プロバイダトレイト、メッセージ/ストリーミング型、OpenAI 互換プロバイダ、レジストリ) | `ene-ai-local` (`llama-cpp-4` による GGUF ローカル推論)

`ene-ai` と `ene-ai-local` は Ene 向けに LLM チャット補完およびテキスト埋め込みの抽象化を提供します。`ene-ai` は汎用のメッセージ/ストリーミング型・プロバイダトレイト・グローバルなプロバイダレジストリ・組み込みの OpenAI 互換実装を定義します。ローカル推論 (GGUF/llama.cpp) は `ene-ai-local` に、ローカル音声 (STT/TTS/VAD) は別クレートの `ene-voice` にあります。

---

## アーキテクチャ境界

- `ene-ai` はプロバイダ抽象化層を所有します: メッセージ/ストリーミング型、プロバイダトレイト、ヘルスモニタリング/フェイルオーバールーティング、リトライポリシー。永続化や認知ロジックへの依存はありません。
- プロセス外の LLM プロバイダ (例: Anthropic プラグイン) は `ene-ai` 自身ではなく `ene-plugin-host` が所有する IPC アダプタを介して同じ `LlmProvider` トレイトへブリッジされます。`ene-ai` はアダプタが実装するトレイトを定義するだけです。
- `ene-ai-local` は自身のプロバイダトレイトを実装するために `ene-ai` と `ene-config` に依存し、プロセス内で推論を実行します — ネットワーク呼び出しはありません。

## 設計思想

- **なぜ具象クライアント型ではなくプロバイダトレイトか**: `LlmProvider`/`EmbeddingProvider` により、クラウドプロバイダ (OpenAI 互換)、ローカル GGUF 推論、IPC 経由のプロセス外プラグインプロバイダのすべてが同一インターフェースを満たせます。これにより `ene-mind`/`ene-runtime` 側の補完ストリーミングや埋め込みコードは、どのバックエンドが応答しているかを知る必要がありません。
- **なぜローカル推論が別クレートか**: `llama-cpp-4` は GPU バックエンドのビルド複雑性 (`vulkan`/`cuda` Cargo feature) を持ち込みますが、クラウドのみのデプロイにはこれは不要です。分離することで、リモートプロバイダのみを使用する利用者向けに `ene-ai` を軽量に保てます。

## API リファレンス

構造体・メソッドのシグネチャはここには転記しません — 転記すると必ず陳腐化します。最新かつ正確な API は rustdoc を生成して参照してください:

```sh
cargo doc -p ene-ai --open
cargo doc -p ene-ai-local --open
```

`ene-ai` の `LlmProvider` / `EmbeddingProvider` トレイトから始めてください。

---

## 関連ドキュメント
- [設定リファレンス](../configuration.md)
- [システムアーキテクチャ](../architecture.md)
