use ene_plugin_db::{DbColumn, DbSchema, DbTable, DbType, DbValue};

/// Returns the DB schema for the counter plugin's table.
///
/// The server enforces that every table name starts with the plugin's
/// declared prefix (`counter_`), so another plugin can never read or
/// write these rows.
pub fn counter_db_schema() -> DbSchema {
    DbSchema {
        prefix: "counter_".to_string(),
        tables: vec![DbTable {
            name: "counter_counts".to_string(),
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
                    unique: true,
                    default: None,
                },
                DbColumn {
                    name: "value".to_string(),
                    ty: DbType::Integer,
                    nullable: false,
                    primary_key: false,
                    auto_increment: false,
                    unique: false,
                    default: Some(DbValue::Int(0)),
                },
            ],
        }],
        indexes: vec![],
    }
}
