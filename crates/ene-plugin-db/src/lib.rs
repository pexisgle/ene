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
//! ## Wire compatibility
//!
//! The DB IPC channel carries no version field — it is tied to the plugin
//! protocol version and the host/plugin pair shipped by the same release.
//! [`DbRequest::Batch`] is an *additive* extension to the wire protocol:
//!
//! - **Old host, new plugin**: an old host does not know the `Batch` variant
//!   and rejects the request at the JSON decode step, closing the connection.
//!   The plugin sees a transport error. Because plugin binaries and the core
//!   are released together, this only occurs when a plugin from a newer
//!   release is run against an older core, which is not a supported
//!   configuration.
//! - **New host, old plugin**: an old plugin never sends `Batch`, and every
//!   pre-existing request/response shape is unchanged, so old plugins keep
//!   working against a newer host.
//!
//! Do not extend the request or response enums with *breaking* changes
//! (removing or renaming variants); prefer additive variants plus, where a
//! field must become optional, `#[serde(default)]`.
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
