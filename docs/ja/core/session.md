# セッション管理

`ConversationSession` はアクティブ会話の実行状態を保持する中核コンテナです。`EneActor` が所有し、各ストリーミング実行で利用されます。

## セッションが保持するもの

- Prompt 構築に使う会話履歴
- ストリーミング表示バッファとトークン carry
- メモリ文脈ハンドル（`MemoryStore`、埋め込み、`session_id`）
- 読み込み済みキャラクターカード状態

## Cognitive Runtime 時の挙動

`cognition.enabled=true` では、セッション状態は `CognitionEngine` と組み合わせて処理されます。

1. `before_turn` で recall 計画と affect 更新
2. `compose_prompt_packet` でセクション化プロンプト生成
3. `after_turn` で typed memory 保存と affect 永続化

この設計により、コンテキスト圧力は圧縮で吸収しつつ、会話継続性を維持します。

## session_id と継続性

- `session_id` はセッション開始時に生成され、以下の結び付けに使われます。
  - raw conversation logs
  - compression spans（`memory_spans`）
  - cognition tracing/debug
- cognitive compression モードでは、古いターンを圧縮しても `session_id` は維持されます。

## セッション内の CharacterCardV3

セッションは読み込まれた `CharacterCardV3` を保持し、毎ターン以下に利用します。

- Identity Kernel のコンパイルと注入
- lorebook/style example の想起
- Output Arbiter 用の表情定義解決

## 関連ドキュメント

- `docs/ja/architecture/cognitive-runtime.md`
- `docs/ja/core/prompt.md`
- `docs/ja/core/session-split.md`
