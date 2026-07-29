use ene_plugin_db::{DbColumn, DbIndex, DbSchema, DbTable, DbType};

pub fn fs_db_schema() -> DbSchema {
    DbSchema {
        prefix: "fs_".to_string(),
        tables: vec![
            DbTable {
                name: "fs_undo_entries".to_string(),
                columns: vec![
                    DbColumn {
                        name: "id".to_string(),
                        ty: DbType::Text,
                        nullable: false,
                        primary_key: true,
                        auto_increment: false,
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
                        name: "tool_name".to_string(),
                        ty: DbType::Text,
                        nullable: false,
                        primary_key: false,
                        auto_increment: false,
                        unique: false,
                        default: None,
                    },
                    DbColumn {
                        name: "timestamp".to_string(),
                        ty: DbType::Text,
                        nullable: false,
                        primary_key: false,
                        auto_increment: false,
                        unique: false,
                        default: None,
                    },
                ],
            },
            DbTable {
                name: "fs_undo_operations".to_string(),
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
                        name: "entry_id".to_string(),
                        ty: DbType::Text,
                        nullable: false,
                        primary_key: false,
                        auto_increment: false,
                        unique: false,
                        default: None,
                    },
                    DbColumn {
                        name: "op_type".to_string(),
                        ty: DbType::Text,
                        nullable: false,
                        primary_key: false,
                        auto_increment: false,
                        unique: false,
                        default: None,
                    },
                    DbColumn {
                        name: "path".to_string(),
                        ty: DbType::Text,
                        nullable: false,
                        primary_key: false,
                        auto_increment: false,
                        unique: false,
                        default: None,
                    },
                    DbColumn {
                        name: "original_content".to_string(),
                        ty: DbType::Blob,
                        nullable: true,
                        primary_key: false,
                        auto_increment: false,
                        unique: false,
                        default: None,
                    },
                    DbColumn {
                        name: "sort_order".to_string(),
                        ty: DbType::Integer,
                        nullable: false,
                        primary_key: false,
                        auto_increment: false,
                        unique: false,
                        default: None,
                    },
                ],
            },
        ],
        indexes: vec![
            DbIndex {
                name: "idx_fs_undo_entries_session_id".to_string(),
                table: "fs_undo_entries".to_string(),
                columns: vec!["session_id".to_string()],
                unique: false,
            },
            DbIndex {
                name: "idx_fs_undo_operations_entry_id".to_string(),
                table: "fs_undo_operations".to_string(),
                columns: vec!["entry_id".to_string()],
                unique: false,
            },
        ],
    }
}
