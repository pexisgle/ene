# ツール RAG (Tool RAG)

ツール RAG (Retrieval-Augmented Generation) は、ベクトル埋め込み (embeddings) を使用して、ユーザーの入力クエリに対して最も関連性の高いツールを選択します。すべてのツールを LLM に送信するのではなく、最も関連性の高い上位 N 個のツールのみがプロンプトに含まれます。

インデックス化テキストは [`ToolRagProfile`](./sdk.md#toolragprofile) (#137) から取得されます。LLM には slim 化された [`ToolSpec`](./sdk.md#toolspec)（`name`、`description`、`parameters` のみ）が渡されます。

## 仕組み (How It Works)

```
User query
  ↓
1. Embed query → query_embedding
  ↓ (optional) HyDE: generate hypothetical answer → embed → hyde_embedding
  ↓
2. For each tool, compute weighted similarity:
   score = Σ (weight_i × cosine_sim(query_embedding, tool_field_i))
   where fields are: summary, description, capability, example, negative, hyde
  ↓
3. Apply per-category limits (e.g. max 3 Filesystem tools)
  ↓
4. Sort by score, take top_k candidates
  ↓
5. (optional) LLM rerank the top_k → pick final_n
  ↓
6. Always include forced tools regardless of score
  ↓
Vec<ToolSpec> → passed to LLM
```

## 設定 (Configuration)

`settings.json` の `tools.rag` セクション（設定キー `rag`、パス `["tools", "rag"]`）:

```json
{
  "tools": {
    "rag": {
      "enabled": true,
      "top_k": 12,
      "final_n": 6,
      "use_hyde": true,
      "use_rerank": true,
      "rerank_candidates": 24,
      "min_similarity": 0.25,
      "background_index_on_startup": true,
      "forced": [
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
        "hyde": 0.7,
        "hyde_blend": 0.6
      },
      "per_category_limits": {
        "Filesystem": 3
      }
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
| `background_index_on_startup` | bool | `true` | ランタイム bootstrap 時にツール embedding index をウォームアップする |
| `forced` | string[] | `["utility.question", "utility.todo_add", "utility.get_current_time"]` | 常に含めるツール |
| `per_category_limits` | map | `{}` | `ToolCategory::config_key` ごとの最大ツール数（例: `"Filesystem"`） |

## マルチベクトル埋め込み (Multi-Vector Embedding)

各ツールは `ToolRagProfile` の複数フィールドにわたって埋め込まれ、`tool_embedding_index` に保存されます:

| フィールド | 内容 | デフォルトの重み |
|-------|---------|---------------|
| `summary` | `"{name}: {summary}"` | 1.0 |
| `description` | description + keywords + JSON Schema property summary | 0.6 |
| `capability` | category label + summary + primary keywords | 0.8 |
| `example` | 例ごとに1行（`field_key = ex_N`） | 0.4 |
| `negative` | `"{name} NOT: {negative keywords}"` | -0.5 (ペナルティ) |

バージョンハッシュはテキストコンテンツから派生するため、ツールはその内容が変更されたときにのみ再埋め込みされます。`ensure_index(specs, profiles)` は両方の入力をハッシュします。

## HyDE (Hypothetical Document Embeddings)

`use_hyde = true` の場合、パイプラインは以下を行います：
1. LLM を使用してユーザーのクエリに対する仮想の回答 (hypothetical answer) を生成する
2. その仮想の回答を埋め込む (ベクトル化)
3. `weights.hyde` と `weights.hyde_blend` 経由でクエリと HyDE の類似度をブレンドする

これにより、ツール名を直接指定するのではなく、達成したい目的を記述しているクエリの検出精度 (recall) が向上します。

## フィールドの重み (Field Weights)

| 重み | 説明 |
|--------|-------------|
| `summary` | ツールの概要がスコアに寄与する度合い |
| `description` | 詳細な説明がスコアに寄与する度合い |
| `capability` | capability 埋め込みがスコアに寄与する度合い |
| `example` | 例 (examples) がスコアに寄与する度合い |
| `negative` | ネガティブキーワード一致に対する緩やかなペナルティ（デフォルト `-0.5`） |
| `hyde` | HyDE 埋め込みがスコアに寄与する度合い |
| `hyde_blend` | HyDE と直接類似度のスコア配分（`0.0`–`1.0`） |

## カテゴリごとの制限 (Per-Category Limits)

スコアリング後に各カテゴリから含められるツール数を制限します（スコアが低いものから除外）:

```json
{
  "per_category_limits": {
    "Filesystem": 3,
    "Browser": 2
  }
}
```

キーは `ToolCategory::config_key()` と一致させる必要があります（`Filesystem`、`Shell`、`Browser`、`App`、`WebSearch`、`WebFetch`、`Utility`、`Memory`、`Search`、`Meta`）。

## 強制ツール (Forced Tools)

`forced` にリストされたツールは、類似度スコアに関係なく常に含まれます。デフォルトの強制ツールは、LLM が常にアクセスできるべき汎用ユーティリティです。

## アーキテクチャ (Architecture)

```
Tool binaries
  → ToolProvider::list_specs / list_rag_profiles
  → IpcToolRegistry (ListTools + ListRagProfiles, IPC v4)
  → CompositeToolRegistry
  → ToolRag::ensure_index(specs, profiles)
  → tool_embedding_index (SQLite)
  → ToolRag::select → Vec<ToolSpec> for the LLM
```

MCP ツールには authoring プロファイルがないため、ホストは各 `ToolSpec` から最小限の `ToolRagProfile` を合成します。
