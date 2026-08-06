# `ene-rag` インターフェース

## 役割

RAG の**ポリシー**層: 純粋なスコアリング・減衰・ツール選択・ワークスペース
文書チャンク化。I/O もデータベースアクセスもありません。

## 公開モジュール

| モジュール | 内容 |
|---|---|
| `decay` | `half_life_decay`・`decay_score`・`recency_score`・`emotional_impact`・ライフサイクル閾値（`FADE_THRESHOLD`・`ARCHIVE_THRESHOLD`）と重み |
| `scoring` | `score_candidate`・`score_and_rank`・`relevance_score`・`lexical_overlap_score`・`emotional_match_score`・`relationship_score`・`contradiction_penalty`・`stale_penalty`・`access_boost_score`・`within_time_range`・`document_lexical_similarity` |
| `tool`（フィーチャー `tool`） | `ToolRag`・`ToolRagConfig`・`ToolRagOptions`・`ToolRagStats`・`hybrid_embed`・`hyde_document`・`rerank_tool_specs`・`FieldWeights` |
| `workspace` | `chunk_document`・`ChunkOptions`・`ChunkedDocument`・`DocumentChunk`・`score_chunk`・`glob_matches`・`WorkspaceRagConfig` |

## 依存関係

- 依存: `ene-core`・`ene-config`。`tool` フィーチャー有効時はさらに `ene-ai`・
  `ene-plugin-proto` など。
- 利用: `ene-store`（スコアリング核）・`ene-mind`（想起スコアリング）・
  `ene-runtime`（ツール選択・ワークスペースインデックス）。

## リファクタリングの注目点

- このクレートは**メモリ側とツール側のスコアリングポリシーが乖離しない**
  ために存在します。呼び出し側ではなくここにスコアリングを置いてください。
- `tool` フィーチャーが依存ゲートです。永続化・認知の呼び出し側はデフォルト
  （純粋）機能セットを使い、`tool` を有効化するのは `ene-runtime` だけです。
  デフォルトセットへの依存追加は埋め込みスタックを `ene-store` に漏らします。
- 減衰閾値と重みは意味が文書化されたマジック定数です。変更は想起挙動を
  グローバルに変えるため、固定するテストを維持してください。
- `glob_matches`（無視ルール）の意味論はテストで固定されています
  （basename 一致・末尾 `/**` はディレクトリ自体にも一致）。
