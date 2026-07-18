# Tool RAG

Tool RAG（Retrieval-Augmented Generation）は、ベクトル埋め込みを使ってユーザークエリに最も関連するツールを選びます。すべてのツールを LLM に送るのではなく、上位 N 件だけをプロンプトに含めます。

インデックス用テキストは [`ToolRagProfile`](./sdk.md#toolragprofile)（#137）から取得します。LLM が受け取るのはスリムな [`ToolSpec`](./sdk.md#toolspec)（`name` / `description` / `parameters` のみ）です。

## 動作の流れ

```
ユーザークエリ
  ↓
1. クエリを埋め込み → query_embedding
  ↓
2. 各ツールについて重み付き類似度を計算:
   score = Σ (weight_i × cosine_sim(query, tool_field_i))
   フィールド: summary, description, capability, example, negative
  ↓
3. カテゴリ別上限を適用（例: Filesystem は最大 3 件）
  ↓
4. スコア順に並べ、top_k 候補を取得
  ↓
5. `use_rerank` かつ候補が複数なら任意の cosine embedding rerank → final_n を選択
  ↓
6. forced ツールはスコアに関係なく常に含める
  ↓
Vec<ToolSpec> → LLM に渡す
```

LLM HyDE は無効です（`use_hyde` は予約済み no-op）。リランクに LLM は使いません（description 埋め込みの cosine のみ）。

## 設定

`settings.json` の `tools.rag` で設定します（[設定リファレンス](../configuration/settings.md) を参照）。`tools.rag.enabled` が true で embedding プロバイダがあるときに構築されます。

| オプション | 型 | 既定 | 説明 |
|--------|------|---------|-------------|
| `enabled` | bool | `true` | Tool RAG パイプラインを有効化 |
| `use_hyde` | bool | `false` | 予約済み。LLM HyDE は無効（no-op） |
| `use_rerank` | bool | `false` | 候補の cosine 埋め込みリランク（LLM なし） |
| `background_index_on_startup` | bool | `true` | 起動時にバックグラウンドでインデックスをウォームアップ |
| `top_k` | int | `12` | リランク前の候補数 |
| `final_n` | int | `6` | LLM に送る最終ツール数 |
| `rerank_candidates` | int | `24` | embedding rerank で考慮する候補数 |
| `min_similarity` | float | `0.25` | 最小類似度しきい値 |
| `forced` | string[] | `["utility.question", "utility.todo_add", "utility.get_current_time"]` | 常に含めるツール |
| `per_category_limits` | map | `{}` | `ToolCategory::config_key` ごとの最大数（例: `"Filesystem"`） |

## マルチベクトル埋め込み

各ツールは `ToolRagProfile` の複数フィールドで埋め込まれ、`tool_embedding_index` に保存されます:

| フィールド | 内容 | 既定の重み |
|-------|---------|---------------|
| `summary` | `"{name}: {summary}"` | 1.0 |
| `description` | description + keywords + JSON Schema プロパティ要約 | 0.6 |
| `capability` | カテゴリラベル + summary + 主要キーワード | 0.8 |
| `example` | 例ごとに 1 行（`field_key = ex_N`） | 0.4 |
| `negative` | `"{name} NOT: {negative keywords}"` | -0.5（ペナルティ） |

バージョンハッシュはテキスト内容から導出されるため、内容が変わったツールだけ再埋め込みされます。`ensure_index(specs, profiles)` は両方の入力をハッシュします。

## Embedding Rerank

スコアリング後に複数候補が残る場合、クエリ埋め込みと各ツールの主要フィールド埋め込みのコサイン類似度でリランクし、`final_n` を選びます。決定論的で LLM は呼び出しません。

## フィールド重み

| 重み | 説明 |
|--------|-------------|
| `summary` | ツール summary の寄与度 |
| `description` | 全文 description の寄与度 |
| `capability` | capability 埋め込みの寄与度 |
| `example` | 例の寄与度 |
| `negative` | ネガティブキーワード一致のソフトペナルティ（既定 `-0.5`） |

## カテゴリ別上限

スコアリング後、カテゴリごとに出現数を制限できます（低スコアから削除）:

```json
{
  "per_category_limits": {
    "Filesystem": 3,
    "Browser": 2
  }
}
```

キーは `ToolCategory::config_key()`（`Filesystem`, `Shell`, `Browser`, `App`, `WebSearch`, `WebFetch`, `Utility`, `Memory`, `Search`, `Meta`）と一致させます。

## Forced ツール

`forced` に列挙されたツールは類似度に関係なく常に含まれます。既定の forced ツールは、LLM が常に使えるべき汎用ユーティリティです。

## アーキテクチャ

```
Tool binaries
  → ToolProvider::list_specs / list_rag_profiles
  → IpcToolRegistry (ListTools + ListRagProfiles, IPC v4)
  → CompositeToolRegistry
  → ToolRag::ensure_index(specs, profiles)
  → tool_embedding_index (SQLite)
  → ToolRag::select → Vec<ToolSpec> for the LLM
```

MCP ツールには著作用プロファイルがないため、ホストが各 `ToolSpec` から最小の `ToolRagProfile` を合成します。
