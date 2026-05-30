// @generated automatically by Diesel CLI.

diesel::table! {
    undo_entries (id) {
        id -> Text,
        session_id -> Text,
        tool_name -> Text,
        timestamp -> Text,
    }
}

diesel::table! {
    undo_operations (id) {
        id -> Nullable<Integer>,
        entry_id -> Text,
        op_type -> Text,
        path -> Text,
        original_content -> Nullable<Binary>,
        sort_order -> Integer,
    }
}

diesel::joinable!(undo_operations -> undo_entries (entry_id));

diesel::allow_tables_to_appear_in_same_query!(undo_entries, undo_operations,);
