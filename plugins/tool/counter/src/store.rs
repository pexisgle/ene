use crate::schema::counter_db_schema;
use async_trait::async_trait;
use ene_plugin_db::{DbClient, DbError, DbFilter, DbValue, Row};
use ene_plugin_proto::ToolError;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Errors produced by counter store operations.
#[derive(Debug, thiserror::Error)]
pub enum CounterStoreError {
    /// IPC or database server error.
    #[error("database error: {0}")]
    Db(#[from] DbError),

    /// The DB server requires authentication and rejects unauthenticated
    /// connections, so connecting without a token can never succeed.
    #[error("DB auth token is required: the DB server rejects unauthenticated connections")]
    MissingAuthToken,

    #[error("corrupt row: missing or invalid column '{0}'")]
    CorruptRow(String),
}

impl From<CounterStoreError> for ToolError {
    fn from(e: CounterStoreError) -> Self {
        ToolError::internal(e.to_string())
    }
}

/// Storage backend for counter state.
///
/// The provider depends on this trait so tests can inject
/// [`InMemoryCounterStore`] instead of a live DB server.
#[async_trait]
pub trait CounterStore: Send + Sync {
    /// Returns the current value for `session_id`, or zero when absent.
    async fn get(&self, session_id: &str) -> Result<i64, CounterStoreError>;

    /// Increments the value for `session_id` and returns the new value.
    async fn increment(&self, session_id: &str) -> Result<i64, CounterStoreError>;

    async fn reset(&self) -> Result<(), CounterStoreError>;
}

/// Extracts the `value` column from a row, mapping missing or invalid
/// data to [`CounterStoreError::CorruptRow`].
fn row_value(row: &Row) -> Result<i64, CounterStoreError> {
    row.get("value")
        .and_then(DbValue::as_i64)
        .ok_or_else(|| CounterStoreError::CorruptRow("value".to_string()))
}

/// DB-backed store talking to the host-service `db` passenger over IPC.
///
/// The inner `DbClient` is wrapped in `Arc<tokio::sync::Mutex<>>` because
/// the client takes `&mut self` for every operation and the store is
/// shared across action instances and concurrent calls.
pub struct DbCounterStore {
    client: Arc<Mutex<DbClient>>,
}

impl DbCounterStore {
    /// Connects to the DB socket and declares the schema.
    ///
    /// `db_auth_token` is the pre-shared token the host delivers in
    /// [`ene_plugin_proto::SandboxConfigData`]; the DB server rejects
    /// unauthenticated connections.
    pub async fn new(
        socket_path: &Path,
        db_auth_token: Option<&str>,
    ) -> Result<Self, CounterStoreError> {
        let token = db_auth_token.ok_or(CounterStoreError::MissingAuthToken)?;
        let mut client = DbClient::connect_with_token(socket_path, token).await?;
        client.declare_schema(counter_db_schema()).await?;
        Ok(Self {
            client: Arc::new(Mutex::new(client)),
        })
    }
}

#[async_trait]
impl CounterStore for DbCounterStore {
    async fn get(&self, session_id: &str) -> Result<i64, CounterStoreError> {
        let mut client = self.client.lock().await;
        let rows = client
            .select(
                "counter_counts",
                &[],
                DbFilter::eq("session_id", DbValue::Text(session_id.to_string())),
                &[],
                Some(1),
            )
            .await?;
        Ok(rows.first().map_or(Ok(0), row_value)?)
    }

    async fn increment(&self, session_id: &str) -> Result<i64, CounterStoreError> {
        let mut client = self.client.lock().await;
        let filter = DbFilter::eq("session_id", DbValue::Text(session_id.to_string()));
        let rows = client
            .select("counter_counts", &[], filter.clone(), &[], Some(1))
            .await?;
        let next = if let Some(row) = rows.first() {
            let next = row_value(row)? + 1;
            let mut set = BTreeMap::new();
            set.insert("value".to_string(), DbValue::Int(next));
            client.update("counter_counts", set, filter).await?;
            next
        } else {
            let mut row = BTreeMap::new();
            row.insert(
                "session_id".to_string(),
                DbValue::Text(session_id.to_string()),
            );
            row.insert("value".to_string(), DbValue::Int(1));
            client.insert("counter_counts", row).await?;
            1
        };
        Ok(next)
    }

    async fn reset(&self) -> Result<(), CounterStoreError> {
        let mut client = self.client.lock().await;
        client.delete("counter_counts", DbFilter::Always).await?;
        Ok(())
    }
}

/// In-memory store used as a test double and as the recipe for mocking
/// the DB IPC boundary in action tests.
#[derive(Debug, Default)]
pub struct InMemoryCounterStore {
    counts: parking_lot::RwLock<BTreeMap<String, i64>>,
}

#[async_trait]
impl CounterStore for InMemoryCounterStore {
    async fn get(&self, session_id: &str) -> Result<i64, CounterStoreError> {
        Ok(self.counts.read().get(session_id).copied().unwrap_or(0))
    }

    async fn increment(&self, session_id: &str) -> Result<i64, CounterStoreError> {
        let mut counts = self.counts.write();
        let next = counts.get(session_id).copied().unwrap_or(0) + 1;
        counts.insert(session_id.to_string(), next);
        Ok(next)
    }

    async fn reset(&self) -> Result<(), CounterStoreError> {
        self.counts.write().clear();
        Ok(())
    }
}
