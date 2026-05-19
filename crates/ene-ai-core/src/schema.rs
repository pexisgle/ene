// @generated automatically by Diesel CLI.

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

diesel::table! {
    tool_embeddings (tool_name) {
        tool_name -> Text,
        version_hash -> Text,
        embedding -> Binary,
        created_at -> Text,
    }
}

diesel::allow_tables_to_appear_in_same_query!(
    conversation_keyfacts,
    conversation_logs,
    conversation_summaries,
    tool_embeddings,
);

diesel::joinable!(conversation_keyfacts -> conversation_summaries (summary_id));
