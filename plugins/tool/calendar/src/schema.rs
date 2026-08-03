use ene_plugin_db::{DbColumn, DbIndex, DbSchema, DbTable, DbType};

/// Returns the DB schema for the calendar tool's tables.
///
/// Table names use the `calendar_` prefix as required by the host-service
/// namespace isolation. `calendar_events` rows are scoped to an account via
/// `account_id`; removing an account must cascade-delete its events, which
/// `CalendarStore::remove_account` does in a single `Batch` transaction.
pub fn calendar_db_schema() -> DbSchema {
    DbSchema {
        prefix: "calendar_".to_string(),
        tables: vec![
            DbTable {
                name: "calendar_accounts".to_string(),
                columns: vec![
                    column("id", DbType::Text, true, false),
                    DbColumn {
                        name: "name".to_string(),
                        ty: DbType::Text,
                        nullable: false,
                        primary_key: false,
                        auto_increment: false,
                        unique: false,
                        default: None,
                    },
                    DbColumn {
                        name: "kind".to_string(),
                        ty: DbType::Text,
                        nullable: false,
                        primary_key: false,
                        auto_increment: false,
                        unique: false,
                        default: None,
                    },
                    DbColumn {
                        name: "read_allowed".to_string(),
                        ty: DbType::Boolean,
                        nullable: false,
                        primary_key: false,
                        auto_increment: false,
                        unique: false,
                        default: Some(ene_plugin_db::DbValue::Bool(true)),
                    },
                    DbColumn {
                        name: "write_allowed".to_string(),
                        ty: DbType::Boolean,
                        nullable: false,
                        primary_key: false,
                        auto_increment: false,
                        unique: false,
                        default: Some(ene_plugin_db::DbValue::Bool(false)),
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
            },
            DbTable {
                name: "calendar_events".to_string(),
                columns: vec![
                    column("id", DbType::Text, true, false),
                    DbColumn {
                        name: "account_id".to_string(),
                        ty: DbType::Text,
                        nullable: false,
                        primary_key: false,
                        auto_increment: false,
                        unique: false,
                        default: None,
                    },
                    DbColumn {
                        name: "title".to_string(),
                        ty: DbType::Text,
                        nullable: false,
                        primary_key: false,
                        auto_increment: false,
                        unique: false,
                        default: None,
                    },
                    DbColumn {
                        name: "description".to_string(),
                        ty: DbType::Text,
                        nullable: true,
                        primary_key: false,
                        auto_increment: false,
                        unique: false,
                        default: None,
                    },
                    DbColumn {
                        name: "location".to_string(),
                        ty: DbType::Text,
                        nullable: true,
                        primary_key: false,
                        auto_increment: false,
                        unique: false,
                        default: None,
                    },
                    DbColumn {
                        name: "start_at".to_string(),
                        ty: DbType::Integer,
                        nullable: false,
                        primary_key: false,
                        auto_increment: false,
                        unique: false,
                        default: None,
                    },
                    DbColumn {
                        name: "end_at".to_string(),
                        ty: DbType::Integer,
                        nullable: false,
                        primary_key: false,
                        auto_increment: false,
                        unique: false,
                        default: None,
                    },
                    DbColumn {
                        name: "timezone".to_string(),
                        ty: DbType::Text,
                        nullable: false,
                        primary_key: false,
                        auto_increment: false,
                        unique: false,
                        default: None,
                    },
                    DbColumn {
                        name: "attendees".to_string(),
                        ty: DbType::Text,
                        nullable: false,
                        primary_key: false,
                        auto_increment: false,
                        unique: false,
                        default: Some(ene_plugin_db::DbValue::Text("[]".to_string())),
                    },
                    DbColumn {
                        name: "status".to_string(),
                        ty: DbType::Text,
                        nullable: false,
                        primary_key: false,
                        auto_increment: false,
                        unique: false,
                        default: Some(ene_plugin_db::DbValue::Text("confirmed".to_string())),
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
            },
        ],
        indexes: vec![DbIndex {
            name: "idx_calendar_events_account_start".to_string(),
            table: "calendar_events".to_string(),
            columns: vec!["account_id".to_string(), "start_at".to_string()],
            unique: false,
        }],
    }
}

fn column(name: &str, ty: DbType, primary_key: bool, unique: bool) -> DbColumn {
    DbColumn {
        name: name.to_string(),
        ty,
        nullable: !primary_key,
        primary_key,
        auto_increment: false,
        unique,
        default: None,
    }
}
