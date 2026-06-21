# ツール RAG (Tool RAG)

ツール RAG (Retrieval-Augmented Generation) は、ベクトル埋め込み (embeddings) を使用して、ユーザーの入力クエリに対して最も関連性の高いツールを選択します。すべてのツールを LLM に送信するのではなく、最も関連性の高い上位 N 個のツールのみがプロンプトに含まれます。

## 仕組み (How It Works)

```
User query
  ↓
1. Embed query → query_embedding
  ↓ (optional) HyDE: generate hypothetical answer → embed → hyde_embedding
  ↓
2. For each tool, compute weighted similarity:
   score = Σ (weight_i × cosine_sim(query_embedding, tool_field_i))
   where fields are: summary, description, negative, hyde
  ↓
3. Apply per-category limits (e.g. max 3 filesystem tools)
  ↓
4. Sort by score, take top_k candidates
  ↓
5. (optional) LLM rerank the top_k → pick final_n
  ↓
6. Always include forced_tools regardless of score
  ↓
Vec<ToolSpec> → passed to LLM
```

## 設定 (Configuration)

`settings.json` の `tools` セクション以下の設定例：

```json
{
  "tools": {
    "tool_rag": {
      "enabled": true,
      "top_k": 12,
      "final_n": 6,
      "use_hyde": true,
      "use_rerank": true,
      "rerank_candidates": 24,
      "min_similarity": 0.25,
      "background_index_on_startup": false,
      "forced_tools": [
        "utility.question",
        "utility.todo_add",
        "utility.get_current_time"
      ],
      "weights": {
        "summary": 1.0,
        "description": 0.6,
        "capability": 0.8,
        "example": 0.4,
        "negative": -0.5,
        "hyde": 0.7
      },
      "per_category_limits": {}
    }
  }
}
```

### オプション (Options)

| オプション | 型 | デフォルト | 説明 |
|--------|------|---------|-------------|
| `enabled` | bool | `true` | ツール RAG の有効化/無効化 |
| `top_k` | int | `12` | リランク前の候補数 |
| `final_n` | int | `6` | LLM に送信される最終的なツール数 |
| `use_hyde` | bool | `true` | HyDE (Hypothetical Document Embeddings) を使用する |
| `use_rerank` | bool | `true` | 上位候補に対して LLM リランクを使用する |
| `rerank_candidates` | int | `24` | LLM リランク対象の候補数 |
| `min_similarity` | float | `0.25` | 最小類似度の閾値 |
| `background_index_on_startup` | bool | `false` | 起動時にバックグラウンドでツールのインデックスを作成する |
| `forced_tools` | string[] | `["utility.question", "utility.todo_add", "utility.get_current_time"]` | 常に含めるツール |

## マルチベクトル埋め込み (Multi-Vector Embedding)

各ツールは複数のフィールドにわたって埋め込まれ、`tool_embedding_index` に個別に保存されます。

| フィールド | 内容 | デフォルトの重み |
|-------|---------|---------------|
| `summary` | `"tool.name: 1行の概要"` | 1.0 |
| `description` | 詳細な説明 + キーワード | 0.6 |
| `negative` | `"tool.name NOT: ネガティブキーワード"` | -0.5 (ペナルティ) |

バージョンハッシュはテキストコンテンツから派生するため、ツールはその内容が変更されたときにのみ再埋め込みされます。

## HyDE (Hypothetical Document Embeddings)

`use_hyde = true` の場合、パイプラインは以下を行います：
1. LLM を使用してユーザーのクエリに対する仮想の回答 (hypothetical answer) を生成する
2. その仮想の回答を埋め込む (ベクトル化)
3. スコアリングのために、元のクエリの埋め込みと HyDE の埋め込みの両方を使用する

これにより、ツール名を直接指定するのではなく、達成したい目的を記述しているクエリの検出精度 (recall) が向上します。

## フィールドの重み (Field Weights)

| 重み | 説明 |
|--------|-------------|
| `summary` | ツールの概要がスコアに寄与する度合い |
| `description` | 詳細な説明がスコアに寄与する度合い |
| `capability` | 機能ベースのマッチング用に予約されている重み |
| `example` | 例 (examples) がスコアに寄与する度合い |
| `negative` | 除外用のキーワードマッチングに対するペナルティ (負の値の場合は緩やかなペナルティ) |
| `hyde` | HyDE 埋め込みがスコアに寄与する度合い |

緩やかなペナルティ (ツールは表示されるが、順位が下がる) にするには `negative < 0` を設定します。強制的な除外 (ツールを候補からドロップする) には `negative > 0` を設定します。

## カテゴリごとの制限 (Per-Category Limits)

最終的なツールセットに含まれるカテゴリごとの最大ツール数を制限できます：

```json
{
  "per_category_limits": {
    "filesystem": 3,
    "browser": 2
  }
}
```

## 強制ツール (Forced Tools)

`forced_tools` にリストされたツールは、類似度スコアに関係なく常に含まれます。デフォルトの強制ツールは、LLM が常にアクセスできるべき汎用ユーティリティです。

## アーキテクチャ (Architecture)

```
ToolRag
  ├── embedder: Arc<dyn EmbeddingProvider>
  ├── store: Option<Arc<MemoryStore>>
  ├── opts: ToolRagOptions
  └── specs: RwLock<HashMap<ToolName, ToolSpec>>

MemoryStore.tool_embedding_index
  ├── tool_name (TEXT)
  ├── field (TEXT: "summary" | "description" | "negative")
  ├── field_key (TEXT: ToolSpec の場合は空文字、ActionSpec の場合はアクション名)
  ├── version_hash (TEXT: コンテンツから派生)
  ├── model_name (TEXT)
  └── embedding (f32 blob)
```

## デバッグ (Debugging)

CLI の `/memory search` コマンドを使用して、ツールの埋め込みをテストします：

```
/memory search "read a file"
```

これにより、クエリに一致するツールとその類似度スコアが表示されます。
