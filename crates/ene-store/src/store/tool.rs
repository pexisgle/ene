//! Tool-embedding index queries.

use super::{
    MemoryError, MemoryStore, ToolEmbeddingFieldRow, bytes_to_embedding, cosine_similarity_expr,
    cosine_similarity_filter, embedding_to_bytes, validate_embedding,
};
use crate::entities;
use chrono::Utc;
use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm::{ColumnTrait, EntityTrait, FromQueryResult, QueryFilter, QueryOrder, QuerySelect};

impl MemoryStore {
    // ── Tool Embeddings (multi-vector) ──────────────────────────────────────

    /// Inserts or updates one field's embedding for a tool.
    ///
    /// `field` must be one of `"summary"`, `"description"`, `"capability"`,
    /// `"example"`, or `"negative"`, matching the provider's embedding-kind labels.
    /// `field_key` disambiguates multiple entries of the same field type
    /// (e.g. separate `"example"` rows with keys `"ex_0"`, `"ex_1"`).
    pub async fn upsert_tool_embedding_field(
        &self,
        tool_name: &str,
        field: &str,
        field_key: &str,
        version_hash: &str,
        model_name: &str,
        embedding: &[f32],
        source_text: &str,
    ) -> Result<(), MemoryError> {
        use sea_orm::ActiveValue::Set;

        validate_embedding(embedding, self.embedding_dim)?;

        let now = Utc::now();
        let embedding_bytes = embedding_to_bytes(embedding);

        let new_embedding = entities::tool_embedding_index::ActiveModel {
            tool_name: Set(tool_name.to_string()),
            field: Set(field.to_string()),
            field_key: Set(field_key.to_string()),
            version_hash: Set(version_hash.to_string()),
            model_name: Set(model_name.to_string()),
            source_text: Set(source_text.to_string()),
            embedding: Set(embedding_bytes),
            created_at: Set(now),
            ..Default::default()
        };

        entities::tool_embedding_index::Entity::insert(new_embedding)
            .on_conflict(
                OnConflict::columns([
                    entities::tool_embedding_index::Column::ToolName,
                    entities::tool_embedding_index::Column::Field,
                    entities::tool_embedding_index::Column::FieldKey,
                    entities::tool_embedding_index::Column::ModelName,
                ])
                .update_columns([
                    entities::tool_embedding_index::Column::VersionHash,
                    entities::tool_embedding_index::Column::Embedding,
                    entities::tool_embedding_index::Column::SourceText,
                    entities::tool_embedding_index::Column::CreatedAt,
                ])
                .to_owned(),
            )
            .exec(&self.db)
            .await?;

        Ok(())
    }

    /// Lists all stored tool embeddings with full data (embedding vector + source text).
    ///
    /// Prefer [`list_tool_embedding_hashes`](Self::list_tool_embedding_hashes) when
    /// only `(tool_name, field, field_key, version_hash, model_name)` is needed —
    /// that variant uses a SQL projection that avoids fetching the large `embedding`
    /// BLOB and `source_text` columns.
    pub async fn list_tool_embedding_fields(
        &self,
    ) -> Result<Vec<ToolEmbeddingFieldRow>, MemoryError> {
        let rows = entities::tool_embedding_index::Entity::find()
            .all(&self.db)
            .await?;

        Ok(rows
            .into_iter()
            .map(|row| ToolEmbeddingFieldRow {
                tool_name: row.tool_name,
                field: row.field,
                field_key: row.field_key,
                version_hash: row.version_hash,
                model_name: row.model_name,
                embedding: bytes_to_embedding(&row.embedding),
                source_text: row.source_text,
            })
            .collect())
    }

    /// Returns `(tool_name, field, field_key, version_hash, model_name)`
    /// for every cached tool embedding row, **without**
    /// deserializing the vector or fetching the source
    /// text. Used by Tool RAG's `ensure_index` to decide
    /// which fields are up-to-date; the previous form
    /// deserialized every f32 vector on every turn and
    /// then discarded them.
    pub async fn list_tool_embedding_hashes(
        &self,
    ) -> Result<Vec<(String, String, String, String, String)>, MemoryError> {
        #[derive(FromQueryResult)]
        struct HashRow {
            tool_name: String,
            field: String,
            field_key: String,
            version_hash: String,
            model_name: String,
        }

        let rows = entities::tool_embedding_index::Entity::find()
            .select_only()
            .column(entities::tool_embedding_index::Column::ToolName)
            .column(entities::tool_embedding_index::Column::Field)
            .column(entities::tool_embedding_index::Column::FieldKey)
            .column(entities::tool_embedding_index::Column::VersionHash)
            .column(entities::tool_embedding_index::Column::ModelName)
            .into_model::<HashRow>()
            .all(&self.db)
            .await?;

        Ok(rows
            .into_iter()
            .map(|row| {
                (
                    row.tool_name,
                    row.field,
                    row.field_key,
                    row.version_hash,
                    row.model_name,
                )
            })
            .collect())
    }

    /// Deletes all field embeddings for a tool (cascades across all fields).
    pub async fn delete_tool_embeddings(&self, tool_name: &str) -> Result<usize, MemoryError> {
        let res = entities::tool_embedding_index::Entity::delete_many()
            .filter(entities::tool_embedding_index::Column::ToolName.eq(tool_name))
            .exec(&self.db)
            .await?;
        Ok(res.rows_affected as usize)
    }

    /// Searches tool embeddings by cosine similarity to the query across ALL
    /// fields, then aggregates the per-field similarity scores for each tool
    /// using max-pool (the strongest signal wins). Returns tools sorted by
    /// aggregated similarity.
    pub async fn search_tools(
        &self,
        query_embedding: &[f32],
        limit: usize,
        similarity_threshold: f32,
    ) -> Result<Vec<(String, f32)>, MemoryError> {
        #[derive(Debug, FromQueryResult)]
        struct SearchToolRow {
            tool_name: String,
            similarity: f64,
        }

        validate_embedding(query_embedding, self.embedding_dim)?;

        let query_bytes = embedding_to_bytes(query_embedding);
        let similarity_expr = cosine_similarity_expr("embedding", &query_bytes);

        let factor = 4u64;
        let row_cap = (limit as u64).saturating_mul(factor).max(limit as u64);

        let select = entities::tool_embedding_index::Entity::find()
            .select_only()
            .column(entities::tool_embedding_index::Column::ToolName)
            .expr_as(similarity_expr, "similarity")
            .filter(cosine_similarity_filter(
                "embedding",
                &query_bytes,
                f64::from(similarity_threshold),
            ))
            .order_by_desc(Expr::col("similarity"))
            .limit(row_cap);

        let rows = select.into_model::<SearchToolRow>().all(&self.db).await?;

        use std::collections::HashMap;
        let mut by_tool: HashMap<String, f32> = HashMap::new();
        for row in rows {
            let sim = row.similarity as f32;
            let entry = by_tool.entry(row.tool_name).or_insert(f32::MIN);
            if sim > *entry {
                *entry = sim;
            }
        }

        let mut aggregated: Vec<(String, f32)> = by_tool.into_iter().collect();
        aggregated.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        aggregated.truncate(limit);

        Ok(aggregated)
    }
}
