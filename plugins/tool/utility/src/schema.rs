use ene_plugin_db::{DbColumn, DbIndex, DbSchema, DbTable, DbType, DbValue};

/// Returns the DB schema for the utility tool's todo tables.
pub fn utility_db_schema() -> DbSchema {
    DbSchema {
        prefix: "utility_".to_string(),
        tables: vec![DbTable {
            name: "utility_todo_items".to_string(),
            columns: vec![
                DbColumn {
                    name: "id".to_string(),
                    ty: DbType::Integer,
                    nullable: false,
                    primary_key: true,
                    auto_increment: true,
                    unique: false,
                    default: None,
                },
                DbColumn {
                    name: "session_id".to_string(),
                    ty: DbType::Text,
                    nullable: false,
                    primary_key: false,
                    auto_increment: false,
                    unique: false,
                    default: None,
                },
                DbColumn {
                    name: "parent_id".to_string(),
                    ty: DbType::Integer,
                    nullable: true,
                    primary_key: false,
                    auto_increment: false,
                    unique: false,
                    default: None,
                },
                DbColumn {
                    name: "content".to_string(),
                    ty: DbType::Text,
                    nullable: false,
                    primary_key: false,
                    auto_increment: false,
                    unique: false,
                    default: None,
                },
                DbColumn {
                    name: "status".to_string(),
                    ty: DbType::Text,
                    nullable: false,
                    primary_key: false,
                    auto_increment: false,
                    unique: false,
                    default: Some(DbValue::Text("pending".to_string())),
                },
                DbColumn {
                    name: "priority".to_string(),
                    ty: DbType::Text,
                    nullable: false,
                    primary_key: false,
                    auto_increment: false,
                    unique: false,
                    default: Some(DbValue::Text("medium".to_string())),
                },
                DbColumn {
                    name: "created_at".to_string(),
                    ty: DbType::Text,
                    nullable: false,
                    primary_key: false,
                    auto_increment: false,
                    unique: false,
                    default: None,
                },
                DbColumn {
                    name: "updated_at".to_string(),
                    ty: DbType::Text,
                    nullable: false,
                    primary_key: false,
                    auto_increment: false,
                    unique: false,
                    default: None,
                },
            ],
        }],
        indexes: vec![
            DbIndex {
                name: "idx_utility_todo_session".to_string(),
                table: "utility_todo_items".to_string(),
                columns: vec!["session_id".to_string()],
                unique: false,
            },
            DbIndex {
                name: "idx_utility_todo_parent".to_string(),
                table: "utility_todo_items".to_string(),
                columns: vec!["parent_id".to_string()],
                unique: false,
            },
            DbIndex {
                name: "idx_utility_todo_status".to_string(),
                table: "utility_todo_items".to_string(),
                columns: vec!["status".to_string()],
                unique: false,
            },
        ],
    }
}
