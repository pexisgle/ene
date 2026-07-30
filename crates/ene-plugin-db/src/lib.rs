//! # ene-plugin-db
//!
//! Feature-agnostic typed CRUD database API for plugin binaries.
//!
//! Plugins declare their schema via [`DbSchema`], then use the [`DbClient`] to
//! perform typed CRUD operations over a Unix socket connection to the core
//! DB server. The core server enforces table-name prefix isolation so that
//! each plugin can only access its own tables.
//!
//! Groups of writes that must stand or fall together can be applied atomically
//! with [`DbClient::batch`]: the server runs the whole [`DbWriteOp`] list in a
//! single `SQLite` transaction, committing all of them or rolling all of them
//! back, so a plugin never persists a half-applied update.
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use ene_plugin_db::{DbClient, DbSchema, DbTable, DbColumn, DbType, DbFilter, Row};
//! use std::collections::BTreeMap;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let mut client = DbClient::connect_with_token(
//!     std::path::Path::new("/tmp/db.sock"),
//!     "pre-shared-token",
//! ).await?;
//!
//! let schema = DbSchema {
//!     prefix: "my_plugin_".to_string(),
//!     tables: vec![DbTable {
//!         name: "my_plugin_items".to_string(),
//!         columns: vec![
//!             DbColumn { name: "id".into(), ty: DbType::Integer, primary_key: true, auto_increment: true, ..Default::default() },
//!             DbColumn { name: "content".into(), ty: DbType::Text, ..Default::default() },
//!         ],
//!     }],
//!     indexes: vec![],
//! };
//! client.declare_schema(schema).await?;
//!
//! let mut row = BTreeMap::new();
//! row.insert("content".into(), ene_plugin_db::DbValue::Text("hello".into()));
//! let rowid = client.insert("my_plugin_items", row).await?;
//! # Ok(())
//! # }
//! ```
#![warn(missing_docs)]

/// Client for connecting to the core DB server.
pub mod client;
/// IPC message types for DB operations.
pub mod messages;
/// Database value types, schema declarations, filters, and ordering.
pub mod types;

pub use client::{DbClient, DbError};
pub use messages::{DbBatchOpResult, DbErrorCode, DbRequest, DbResponse, DbWriteOp};
pub use types::{
    DbColumn, DbFilter, DbIndex, DbOrderBy, DbOrderDirection, DbSchema, DbTable, DbType, DbValue,
    Row,
};
