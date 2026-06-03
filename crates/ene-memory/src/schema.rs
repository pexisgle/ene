#![allow(missing_docs)]
// @generated automatically by Diesel CLI.

// Key-value facts extracted from conversations, linked to summaries.
diesel::table! {
    conversation_keyfacts (id) {
        id -> BigInt,
        card_name -> Text,
        summary_id -> Nullable<BigInt>,
        key -> Text,
        value -> Text,
        created_at -> Text,
    }
}

// Raw conversation logs, ordered by creation time within a session.
diesel::table! {
    conversation_logs (id) {
        id -> BigInt,
        session_id -> Text,
        card_name -> Text,
        role -> Text,
        content -> Text,
        created_at -> Text,
    }
}

// Summarized conversation entries with vector embeddings for similarity search.
diesel::table! {
    conversation_summaries (id) {
        id -> BigInt,
        session_id -> Text,
        card_name -> Text,
        summary -> Text,
        embedding -> Binary,
        created_at -> Text,
        ended_at -> Text,
    }
}

// Multi-vector tool embedding index. One row per (tool_name, field, field_key,
// model_name) where `field` ∈ { 'summary', 'description', 'capability',
// 'example', 'negative' }. Enables per-field embedding, storage, and retrieval
// for the ToolRag pipeline.
diesel::table! {
    tool_embedding_index (id) {
        id -> Integer,
        tool_name -> Text,
        field -> Text,
        field_key -> Text,
        version_hash -> Text,
        model_name -> Text,
        embedding -> Binary,
        created_at -> Text,
    }
}

diesel::allow_tables_to_appear_in_same_query!(
    conversation_keyfacts,
    conversation_logs,
    conversation_summaries,
    tool_embedding_index,
);

diesel::joinable!(conversation_keyfacts -> conversation_summaries (summary_id));
