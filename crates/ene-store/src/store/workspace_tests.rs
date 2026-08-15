use super::*;
use chrono::Utc;
use ene_core::{NewWorkspaceChunk, WorkspaceFileRow, WorkspaceSearchQuery};

async fn setup_store() -> MemoryStore {
    MemoryStore::open_in_memory(4).await.unwrap()
}

fn file_row(root: &str, path: &str, hash: &str) -> WorkspaceFileRow {
    WorkspaceFileRow {
        root: root.to_string(),
        path: path.to_string(),
        size: 42,
        modified_at: Utc::now(),
        content_hash: hash.to_string(),
        model_name: "test-model".to_string(),
        chunk_count: 0,
    }
}

fn chunk(index: u32, content: &str, start: u32, end: u32, embedding: &[f32]) -> NewWorkspaceChunk {
    NewWorkspaceChunk {
        chunk_index: index,
        heading: format!("heading-{index}"),
        content: content.to_string(),
        start_line: start,
        end_line: end,
        embedding: embedding.to_vec(),
    }
}

async fn seed(store: &MemoryStore, root: &str, path: &str, content: &str, embedding: &[f32]) {
    store
        .replace_workspace_file(
            &file_row(root, path, "hash-a"),
            &[chunk(0, content, 1, 3, embedding)],
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn workspace_replace_rename_delete_and_status() {
    let store = setup_store().await;
    seed(
        &store,
        "/roots/a",
        "/roots/a/one.md",
        "alpha beta",
        &[1.0, 0.0, 0.0, 0.0],
    )
    .await;
    seed(
        &store,
        "/roots/a",
        "/roots/a/two.md",
        "gamma delta",
        &[0.0, 1.0, 0.0, 0.0],
    )
    .await;

    let status = store.workspace_index_status().await.unwrap();
    assert_eq!(status.indexed_files, 2);
    assert_eq!(status.indexed_chunks, 2);

    let renamed = store
        .rename_workspace_file(
            "/roots/a/one.md",
            &file_row("/roots/a", "/roots/a/one-renamed.md", "hash-a"),
        )
        .await
        .unwrap();
    assert!(renamed);
    let files = store.list_workspace_files().await.unwrap();
    assert!(files.iter().any(|f| f.path == "/roots/a/one-renamed.md"));

    let deleted = store
        .delete_workspace_files(&["/roots/a/two.md".to_string()])
        .await
        .unwrap();
    assert_eq!(deleted, 1);
    assert_eq!(
        store.workspace_index_status().await.unwrap().indexed_files,
        1
    );

    let pruned = store.prune_workspace_roots(&[]).await.unwrap();
    assert_eq!(pruned, 1);
    assert_eq!(
        store.workspace_index_status().await.unwrap().indexed_files,
        0
    );
}

#[tokio::test]
async fn workspace_search_filters_roots_and_returns_citations() {
    let store = setup_store().await;
    seed(
        &store,
        "/roots/a",
        "/roots/a/guide.md",
        "installation steps for ene",
        &[1.0, 0.0, 0.0, 0.0],
    )
    .await;
    seed(
        &store,
        "/roots/b",
        "/roots/b/other.md",
        "unrelated content",
        &[0.0, 1.0, 0.0, 0.0],
    )
    .await;

    let hits = store
        .search_workspace(&WorkspaceSearchQuery {
            query_text: "installation steps",
            embedding: Some(&[1.0, 0.0, 0.0, 0.0]),
            model_name: "test-model",
            allowed_roots: &["/roots/a".to_string()],
            top_k: 8,
            min_similarity: 0.0,
        })
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, "/roots/a/guide.md");
    assert_eq!(hits[0].start_line, 1);
    assert_eq!(hits[0].end_line, 3);
    assert_eq!(hits[0].heading, "heading-0");
    assert!(hits[0].similarity > 0.0);

    let hits = store
        .search_workspace(&WorkspaceSearchQuery {
            query_text: "installation steps",
            embedding: None,
            model_name: "test-model",
            allowed_roots: &[],
            top_k: 8,
            min_similarity: 0.0,
        })
        .await
        .unwrap();
    assert!(hits.is_empty());

    // A different model's vectors are invisible, but lexical matching still
    // works (the model only gates the vector half of the hybrid search).
    let hits = store
        .search_workspace(&WorkspaceSearchQuery {
            query_text: "installation steps",
            embedding: Some(&[1.0, 0.0, 0.0, 0.0]),
            model_name: "other-model",
            allowed_roots: &["/roots/a".to_string()],
            top_k: 8,
            min_similarity: 0.0,
        })
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].content.contains("installation steps"));
}

#[tokio::test]
async fn workspace_search_lexical_fallback_finds_token_matches() {
    let store = setup_store().await;
    seed(
        &store,
        "/roots/a",
        "/roots/a/notes.md",
        "blake3 hashing is fast and safe",
        &[0.0, 0.0, 1.0, 0.0],
    )
    .await;

    let hits = store
        .search_workspace(&WorkspaceSearchQuery {
            query_text: "blake3 hashing",
            embedding: None,
            model_name: "test-model",
            allowed_roots: &["/roots/a".to_string()],
            top_k: 8,
            min_similarity: 0.0,
        })
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].content.contains("blake3"));
}

#[tokio::test]
async fn workspace_replace_is_atomic_per_file() {
    let store = setup_store().await;
    seed(
        &store,
        "/roots/a",
        "/roots/a/doc.md",
        "old content",
        &[1.0, 0.0, 0.0, 0.0],
    )
    .await;

    let updated = file_row("/roots/a", "/roots/a/doc.md", "hash-b");
    store
        .replace_workspace_file(
            &updated,
            &[
                chunk(0, "new first", 1, 2, &[0.0, 1.0, 0.0, 0.0]),
                chunk(1, "new second", 3, 4, &[0.0, 0.0, 1.0, 0.0]),
            ],
        )
        .await
        .unwrap();

    let files = store.list_workspace_files().await.unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].content_hash, "hash-b");
    let status = store.workspace_index_status().await.unwrap();
    assert_eq!(status.indexed_chunks, 2);

    let hits = store
        .search_workspace(&WorkspaceSearchQuery {
            query_text: "first",
            embedding: None,
            model_name: "test-model",
            allowed_roots: &["/roots/a".to_string()],
            top_k: 8,
            min_similarity: 0.0,
        })
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].chunk_index, 0);
}

#[tokio::test]
async fn workspace_dimension_change_forces_reembed() {
    let store = setup_store().await;
    seed(
        &store,
        "/roots/a",
        "/roots/a/doc.md",
        "stable content",
        &[1.0, 0.0, 0.0, 0.0],
    )
    .await;
    assert_eq!(
        store.list_workspace_files().await.unwrap()[0].chunk_count,
        1
    );

    // A dimension change rebuilds the vec0 tables empty and drops the
    // dimension-bound base chunk rows, so the next sync re-embeds the file
    // even though its content hash is unchanged.
    super::ensure_vec0_index(&store.db, 8).await.unwrap();
    let files = store.list_workspace_files().await.unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].chunk_count, 0);
    assert_eq!(
        store.workspace_index_status().await.unwrap().indexed_chunks,
        0
    );
}
