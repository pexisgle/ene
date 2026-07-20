# Tool Semantic RAG Retrieval Specifications (`ene-tool-rag`)

The `ene-tool-rag` crate implements the tool RAG (Retrieval-Augmented Generation) pipeline. It dynamically selects the most relevant tools for a given user query to optimize LLM system prompt sizes.

---

## 1. Data Structures

### `FieldWeights` (Public / Struct)
Weights used to scale similarities across different tool metadata vector fields:
*   `summary: f32`: Weight for the tool summary embedding (defaults to 1.0).
*   `description: f32`: Weight for the detailed behavior description (defaults to 0.6).
*   `capability: f32`: Weight for the capability description (defaults to 0.8).
*   `example: f32`: Weight for execution examples (defaults to 0.4).
*   `negative: f32`: **Negative Match Penalty**. Deducts points if the query matches negative/unwanted behaviors defined on the tool (defaults to -0.5).
*   *(Note)*: LLM `HyDE` is deprecated and disabled (no-op).

---

## 2. Selection Pipeline

`ToolRag::select` filters and ranks tools using the following sequence:

1.  **Query Embedding**:
    -   Passes the active user query to `EmbeddingProvider` to calculate its search vector.
2.  **Multi-Vector Search**:
    -   Queries the `tool_embedding_index` table inside the `MemoryStore`.
    -   Computes cosine similarities for all registered tools across fields (summary, description, capability, example, negative).
    -   Applies `FieldWeights` to calculate the final aggregate score for each candidate tool.
3.  **Threshold Pruning**:
    -   Filters out tools with scores below `min_similarity` (defaulting to 0.25).
4.  **Forced Inject Merger**:
    -   Merges the list with core tools that must always bypass RAG filtering (such as `utility.question` and `utility.get_current_time`).
5.  **Category Caps (`per_category_limits`)**:
    -   Trims tool counts per category key (e.g. capping file tools or web search tools) to promote diversity and prevent single-domain tools from filling the prompt context.
6.  **Cutoff Limits (`final_n`)**:
    -   Sorts remaining candidates by score and returns the top `final_n` (defaulting to 6) `ToolSpec` definitions.
