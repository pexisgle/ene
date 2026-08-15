//! Cleanup and index migration for `memory_embeddings`.
//!
//! `PRAGMA` settings are per-connection: with a pooled connection,
//! applying `PRAGMA foreign_keys` once after connect only affects the
//! connection that ran it, so `ON DELETE CASCADE` can silently never
//! fire. That left orphan rows in `memory_embeddings` whose parent
//! `typed_memories` row was already deleted. This migration:
//!
//! 1. Deletes orphan rows whose `memory_item_id` has no matching
//!    `typed_memories.id`.
//! 2. Deduplicates rows sharing the same `(memory_item_id, model_name,
//!    field)`, keeping the most recent (highest `id`).
//! 3. Recreates the `uniq_memory_embedding` unique index (drop + create
//!    to guarantee a clean B-tree).
//! 4. Creates a plain index on `memory_item_id` alone so that FK
//!    cascade lookups have a dedicated single-column index.
//!
//! All statements operate on the **child** table (`memory_embeddings`),
//! so FK enforcement does not need to be disabled: deleting child rows
//! never triggers a constraint check.

use sea_orm_migration::prelude::*;

pub struct EmbeddingsCleanupIndexMigration;

impl MigrationName for EmbeddingsCleanupIndexMigration {
    fn name(&self) -> &'static str {
        "m20260728_000003_embeddings_cleanup_index"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for EmbeddingsCleanupIndexMigration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // Remove orphan embeddings whose parent typed_memories row is
        // gone. These accumulated while FK enforcement was broken.
        // Deleting from the child table is safe with foreign_keys=ON.
        db.execute_unprepared(
            "DELETE FROM memory_embeddings WHERE memory_item_id NOT IN \
             (SELECT id FROM typed_memories)",
        )
        .await?;

        // Deduplicate: keep only the newest row (highest id) for each
        // (memory_item_id, model_name, field) triple. ROW_NUMBER()
        // requires SQLite ≥ 3.25.0 (2018); the bundled libsqlite3-sys
        // far exceeds this.
        db.execute_unprepared(
            "DELETE FROM memory_embeddings WHERE id NOT IN ( \
                 SELECT id FROM ( \
                     SELECT id, ROW_NUMBER() OVER ( \
                         PARTITION BY memory_item_id, model_name, field \
                         ORDER BY id DESC \
                     ) AS rn \
                     FROM memory_embeddings \
                 ) WHERE rn = 1 \
             )",
        )
        .await?;

        // Recreate the unique index from scratch. DROP + CREATE (rather
        // than IF NOT EXISTS alone) guarantees a clean B-tree even if
        // the old index was built over duplicate-tolerant data.
        db.execute_unprepared("DROP INDEX IF EXISTS uniq_memory_embedding")
            .await?;
        db.execute_unprepared(
            "CREATE UNIQUE INDEX IF NOT EXISTS uniq_memory_embedding \
             ON memory_embeddings(memory_item_id, model_name, field)",
        )
        .await?;

        // Dedicated single-column index for FK cascade performance.
        // The composite unique index covers memory_item_id as a prefix,
        // but an explicit index gives the query planner maximum
        // flexibility during ON DELETE CASCADE enforcement.
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS idx_memory_embeddings_item_id \
             ON memory_embeddings(memory_item_id)",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP INDEX IF EXISTS idx_memory_embeddings_item_id")
            .await?;
        // Note: `uniq_memory_embedding` was originally created by the base
        // migration. Dropping it here is safe only when migrations are
        // reverted in order (the base migration's `down()` drops the table
        // entirely). If this migration were rolled back in isolation, the
        // unique constraint backing the ON CONFLICT upsert path in
        // `store/memory.rs` would be lost until the base migration re-runs.
        db.execute_unprepared("DROP INDEX IF EXISTS uniq_memory_embedding")
            .await?;
        Ok(())
    }
}
