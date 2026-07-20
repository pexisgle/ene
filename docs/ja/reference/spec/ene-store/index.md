# `ene-store` クレート概要 & スキーマ設計仕様

`ene-store` クレートは、Ene ワークスペース唯一の永続化レイヤーです。SQLite データベース（`memory.db`）のコネクション管理、スキーマ移行（Migrations）、およびベクトル拡張モジュール（`sqlite-vec`）を用いた近傍ベクトル類似度検索（Cosine Similarity）の実装を担当します。

---

## 1. 依存関係と境界

### 物理的依存関係 (`Cargo.toml`)
- **依存先**: `sea-orm`, `sea-orm-migration`, `libsqlite3-sys`, `sqlite-vec`, `tokio`, `chrono`, `serde`
- **内部クレート依存先**: `ene-config`
- **禁止されている境界ルール**: `ene-store` は、`ene-mind`（脳・認知ロジック）や `ene-ai`（LLMプロバイダ）、`ene-runtime`（アクター）に依存してはなりません。これにより、他の任意のクレートから安全にインポートでき、循環参照を防止します。

---

## 2. データベース接続 & プラグマ設定

### WALモードとパフォーマンス最適化
SQLite 接続時、マルチスレッド並行処理性能と堅牢性を最大化するため、以下の接続オプション（プラグマ）を設定します。
*   `journal_mode = WAL`: Write-Ahead Logging モードを強制。読込処理が書込処理をブロックせず、並行スレッドでの高速実行が可能になります。
*   `synchronous = NORMAL`: 書込同期を「標準」に変更。WALモード下での安全性を維持しつつディスク同期I/Oを激減させます。
*   `busy_timeout = 5000`: 書込ロック時のタイムアウトを5秒に設定。データベースがロックされている場合にエラーをすぐ返さず待機（バックオフ）します。
*   `foreign_keys = ON`: 参照整合性（外部キー制約）の強制。

### `sqlite-vec` ベクトル拡張のグローバル登録
`init_sqlite_vec` 関数により、C言語の `sqlite3_auto_extension` API を用いて `sqlite-vec` モジュールを登録します。
- **機能**: `1.0 - vec_distance_cosine(embedding, ?)` などのベクトル操作用SQL関数をプロセス全体で利用可能にします。
- **安全性**: `std::sync::Once` により、重複登録を防ぎます。

---

## 3. テーブル定義 & スキーマ一覧

Migrator により、起動時に以下のテーブルが自動生成されます。

```mermaid
erDiagram
    conversation_logs {
        integer id PK
        text session_id
        text role
        text content
        datetime created_at
    }
    affect_states {
        text character_id PK
        real valence
        real arousal
        real dominance
        real irritation
        real fatigue
        real affinity
        text last_expression
        text mood_label
        datetime updated_at
    }
    pending_affect_proposals {
        integer id PK
        text character_id
        text user_id
        integer source_turn_id
        real valence
        real arousal
        real irritation
        real affinity
        text recommended_expression
        real confidence
        text reason
        datetime created_at
    }
    typed_memories {
        integer id PK
        text scope
        text character_id
        text user_id
        text kind
        text title
        text content
        text source
        text source_ref
        real confidence
        real salience
        text affect
        real relationship_impact
        integer access_count
        datetime last_accessed_at
        datetime created_at
        datetime updated_at
        datetime valid_from
        datetime valid_until
        text status
        integer supersedes_id
        boolean pinned
        datetime faded_at
        integer commitment_id
    }
    memory_embeddings {
        integer memory_id PK, FK
        text model_name
        text source_text
        blob embedding
    }
    memory_links {
        integer from_id PK, FK
        integer to_id PK, FK
        text link_type
        real strength
        datetime created_at
    }
    memory_spans {
        integer id PK
        text session_id
        integer turn_start
        integer turn_end
        text raw_excerpt
        text compressed_summary
        integer compression_level
    }
    commitments {
        integer id PK
        text character_id
        text user_id
        text description
        text status
        datetime created_at
        datetime updated_at
        datetime resolved_at
        text fail_reason
    }
    tool_schemas {
        text tool_name PK
        text schema_json
        datetime declared_at
    }
    tool_embedding_index {
        integer id PK
        text tool_name FK
        text field
        text field_key
        text version_hash
        text model_name
        text source_text
        blob embedding
        datetime indexed_at
    }

    typed_memories ||--o| memory_embeddings : "id = memory_id"
    typed_memories ||--o| commitments : "commitment_id = id"
    tool_schemas ||--o{ tool_embedding_index : "tool_name"
```
*   `typed_memories`: 回想記憶やエピソード記憶の属性データを保存。
*   `memory_embeddings`: `typed_memories` に対応する埋め込みベクトル（浮動小数点配列のバイト表現）を保存するベクトル専用シャドウテーブル。
*   `tool_embedding_index`: ツールRAG用のマルチベクトルインデックス（引数説明文などのベクトル）。
