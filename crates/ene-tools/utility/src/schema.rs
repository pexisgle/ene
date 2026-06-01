// @generated manually — mirrors migrations/00000000000000_create_todo_tables/up.sql.

diesel::table! {
    todo_items (id) {
        id -> Integer,
        session_id -> Text,
        parent_id -> Nullable<Integer>,
        content -> Text,
        status -> Text,
        priority -> Text,
        created_at -> Text,
        updated_at -> Text,
    }
}

diesel::allow_tables_to_appear_in_same_query!(todo_items,);
