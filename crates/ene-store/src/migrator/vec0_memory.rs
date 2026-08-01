//! Creates `vec0` virtual tables for ANN-accelerated embedding search.
//!
//! The `vec_memory_embeddings` and `vec_tool_embeddings` tables mirror
//! `memory_embeddings` and `tool_embedding_index` respectively, enabling
//! approximate nearest-neighbor (ANN) search via the `sqlite-vec` `vec0`
//! module rather than a brute-force `vec_distance_cosine` scan.
//!
//! Because the `vec0` column definition requires a fixed vector dimension
//! (`float[N]`) and the dimension is a runtime parameter of
//! [`MemoryStore`](crate::MemoryStore), this migration creates the tables
//! only when existing embedding data is present (the upgrade path). For
//! fresh databases the tables are created lazily by
//! [`ensure_vec0_index`](crate::store::ensure_vec0_index) during store
//! initialization, once the embedding dimension is known.

use sea_orm::{ConnectionTrait, DbBackend, Statement};
use sea_orm_migration::prelude::*;

/// Creates `vec0` ANN index tables over the existing embedding tables.
pub struct Vec0EmbeddingIndexMigration;

impl MigrationName for Vec0EmbeddingIndexMigration {
    fn name(&self) -> &'static str {
        "m20260731_000006_vec0_embedding_index"
    }
}

/// Reads the embedding dimension (number of f32 components) from the first
/// row of `table`, returning `None` when the table is empty.
async fn detect_embedding_dim<C: ConnectionTrait>(
    db: &C,
    table: &str,
) -> Result<Option<i64>, DbErr> {
    let sql = format!("SELECT length(embedding) / 4 FROM {table} LIMIT 1");
    let rows = db
        .query_all_raw(Statement::from_string(DbBackend::Sqlite, sql))
        .await?;
    match rows.first() {
        Some(row) => {
            let dim: i64 = row.try_get_by_index(0)?;
            Ok(Some(dim))
        }
        None => Ok(None),
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Vec0EmbeddingIndexMigration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // Determine the embedding dimension from existing data. Both tables
        // share the same model, so either source suffices; prefer
        // memory_embeddings (the larger, performance-critical table).
        let dim = match detect_embedding_dim(db, "memory_embeddings").await? {
            Some(d) => Some(d),
            None => detect_embedding_dim(db, "tool_embedding_index").await?,
        };

        let Some(dim) = dim else {
            // No embeddings yet; the vec0 tables will be created lazily by
            // `ensure_vec0_index` when the first embedding is stored.
            return Ok(());
        };

        // character_id is denormalized from typed_memories so the KNN query
        // can scope to a single character via a vec0 metadata filter.
        db.execute_unprepared(&format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS vec_memory_embeddings USING vec0( \
                 memory_embedding_id integer primary key, \
                 embedding float[{dim}] distance_metric=cosine, \
                 character_id text, \
                 model_name text, \
                 field text \
             )"
        ))
        .await?;

        db.execute_unprepared(
            "INSERT INTO vec_memory_embeddings(memory_embedding_id, embedding, character_id, model_name, field) \
             SELECT me.id, me.embedding, tm.character_id, me.model_name, me.field \
             FROM memory_embeddings me \
             INNER JOIN typed_memories tm ON tm.id = me.memory_item_id",
        )
        .await?;

        db.execute_unprepared(&format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS vec_tool_embeddings USING vec0( \
                 tool_embedding_id integer primary key, \
                 embedding float[{dim}] distance_metric=cosine, \
                 tool_name text \
             )"
        ))
        .await?;

        db.execute_unprepared(
            "INSERT INTO vec_tool_embeddings(tool_embedding_id, embedding, tool_name) \
             SELECT id, embedding, tool_name FROM tool_embedding_index",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // Drop the sync triggers first so they don't reference the dropped
        // vec0 tables (they are created by `ensure_vec0_index`, not by this
        // migration, but the down path must clean them up regardless).
        db.execute_unprepared("DROP TRIGGER IF EXISTS trg_vec_mem_ai")
            .await?;
        db.execute_unprepared("DROP TRIGGER IF EXISTS trg_vec_mem_au")
            .await?;
        db.execute_unprepared("DROP TRIGGER IF EXISTS trg_vec_mem_ad")
            .await?;
        db.execute_unprepared("DROP TRIGGER IF EXISTS trg_vec_tool_ai")
            .await?;
        db.execute_unprepared("DROP TRIGGER IF EXISTS trg_vec_tool_au")
            .await?;
        db.execute_unprepared("DROP TRIGGER IF EXISTS trg_vec_tool_ad")
            .await?;

        db.execute_unprepared("DROP TABLE IF EXISTS vec_memory_embeddings")
            .await?;
        db.execute_unprepared("DROP TABLE IF EXISTS vec_tool_embeddings")
            .await?;
        Ok(())
    }
}
