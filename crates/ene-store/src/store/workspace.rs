//! Workspace document index queries (the document/workspace RAG persistence).

use super::{MemoryStore, embedding_to_bytes, validate_embedding};
use crate::entities;
use chrono::Utc;
use ene_core::{
    NewWorkspaceChunk, WorkspaceChunkHit, WorkspaceFileRow, WorkspaceIndexStatus,
    WorkspaceSearchQuery,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, FromQueryResult,
    PaginatorTrait, QueryFilter, Statement, TransactionTrait,
};
use std::collections::HashMap;

#[derive(Debug, FromQueryResult)]
struct FileListRow {
    root: String,
    path: String,
    size: i64,
    modified_at: chrono::DateTime<chrono::Utc>,
    content_hash: String,
    model_name: String,
    chunk_count: i64,
}

#[derive(Debug, FromQueryResult)]
struct VecSearchRow {
    chunk_index: i32,
    heading: String,
    content: String,
    start_line: i32,
    end_line: i32,
    root: String,
    path: String,
    similarity: f64,
}

#[derive(Debug, FromQueryResult)]
struct LexicalSearchRow {
    chunk_index: i32,
    heading: String,
    content: String,
    start_line: i32,
    end_line: i32,
    root: String,
    path: String,
}

impl MemoryStore {
    /// All indexed file rows (no vectors or chunk text).
    pub async fn list_workspace_files(
        &self,
    ) -> Result<Vec<WorkspaceFileRow>, crate::error::EneMemoryError> {
        let rows = self
            .db
            .query_all_raw(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT f.root, f.path, f.size, f.modified_at, f.content_hash, \
                        f.model_name, COUNT(c.id) AS chunk_count \
                 FROM workspace_document_files f \
                 LEFT JOIN workspace_document_chunks c ON c.file_id = f.id \
                 GROUP BY f.id \
                 ORDER BY f.path ASC"
                    .to_string(),
            ))
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|row| FileListRow::from_query_result(&row, "").ok())
            .map(model_to_file_row)
            .collect())
    }

    /// Atomically replace one file's chunks and vec0 entries.
    pub async fn replace_workspace_file(
        &self,
        file: &WorkspaceFileRow,
        chunks: &[NewWorkspaceChunk],
    ) -> Result<(), crate::error::EneMemoryError> {
        use entities::workspace_document_chunks::{
            ActiveModel as ChunkActive, Column as ChunkColumn,
        };
        use entities::workspace_document_files::{
            ActiveModel as FileActive, Column as FileColumn, Entity as FileEntity,
        };
        use sea_orm::ActiveValue::Set;

        for chunk in chunks {
            validate_embedding(&chunk.embedding, self.embedding_dim)?;
        }

        let txn = self.db.begin().await?;
        let now = Utc::now();

        let existing = FileEntity::find()
            .filter(FileColumn::Path.eq(&file.path))
            .one(&txn)
            .await?;

        let file_id = if let Some(row) = existing {
            let mut active: FileActive = row.into();
            active.root = Set(file.root.clone());
            active.size = Set(i64::try_from(file.size).unwrap_or(i64::MAX));
            active.modified_at = Set(file.modified_at);
            active.content_hash = Set(file.content_hash.clone());
            active.model_name = Set(file.model_name.clone());
            active.updated_at = Set(now);
            active.update(&txn).await?.id
        } else {
            let active = FileActive {
                root: Set(file.root.clone()),
                path: Set(file.path.clone()),
                size: Set(i64::try_from(file.size).unwrap_or(i64::MAX)),
                modified_at: Set(file.modified_at),
                content_hash: Set(file.content_hash.clone()),
                model_name: Set(file.model_name.clone()),
                created_at: Set(now),
                updated_at: Set(now),
                ..Default::default()
            };
            active.insert(&txn).await?.id
        };

        // Drop the old chunks (and their vec0 rows) before inserting the new
        // ones so the file's index always reflects a single snapshot.
        let old_chunk_ids = self.workspace_chunk_ids_for_file(&txn, file_id).await?;
        self.delete_workspace_vec_rows(&txn, &old_chunk_ids).await?;
        entities::workspace_document_chunks::Entity::delete_many()
            .filter(ChunkColumn::FileId.eq(file_id))
            .exec(&txn)
            .await?;

        for chunk in chunks {
            let inserted = ChunkActive {
                file_id: Set(file_id),
                chunk_index: Set(i32::try_from(chunk.chunk_index).unwrap_or(i32::MAX)),
                heading: Set(chunk.heading.clone()),
                content: Set(chunk.content.clone()),
                start_line: Set(i32::try_from(chunk.start_line).unwrap_or(i32::MAX)),
                end_line: Set(i32::try_from(chunk.end_line).unwrap_or(i32::MAX)),
                embedding: Set(embedding_to_bytes(&chunk.embedding)),
                created_at: Set(now),
                ..Default::default()
            }
            .insert(&txn)
            .await?;
            self.insert_workspace_vec_row(
                &txn,
                inserted.id,
                file_id,
                &chunk.embedding,
                &file.model_name,
            )
            .await?;
        }

        txn.commit().await?;
        Ok(())
    }

    /// Move an indexed file to a new path, keeping its chunks.
    ///
    /// Returns `false` when no row had `old_path`.
    pub async fn rename_workspace_file(
        &self,
        old_path: &str,
        file: &WorkspaceFileRow,
    ) -> Result<bool, crate::error::EneMemoryError> {
        use entities::workspace_document_files::{ActiveModel, Column, Entity};
        use sea_orm::ActiveValue::Set;

        let Some(row) = Entity::find()
            .filter(Column::Path.eq(old_path))
            .one(&self.db)
            .await?
        else {
            return Ok(false);
        };
        let now = Utc::now();
        let mut active: ActiveModel = row.into();
        active.root = Set(file.root.clone());
        active.path = Set(file.path.clone());
        active.size = Set(i64::try_from(file.size).unwrap_or(i64::MAX));
        active.modified_at = Set(file.modified_at);
        active.content_hash = Set(file.content_hash.clone());
        active.updated_at = Set(now);
        active.update(&self.db).await?;
        Ok(true)
    }

    /// Delete the file rows (and chunk/vec0 rows) for the given paths.
    pub async fn delete_workspace_files(
        &self,
        paths: &[String],
    ) -> Result<usize, crate::error::EneMemoryError> {
        if paths.is_empty() {
            return Ok(0);
        }
        use entities::workspace_document_files::{Column, Entity};

        let txn = self.db.begin().await?;
        let mut deleted = 0usize;
        for batch in paths.chunks(500) {
            let rows = Entity::find()
                .filter(Column::Path.is_in(batch.iter().map(String::as_str)))
                .all(&txn)
                .await?;
            for row in rows {
                let chunk_ids = self.workspace_chunk_ids_for_file(&txn, row.id).await?;
                self.delete_workspace_vec_rows(&txn, &chunk_ids).await?;
                Entity::delete_by_id(row.id).exec(&txn).await?;
                deleted = deleted.saturating_add(1);
            }
        }
        txn.commit().await?;
        Ok(deleted)
    }

    /// Delete every file row whose root is not in `keep_roots`.
    pub async fn prune_workspace_roots(
        &self,
        keep_roots: &[String],
    ) -> Result<usize, crate::error::EneMemoryError> {
        use entities::workspace_document_files::{Column, Entity};

        let txn = self.db.begin().await?;
        let mut query = Entity::find();
        if !keep_roots.is_empty() {
            query = query.filter(Column::Root.is_not_in(keep_roots.iter().map(String::as_str)));
        }
        let rows = query.all(&txn).await?;
        let total = rows.len();
        for row in rows {
            let chunk_ids = self.workspace_chunk_ids_for_file(&txn, row.id).await?;
            self.delete_workspace_vec_rows(&txn, &chunk_ids).await?;
            Entity::delete_by_id(row.id).exec(&txn).await?;
        }
        txn.commit().await?;
        Ok(total)
    }

    /// Hybrid search over chunks belonging to the permitted roots.
    pub async fn search_workspace(
        &self,
        query: &WorkspaceSearchQuery<'_>,
    ) -> Result<Vec<WorkspaceChunkHit>, crate::error::EneMemoryError> {
        if query.allowed_roots.is_empty() || query.top_k == 0 {
            return Ok(Vec::new());
        }
        if let Some(embedding) = query.embedding {
            validate_embedding(embedding, self.embedding_dim)?;
        }

        let pool = query.top_k.saturating_mul(4).max(query.top_k);
        let mut by_id: HashMap<(String, u32), WorkspaceChunkHit> = HashMap::new();

        if let Some(embedding) = query.embedding {
            for hit in self.search_workspace_vector(query, embedding, pool).await? {
                let score = ene_rag::workspace::score_chunk(
                    query.query_text,
                    &hit.content,
                    Some(hit.similarity),
                );
                if score >= query.min_similarity {
                    by_id.insert(
                        (hit.path.clone(), hit.chunk_index),
                        WorkspaceChunkHit {
                            similarity: score,
                            ..hit
                        },
                    );
                }
            }
        }

        for hit in self.search_workspace_lexical(query, pool).await? {
            let score = ene_rag::workspace::score_chunk(query.query_text, &hit.content, None);
            if score < query.min_similarity {
                continue;
            }
            let key = (hit.path.clone(), hit.chunk_index);
            match by_id.get(&key) {
                Some(existing) if existing.similarity >= score => {}
                _ => {
                    by_id.insert(
                        key,
                        WorkspaceChunkHit {
                            similarity: score,
                            ..hit
                        },
                    );
                }
            }
        }

        let mut hits: Vec<WorkspaceChunkHit> = by_id.into_values().collect();
        hits.sort_by(|a, b| {
            b.similarity
                .partial_cmp(&a.similarity)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(query.top_k);
        Ok(hits)
    }

    /// Indexed file/chunk counts.
    pub async fn workspace_index_status(
        &self,
    ) -> Result<WorkspaceIndexStatus, crate::error::EneMemoryError> {
        use entities::{workspace_document_chunks, workspace_document_files};
        Ok(WorkspaceIndexStatus {
            indexed_files: workspace_document_files::Entity::find()
                .count(&self.db)
                .await?,
            indexed_chunks: workspace_document_chunks::Entity::find()
                .count(&self.db)
                .await?,
        })
    }

    async fn search_workspace_vector(
        &self,
        query: &WorkspaceSearchQuery<'_>,
        embedding: &[f32],
        pool: usize,
    ) -> Result<Vec<WorkspaceChunkHit>, crate::error::EneMemoryError> {
        let query_bytes = embedding_to_bytes(embedding);
        let max_distance = 1.0_f64 - f64::from(query.min_similarity);
        let knn_k = u64::try_from(pool).unwrap_or(u64::MAX);
        let root_placeholders: Vec<&str> = query.allowed_roots.iter().map(|_| "?").collect();
        let root_list = root_placeholders.join(", ");

        let sql = format!(
            "WITH knn AS ( \
                 SELECT chunk_id, distance \
                 FROM vec_workspace_chunks \
                 WHERE embedding MATCH ? \
                   AND k = ? \
                   AND model_name = ? \
                   AND distance <= ? \
             ) \
             SELECT c.chunk_index, c.heading, c.content, c.start_line, c.end_line, \
                    f.root, f.path, 1.0 - knn.distance AS similarity \
             FROM knn \
             INNER JOIN workspace_document_chunks c ON c.id = knn.chunk_id \
             INNER JOIN workspace_document_files f ON f.id = c.file_id \
             WHERE f.root IN ({root_list}) \
             ORDER BY knn.distance ASC \
             LIMIT ?"
        );

        let mut values: Vec<sea_orm::Value> = vec![
            sea_orm::Value::from(query_bytes),
            sea_orm::Value::from(knn_k),
            sea_orm::Value::from(query.model_name.to_string()),
            sea_orm::Value::from(max_distance),
        ];
        values.extend(
            query
                .allowed_roots
                .iter()
                .map(|root| sea_orm::Value::from(root.clone())),
        );
        values.push(sea_orm::Value::from(
            u64::try_from(pool).unwrap_or(u64::MAX),
        ));

        let rows = self
            .db
            .query_all_raw(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                sql,
                values,
            ))
            .await?;

        rows.into_iter()
            .filter_map(|row| VecSearchRow::from_query_result(&row, "").ok())
            .map(|row| Ok(vec_row_to_hit(row)))
            .collect()
    }

    async fn search_workspace_lexical(
        &self,
        query: &WorkspaceSearchQuery<'_>,
        pool: usize,
    ) -> Result<Vec<WorkspaceChunkHit>, crate::error::EneMemoryError> {
        use crate::search::tokenize;

        let tokens: Vec<String> = tokenize(query.query_text).into_iter().collect();
        if tokens.is_empty() {
            return Ok(Vec::new());
        }

        let root_placeholders: Vec<&str> = query.allowed_roots.iter().map(|_| "?").collect();
        let root_list = root_placeholders.join(", ");
        let mut sql = String::from(
            "SELECT c.chunk_index, c.heading, c.content, c.start_line, c.end_line, \
                    f.root, f.path \
             FROM workspace_document_chunks c \
             INNER JOIN workspace_document_files f ON f.id = c.file_id \
             WHERE f.root IN (",
        );
        sql.push_str(&root_list);
        sql.push_str(") AND (");
        let mut values: Vec<sea_orm::Value> = query
            .allowed_roots
            .iter()
            .map(|root| sea_orm::Value::from(root.clone()))
            .collect();
        let mut match_count = String::from("(");
        for (i, token) in tokens.iter().enumerate() {
            if i > 0 {
                sql.push_str(" OR ");
                match_count.push_str(" + ");
            }
            let pattern = format!("%{token}%");
            sql.push_str("(c.content LIKE ? OR c.heading LIKE ? OR f.path LIKE ?)");
            values.push(sea_orm::Value::from(pattern.clone()));
            values.push(sea_orm::Value::from(pattern.clone()));
            values.push(sea_orm::Value::from(pattern.clone()));
            match_count.push_str(
                "CASE WHEN c.content LIKE ? OR c.heading LIKE ? OR f.path LIKE ? THEN 1 ELSE 0 END",
            );
            values.push(sea_orm::Value::from(pattern.clone()));
            values.push(sea_orm::Value::from(pattern.clone()));
            values.push(sea_orm::Value::from(pattern));
        }
        match_count.push(')');
        // Over-fetch the chunks matching the most distinct query tokens so
        // the post-SQL score competition sees the best lexical candidates,
        // not the first rows by insertion order.
        sql.push_str(") ORDER BY ");
        sql.push_str(&match_count);
        sql.push_str(" DESC, c.id ASC LIMIT ?");
        values.push(sea_orm::Value::from(
            u64::try_from(pool).unwrap_or(u64::MAX),
        ));

        let rows = self
            .db
            .query_all_raw(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                sql,
                values,
            ))
            .await?;

        rows.into_iter()
            .filter_map(|row| LexicalSearchRow::from_query_result(&row, "").ok())
            .map(|row| Ok(lexical_row_to_hit(row)))
            .collect()
    }

    async fn workspace_chunk_ids_for_file<C: ConnectionTrait>(
        &self,
        db: &C,
        file_id: i64,
    ) -> Result<Vec<i64>, crate::error::EneMemoryError> {
        use entities::workspace_document_chunks::{Column, Entity};
        let rows = Entity::find()
            .filter(Column::FileId.eq(file_id))
            .all(db)
            .await?;
        Ok(rows.into_iter().map(|row| row.id).collect())
    }

    async fn delete_workspace_vec_rows<C: ConnectionTrait>(
        &self,
        db: &C,
        chunk_ids: &[i64],
    ) -> Result<(), crate::error::EneMemoryError> {
        if chunk_ids.is_empty() {
            return Ok(());
        }
        let placeholders: Vec<&str> = chunk_ids.iter().map(|_| "?").collect();
        let sql = format!(
            "DELETE FROM vec_workspace_chunks WHERE chunk_id IN ({})",
            placeholders.join(", ")
        );
        let values: Vec<sea_orm::Value> = chunk_ids
            .iter()
            .map(|id| sea_orm::Value::from(*id))
            .collect();
        db.execute_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            sql,
            values,
        ))
        .await?;
        Ok(())
    }

    async fn insert_workspace_vec_row<C: ConnectionTrait>(
        &self,
        db: &C,
        chunk_id: i64,
        file_id: i64,
        embedding: &[f32],
        model_name: &str,
    ) -> Result<(), crate::error::EneMemoryError> {
        let sql = "INSERT INTO vec_workspace_chunks(chunk_id, embedding, file_id, model_name) \
                   VALUES (?, ?, ?, ?)";
        db.execute_raw(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            sql.to_string(),
            vec![
                sea_orm::Value::from(chunk_id),
                sea_orm::Value::from(embedding_to_bytes(embedding)),
                sea_orm::Value::from(file_id),
                sea_orm::Value::from(model_name.to_string()),
            ],
        ))
        .await?;
        Ok(())
    }
}

fn model_to_file_row(row: FileListRow) -> WorkspaceFileRow {
    WorkspaceFileRow {
        root: row.root,
        path: row.path,
        size: u64::try_from(row.size).unwrap_or(u64::MAX),
        modified_at: row.modified_at,
        content_hash: row.content_hash,
        model_name: row.model_name,
        chunk_count: u64::try_from(row.chunk_count).unwrap_or(u64::MAX),
    }
}

fn vec_row_to_hit(row: VecSearchRow) -> WorkspaceChunkHit {
    WorkspaceChunkHit {
        chunk_index: u32::try_from(row.chunk_index).unwrap_or(u32::MAX),
        root: row.root,
        path: row.path,
        heading: row.heading,
        start_line: u32::try_from(row.start_line).unwrap_or(u32::MAX),
        end_line: u32::try_from(row.end_line).unwrap_or(u32::MAX),
        content: row.content,
        similarity: row.similarity as f32,
    }
}

fn lexical_row_to_hit(row: LexicalSearchRow) -> WorkspaceChunkHit {
    WorkspaceChunkHit {
        chunk_index: u32::try_from(row.chunk_index).unwrap_or(u32::MAX),
        root: row.root,
        path: row.path,
        heading: row.heading,
        start_line: u32::try_from(row.start_line).unwrap_or(u32::MAX),
        end_line: u32::try_from(row.end_line).unwrap_or(u32::MAX),
        content: row.content,
        similarity: 0.0,
    }
}
