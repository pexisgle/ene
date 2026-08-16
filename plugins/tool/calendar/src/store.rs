use ene_plugin_db::{DbClient, DbError, DbFilter, DbValue, Row};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Kind of calendar account a provider implements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CalendarKind {
    /// Events stored in the plugin's own DB tables (host-service `db`).
    Local,
}

impl CalendarKind {
    /// Stable wire value matching the `calendar_accounts.kind` column.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
        }
    }
}

impl std::fmt::Display for CalendarKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A calendar account; owns a set of events and its own read/write permissions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarAccount {
    /// Stable account identifier.
    pub id: String,
    /// Display name, unique within the store.
    pub name: String,
    /// Provider kind that owns this account's events.
    pub kind: CalendarKind,
    /// Whether read operations are permitted for this calendar.
    pub read_allowed: bool,
    /// Whether write operations are permitted for this calendar.
    pub write_allowed: bool,
    /// RFC3339 creation timestamp.
    pub created_at: String,
    /// RFC3339 last-modification timestamp.
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarEvent {
    /// Stable event identifier (unique per account).
    pub id: String,
    pub account_id: String,
    /// Human-readable title.
    pub title: String,
    /// Free-form notes; empty when unset.
    pub description: String,
    /// Location string; empty when unset.
    pub location: String,
    /// Start time, epoch milliseconds (UTC).
    pub start_at: i64,
    /// End time, epoch milliseconds (UTC).
    pub end_at: i64,
    /// IANA timezone name (or fixed offset label) the event is expressed in.
    pub timezone: String,
    /// Attendee identifiers (emails, names, ...).
    pub attendees: Vec<String>,
    /// `confirmed`, `tentative`, or `cancelled`.
    pub status: String,
    /// RFC3339 creation timestamp.
    pub created_at: String,
    /// RFC3339 last-modification timestamp.
    pub updated_at: String,
}

/// Input for creating an event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarEventInput {
    /// Human-readable title.
    pub title: String,
    /// Free-form notes.
    #[serde(default)]
    pub description: String,
    /// Location string.
    #[serde(default)]
    pub location: String,
    /// Start time, RFC3339 with offset (e.g. `2026-08-03T10:00:00+09:00`).
    pub start: String,
    /// End time, RFC3339 with offset; must be after `start`.
    pub end: String,
    /// IANA timezone name for display; when absent the start offset is used.
    #[serde(default)]
    pub timezone: String,
    /// Attendee identifiers.
    #[serde(default)]
    pub attendees: Vec<String>,
    /// `confirmed` (default) or `tentative`.
    #[serde(default)]
    pub status: String,
}

/// Partial changes applied by `calendar.update_event`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CalendarEventChanges {
    /// Replace the title.
    pub title: Option<String>,
    /// Replace the notes.
    pub description: Option<String>,
    /// Replace the location.
    pub location: Option<String>,
    /// Replace the start time (RFC3339 with offset).
    pub start: Option<String>,
    /// Replace the end time (RFC3339 with offset).
    pub end: Option<String>,
    /// Replace the timezone label.
    pub timezone: Option<String>,
    /// Replace the attendee list.
    pub attendees: Option<Vec<String>>,
    /// Replace the status.
    pub status: Option<String>,
}

/// A contiguous free interval computed by `calendar.find_free_slots`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FreeSlot {
    /// Slot start, epoch milliseconds (UTC).
    pub start_at: i64,
    /// Slot end, epoch milliseconds (UTC).
    pub end_at: i64,
    /// Slot length in minutes.
    pub duration_min: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum CalendarStoreError {
    /// Underlying DB/transport error.
    #[error("database error: {0}")]
    Db(#[from] DbError),
    /// The DB IPC auth token was not configured.
    #[error("missing DB auth token")]
    MissingAuthToken,
    /// `calendar_accounts` has no row with the given id.
    #[error("calendar not found: {0}")]
    AccountNotFound(String),
    /// `calendar_events` has no row with the given id.
    #[error("event not found: {0}")]
    EventNotFound(String),
    /// An event id was found, but not under the requested account.
    #[error("event {event_id} not found in calendar {account_id}")]
    EventNotFoundInAccount {
        /// Account that was searched.
        account_id: String,
        /// Event that was searched for.
        event_id: String,
    },
    /// No provider is registered for the account's kind.
    #[error("no provider registered for calendar kind '{0}'")]
    UnknownProvider(String),
    /// `add_calendar` was given a name that already exists.
    #[error("a calendar named '{0}' already exists")]
    DuplicateName(String),
    /// Read operation on a calendar with `read_allowed = false`.
    #[error("read access denied for calendar: {0}")]
    ReadDenied(String),
    /// Write operation on a calendar with `write_allowed = false`.
    #[error("write access denied for calendar: {0}")]
    WriteDenied(String),
    /// An event ended before it started (or a slot ended at its start).
    #[error("invalid event time: end must be after start")]
    InvalidTimeRange,
    /// An input string could not be parsed as RFC3339.
    #[error("invalid RFC3339 timestamp '{0}'")]
    InvalidTimestamp(String),
    /// Unknown event status string.
    #[error("invalid status '{0}'; expected 'confirmed', 'tentative', or 'cancelled'")]
    InvalidStatus(String),
    /// `create_event` was asked for a status that is a lifecycle state only.
    #[error("invalid status '{0}' for a new event; use 'confirmed' or 'tentative'")]
    InvalidNewStatus(String),
    /// `add_calendar` was given an empty name.
    #[error("calendar name must not be empty")]
    EmptyAccountName,
    /// A stored row could not be interpreted.
    #[error("corrupt row: {0}")]
    CorruptRow(String),
}

pub fn parse_rfc3339_ms(value: &str) -> Result<i64, CalendarStoreError> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.timestamp_millis())
        .map_err(|_| CalendarStoreError::InvalidTimestamp(value.to_string()))
}

fn offset_label(value: &str) -> Option<String> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.offset().to_string())
}

/// DB-backed calendar store (host-service `db` passenger).
pub struct CalendarStore {
    client: tokio::sync::Mutex<DbClient>,
}

impl CalendarStore {
    /// Connects to the host DB service, declares the calendar schema, and
    /// returns the store.
    pub async fn new(
        socket_path: &Path,
        auth_token: Option<&str>,
    ) -> Result<Self, CalendarStoreError> {
        let token = auth_token.ok_or(CalendarStoreError::MissingAuthToken)?;
        let mut client = DbClient::connect_with_token(socket_path, token).await?;
        client
            .declare_schema(crate::schema::calendar_db_schema())
            .await?;
        Ok(Self {
            client: tokio::sync::Mutex::new(client),
        })
    }

    /// Acquires the DB client guard.
    ///
    /// All requests go through this accessor so the lock is always taken in
    /// the same place; `tokio::sync::Mutex` keeps the guard await-safe.
    async fn client(&self) -> tokio::sync::MutexGuard<'_, DbClient> {
        self.client.lock().await
    }

    /// Lists all calendar accounts, ordered by creation.
    pub async fn list_accounts(&self) -> Result<Vec<CalendarAccount>, CalendarStoreError> {
        let rows = self
            .client()
            .await
            .select("calendar_accounts", &[], DbFilter::Always, &[], None)
            .await?;
        rows.iter().map(CalendarAccount::from_row).collect()
    }

    pub async fn get_account(&self, id: &str) -> Result<CalendarAccount, CalendarStoreError> {
        let rows = self
            .client()
            .await
            .select(
                "calendar_accounts",
                &[],
                DbFilter::eq("id", DbValue::Text(id.to_string())),
                &[],
                Some(1),
            )
            .await?;
        let row = rows
            .first()
            .ok_or_else(|| CalendarStoreError::AccountNotFound(id.to_string()))?;
        CalendarAccount::from_row(row)
    }

    pub async fn find_account_by_name(
        &self,
        name: &str,
    ) -> Result<Option<CalendarAccount>, CalendarStoreError> {
        let rows = self
            .client()
            .await
            .select(
                "calendar_accounts",
                &[],
                DbFilter::eq("name", DbValue::Text(name.to_string())),
                &[],
                Some(1),
            )
            .await?;
        rows.first().map(CalendarAccount::from_row).transpose()
    }

    /// Creates a calendar account with the given `id`. New accounts default
    /// to `read_allowed = true` and `write_allowed = false`
    /// (deny-by-default writes); writes become possible only after
    /// `set_permissions` grants write access, and every write still requires
    /// per-operation approval.
    ///
    /// The caller supplies `id` so the approval gate can pin the target
    /// (`calendar:<id>`) before the row exists.
    pub async fn add_account(
        &self,
        id: &str,
        name: &str,
        kind: CalendarKind,
    ) -> Result<CalendarAccount, CalendarStoreError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(CalendarStoreError::EmptyAccountName);
        }
        if self.find_account_by_name(name).await?.is_some() {
            return Err(CalendarStoreError::DuplicateName(name.to_string()));
        }
        let now = now_rfc3339();
        let mut row: Row = BTreeMap::new();
        row.insert("id".to_string(), DbValue::Text(id.to_string()));
        row.insert("name".to_string(), DbValue::Text(name.to_string()));
        row.insert("kind".to_string(), DbValue::Text(kind.as_str().to_string()));
        row.insert("read_allowed".to_string(), DbValue::Bool(true));
        row.insert("write_allowed".to_string(), DbValue::Bool(false));
        row.insert("created_at".to_string(), DbValue::Text(now.clone()));
        row.insert("updated_at".to_string(), DbValue::Text(now));
        self.client().await.insert("calendar_accounts", row).await?;
        self.get_account(id).await
    }

    /// Updates the read/write permission flags of a calendar.
    pub async fn set_permissions(
        &self,
        account_id: &str,
        read_allowed: Option<bool>,
        write_allowed: Option<bool>,
    ) -> Result<CalendarAccount, CalendarStoreError> {
        if read_allowed.is_none() && write_allowed.is_none() {
            return self.get_account(account_id).await;
        }
        let mut set: Row = BTreeMap::new();
        if let Some(v) = read_allowed {
            set.insert("read_allowed".to_string(), DbValue::Bool(v));
        }
        if let Some(v) = write_allowed {
            set.insert("write_allowed".to_string(), DbValue::Bool(v));
        }
        set.insert("updated_at".to_string(), DbValue::Text(now_rfc3339()));
        self.client()
            .await
            .update(
                "calendar_accounts",
                set,
                DbFilter::eq("id", DbValue::Text(account_id.to_string())),
            )
            .await?;
        self.get_account(account_id).await
    }

    /// Unlinks an account: deletes the account and all of its events in one
    /// transaction. External providers must override or observe this so that
    /// the disconnect is reflected immediately (the local provider deletes
    /// local rows; the provider registry remains free of stale accounts
    /// because the account row is gone).
    pub async fn remove_account(&self, account_id: &str) -> Result<(), CalendarStoreError> {
        if self.get_account(account_id).await.is_err() {
            return Err(CalendarStoreError::AccountNotFound(account_id.to_string()));
        }
        let ops = vec![
            ene_plugin_db::DbWriteOp::Delete {
                table: "calendar_events".to_string(),
                filter: DbFilter::eq("account_id", DbValue::Text(account_id.to_string())),
            },
            ene_plugin_db::DbWriteOp::Delete {
                table: "calendar_accounts".to_string(),
                filter: DbFilter::eq("id", DbValue::Text(account_id.to_string())),
            },
        ];
        self.client().await.batch(ops).await?;
        Ok(())
    }

    /// Returns the account after enforcing its `read_allowed` flag.
    pub async fn require_readable(
        &self,
        account_id: &str,
    ) -> Result<CalendarAccount, CalendarStoreError> {
        let account = self.get_account(account_id).await?;
        if !account.read_allowed {
            return Err(CalendarStoreError::ReadDenied(account_id.to_string()));
        }
        Ok(account)
    }

    /// Returns the account after enforcing its `write_allowed` flag.
    pub async fn require_writable(
        &self,
        account_id: &str,
    ) -> Result<CalendarAccount, CalendarStoreError> {
        let account = self.get_account(account_id).await?;
        if !account.write_allowed {
            return Err(CalendarStoreError::WriteDenied(account_id.to_string()));
        }
        Ok(account)
    }

    /// Lists events of an account. `start_ms`/`end_ms` bound the window;
    /// cancelled events are excluded unless `include_cancelled` is set.
    pub async fn list_events(
        &self,
        account_id: &str,
        start_ms: Option<i64>,
        end_ms: Option<i64>,
        include_cancelled: bool,
    ) -> Result<Vec<CalendarEvent>, CalendarStoreError> {
        let mut filters: Vec<DbFilter> = vec![DbFilter::eq(
            "account_id",
            DbValue::Text(account_id.to_string()),
        )];
        if !include_cancelled {
            filters.push(DbFilter::ne(
                "status",
                DbValue::Text("cancelled".to_string()),
            ));
        }
        if let Some(start) = start_ms {
            filters.push(DbFilter::Ge {
                column: "start_at".to_string(),
                value: DbValue::Int(start),
            });
        }
        if let Some(end) = end_ms {
            filters.push(DbFilter::Lt {
                column: "start_at".to_string(),
                value: DbValue::Int(end),
            });
        }
        let filter = match filters.len() {
            1 => filters.remove(0),
            _ => DbFilter::And(filters),
        };
        let rows = self
            .client()
            .await
            .select(
                "calendar_events",
                &[],
                filter,
                &[ene_plugin_db::DbOrderBy::asc("start_at")],
                None,
            )
            .await?;
        rows.iter().map(CalendarEvent::from_row).collect()
    }

    pub async fn get_event(
        &self,
        account_id: &str,
        event_id: &str,
    ) -> Result<CalendarEvent, CalendarStoreError> {
        let rows = self
            .client()
            .await
            .select(
                "calendar_events",
                &[],
                DbFilter::And(vec![
                    DbFilter::eq("id", DbValue::Text(event_id.to_string())),
                    DbFilter::eq("account_id", DbValue::Text(account_id.to_string())),
                ]),
                &[],
                Some(1),
            )
            .await?;
        let row = rows
            .first()
            .ok_or_else(|| CalendarStoreError::EventNotFoundInAccount {
                account_id: account_id.to_string(),
                event_id: event_id.to_string(),
            })?;
        CalendarEvent::from_row(row)
    }

    /// Lists events whose time range overlaps `[start_ms, end_ms)`, excluding
    /// cancelled ones.
    ///
    /// Unlike [`Self::list_events`], whose `start_at`-based window drops
    /// events that start before the window, this predicate also returns
    /// events that *span* the window — the set busy-time search needs, so a
    /// 9:00–11:00 meeting blocks a 10:00 slot in a 10:00–12:00 search.
    pub async fn list_events_overlapping(
        &self,
        account_id: &str,
        start_ms: i64,
        end_ms: i64,
    ) -> Result<Vec<CalendarEvent>, CalendarStoreError> {
        let rows = self
            .client()
            .await
            .select(
                "calendar_events",
                &[],
                DbFilter::And(vec![
                    DbFilter::eq("account_id", DbValue::Text(account_id.to_string())),
                    DbFilter::ne("status", DbValue::Text("cancelled".to_string())),
                    DbFilter::Gt {
                        column: "end_at".to_string(),
                        value: DbValue::Int(start_ms),
                    },
                    DbFilter::Lt {
                        column: "start_at".to_string(),
                        value: DbValue::Int(end_ms),
                    },
                ]),
                &[ene_plugin_db::DbOrderBy::asc("start_at")],
                None,
            )
            .await?;
        rows.iter().map(CalendarEvent::from_row).collect()
    }

    pub async fn create_event(
        &self,
        account_id: &str,
        input: &CalendarEventInput,
    ) -> Result<CalendarEvent, CalendarStoreError> {
        let (start_at, end_at, timezone) = resolve_times(input)?;
        let now = now_rfc3339();
        let id = uuid::Uuid::new_v4().to_string();
        let mut row: Row = BTreeMap::new();
        row.insert("id".to_string(), DbValue::Text(id.clone()));
        row.insert(
            "account_id".to_string(),
            DbValue::Text(account_id.to_string()),
        );
        row.insert(
            "title".to_string(),
            DbValue::Text(input.title.trim().to_string()),
        );
        let description = input.description.trim();
        if description.is_empty() {
            row.insert("description".to_string(), DbValue::Null);
        } else {
            row.insert(
                "description".to_string(),
                DbValue::Text(description.to_string()),
            );
        }
        let location = input.location.trim();
        if location.is_empty() {
            row.insert("location".to_string(), DbValue::Null);
        } else {
            row.insert("location".to_string(), DbValue::Text(location.to_string()));
        }
        row.insert("start_at".to_string(), DbValue::Int(start_at));
        row.insert("end_at".to_string(), DbValue::Int(end_at));
        row.insert("timezone".to_string(), DbValue::Text(timezone));
        let attendees_json = serde_json::to_string(&input.attendees)
            .map_err(|e| CalendarStoreError::CorruptRow(format!("attendees serialize: {e}")))?;
        row.insert("attendees".to_string(), DbValue::Text(attendees_json));
        let status = validate_new_status(&input.status)?;
        row.insert("status".to_string(), DbValue::Text(status));
        row.insert("created_at".to_string(), DbValue::Text(now.clone()));
        row.insert("updated_at".to_string(), DbValue::Text(now));
        self.client().await.insert("calendar_events", row).await?;
        self.get_event(account_id, &id).await
    }

    /// Applies partial changes to an event and returns the updated event.
    pub async fn update_event(
        &self,
        account_id: &str,
        event_id: &str,
        changes: &CalendarEventChanges,
    ) -> Result<CalendarEvent, CalendarStoreError> {
        let current = self.get_event(account_id, event_id).await?;
        let mut set: Row = BTreeMap::new();
        if let Some(title) = &changes.title {
            set.insert("title".to_string(), DbValue::Text(title.trim().to_string()));
        }
        if let Some(description) = &changes.description {
            let description = description.trim();
            if description.is_empty() {
                set.insert("description".to_string(), DbValue::Null);
            } else {
                set.insert(
                    "description".to_string(),
                    DbValue::Text(description.to_string()),
                );
            }
        }
        if let Some(location) = &changes.location {
            let location = location.trim();
            if location.is_empty() {
                set.insert("location".to_string(), DbValue::Null);
            } else {
                set.insert("location".to_string(), DbValue::Text(location.to_string()));
            }
        }
        let start_at = match &changes.start {
            Some(value) => Some(parse_rfc3339_ms(value)?),
            None => None,
        };
        let end_at = match &changes.end {
            Some(value) => Some(parse_rfc3339_ms(value)?),
            None => None,
        };
        if let Some(v) = start_at {
            set.insert("start_at".to_string(), DbValue::Int(v));
        }
        if let Some(v) = end_at {
            set.insert("end_at".to_string(), DbValue::Int(v));
        }
        if let Some(tz) = &changes.timezone {
            set.insert("timezone".to_string(), DbValue::Text(tz.clone()));
        }
        if let Some(attendees) = &changes.attendees {
            let attendees_json = serde_json::to_string(attendees)
                .map_err(|e| CalendarStoreError::CorruptRow(format!("attendees serialize: {e}")))?;
            set.insert("attendees".to_string(), DbValue::Text(attendees_json));
        }
        if let Some(status) = &changes.status {
            set.insert(
                "status".to_string(),
                DbValue::Text(validate_status(status)?),
            );
        }
        let final_start = start_at.unwrap_or(current.start_at);
        let final_end = end_at.unwrap_or(current.end_at);
        if final_start >= final_end {
            return Err(CalendarStoreError::InvalidTimeRange);
        }
        if set.is_empty() {
            return Ok(current);
        }
        set.insert("updated_at".to_string(), DbValue::Text(now_rfc3339()));
        self.client()
            .await
            .update(
                "calendar_events",
                set,
                DbFilter::And(vec![
                    DbFilter::eq("id", DbValue::Text(event_id.to_string())),
                    DbFilter::eq("account_id", DbValue::Text(account_id.to_string())),
                ]),
            )
            .await?;
        self.get_event(account_id, event_id).await
    }

    /// Cancels an event by marking it `cancelled` (soft delete).
    ///
    /// The row is kept so a cancellation stays inspectable via
    /// `include_cancelled` and reversible through `update_event`; plain
    /// listings and free-slot search exclude it. Returns the updated event.
    pub async fn cancel_event(
        &self,
        account_id: &str,
        event_id: &str,
    ) -> Result<CalendarEvent, CalendarStoreError> {
        self.get_event(account_id, event_id).await?;
        let mut set: Row = BTreeMap::new();
        set.insert("status".to_string(), DbValue::Text("cancelled".to_string()));
        set.insert("updated_at".to_string(), DbValue::Text(now_rfc3339()));
        self.client()
            .await
            .update(
                "calendar_events",
                set,
                DbFilter::And(vec![
                    DbFilter::eq("id", DbValue::Text(event_id.to_string())),
                    DbFilter::eq("account_id", DbValue::Text(account_id.to_string())),
                ]),
            )
            .await?;
        self.get_event(account_id, event_id).await
    }
}

fn resolve_times(input: &CalendarEventInput) -> Result<(i64, i64, String), CalendarStoreError> {
    let start_at = parse_rfc3339_ms(&input.start)?;
    let end_at = parse_rfc3339_ms(&input.end)?;
    if start_at >= end_at {
        return Err(CalendarStoreError::InvalidTimeRange);
    }
    let timezone = if input.timezone.trim().is_empty() {
        offset_label(&input.start).unwrap_or_else(|| "UTC".to_string())
    } else {
        input.timezone.trim().to_string()
    };
    Ok((start_at, end_at, timezone))
}

fn validate_status(status: &str) -> Result<String, CalendarStoreError> {
    let status = status.trim();
    if status.is_empty() {
        return Ok("confirmed".to_string());
    }
    match status {
        "confirmed" | "tentative" | "cancelled" => Ok(status.to_string()),
        _ => Err(CalendarStoreError::InvalidStatus(status.to_string())),
    }
}

/// Statuses accepted when creating an event; `cancelled` is a lifecycle
/// state reached via `cancel_event`/`update_event`, not a creation input.
fn validate_new_status(status: &str) -> Result<String, CalendarStoreError> {
    let status = status.trim();
    if status.is_empty() {
        return Ok("confirmed".to_string());
    }
    match status {
        "confirmed" | "tentative" => Ok(status.to_string()),
        _ => Err(CalendarStoreError::InvalidNewStatus(status.to_string())),
    }
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Formats an epoch-millis instant for human-readable previews. Uses the
/// stored timezone when it is a fixed offset, otherwise UTC with the
/// timezone label appended.
fn format_preview_time(ms: i64, timezone: &str) -> String {
    if let Ok(offset) = timezone.parse::<chrono::FixedOffset>()
        && let Some(dt) = chrono::DateTime::from_timestamp_millis(ms)
    {
        return dt
            .with_timezone(&offset)
            .format("%Y-%m-%d %H:%M %:z")
            .to_string();
    }
    let dt = chrono::DateTime::from_timestamp_millis(ms).map_or_else(
        || ms.to_string(),
        |d| d.format("%Y-%m-%d %H:%M UTC").to_string(),
    );
    if timezone.trim().is_empty() || timezone == "UTC" {
        dt
    } else {
        format!("{dt} ({timezone})")
    }
}

/// Builds a human-readable diff of an event update for the approval preview.
pub fn format_event_changes(
    account_name: &str,
    event: &CalendarEvent,
    changes: &CalendarEventChanges,
) -> String {
    let preview = |ms: i64, tz: &str| format_preview_time(ms, tz);
    let changes_tz = changes.timezone.as_deref().unwrap_or(&event.timezone);
    let mut parts: Vec<String> = Vec::new();
    if let Some(title) = &changes.title {
        parts.push(format!("title: '{}' -> '{}'", event.title, title.trim()));
    }
    if let Some(description) = &changes.description {
        parts.push(format!(
            "notes: '{}' -> '{}'",
            truncate_preview(&event.description),
            truncate_preview(description),
        ));
    }
    if let Some(location) = &changes.location {
        parts.push(format!(
            "location: '{}' -> '{}'",
            truncate_preview(&event.location),
            truncate_preview(location),
        ));
    }
    match (&changes.start, &changes.end) {
        (Some(start), Some(end)) => {
            let start_ms = parse_rfc3339_ms(start).unwrap_or(0);
            let end_ms = parse_rfc3339_ms(end).unwrap_or(0);
            parts.push(format!(
                "time: {} -> {} - {}",
                preview(event.start_at, &event.timezone),
                preview(start_ms, changes_tz),
                preview(end_ms, changes_tz),
            ));
        }
        (Some(start), None) => {
            let start_ms = parse_rfc3339_ms(start).unwrap_or(0);
            parts.push(format!(
                "start: {} -> {}",
                preview(event.start_at, &event.timezone),
                preview(start_ms, changes_tz),
            ));
        }
        (None, Some(end)) => {
            let end_ms = parse_rfc3339_ms(end).unwrap_or(0);
            parts.push(format!(
                "end: {} -> {}",
                preview(event.end_at, &event.timezone),
                preview(end_ms, changes_tz),
            ));
        }
        (None, None) => {}
    }
    if let Some(tz) = &changes.timezone
        && tz.trim() != event.timezone
    {
        parts.push(format!("timezone: {} -> {}", event.timezone, tz.trim()));
    }
    if let Some(attendees) = &changes.attendees {
        parts.push(format!(
            "attendees: [{}] -> [{}]",
            event.attendees.join(", "),
            attendees.join(", ")
        ));
    }
    if let Some(status) = &changes.status {
        parts.push(format!("status: {} -> {}", event.status, status.trim()));
    }
    if parts.is_empty() {
        "no changes".to_string()
    } else {
        format!(
            "update event '{}' on '{}': {}",
            event.title,
            account_name,
            parts.join("; ")
        )
    }
}

/// Shortens a value for preview lines; long notes keep their start so the
/// user can spot the change without flooding the prompt.
fn truncate_preview(value: &str) -> String {
    const MAX_PREVIEW_CHARS: usize = 60;
    let trimmed = value.trim();
    if trimmed.chars().count() <= MAX_PREVIEW_CHARS {
        return trimmed.to_string();
    }
    format!(
        "{}...",
        trimmed.chars().take(MAX_PREVIEW_CHARS).collect::<String>()
    )
}

/// Formats an event time range for approval previews.
pub fn format_event_window(event: &CalendarEvent) -> String {
    format!(
        "{} - {} ({})",
        format_preview_time(event.start_at, &event.timezone),
        format_preview_time(event.end_at, &event.timezone),
        event.timezone,
    )
}

impl CalendarAccount {
    fn from_row(row: &Row) -> Result<Self, CalendarStoreError> {
        let text = |col: &str| -> Result<String, CalendarStoreError> {
            row.get(col)
                .and_then(DbValue::as_str)
                .map(ToOwned::to_owned)
                .ok_or_else(|| CalendarStoreError::CorruptRow(col.to_string()))
        };
        let kind_str = text("kind")?;
        let kind = match kind_str.as_str() {
            "local" => CalendarKind::Local,
            other => return Err(CalendarStoreError::CorruptRow(format!("kind '{other}'"))),
        };
        let read_allowed = row
            .get("read_allowed")
            .and_then(DbValue::as_bool)
            .ok_or_else(|| CalendarStoreError::CorruptRow("read_allowed".to_string()))?;
        let write_allowed = row
            .get("write_allowed")
            .and_then(DbValue::as_bool)
            .ok_or_else(|| CalendarStoreError::CorruptRow("write_allowed".to_string()))?;
        Ok(Self {
            id: text("id")?,
            name: text("name")?,
            kind,
            read_allowed,
            write_allowed,
            created_at: text("created_at")?,
            updated_at: text("updated_at")?,
        })
    }
}

impl CalendarEvent {
    fn from_row(row: &Row) -> Result<Self, CalendarStoreError> {
        let text = |col: &str| -> Result<String, CalendarStoreError> {
            row.get(col)
                .and_then(DbValue::as_str)
                .map(ToOwned::to_owned)
                .ok_or_else(|| CalendarStoreError::CorruptRow(col.to_string()))
        };
        let int = |col: &str| -> Result<i64, CalendarStoreError> {
            row.get(col)
                .and_then(DbValue::as_i64)
                .ok_or_else(|| CalendarStoreError::CorruptRow(col.to_string()))
        };
        let description = row
            .get("description")
            .and_then(DbValue::as_str)
            .unwrap_or_default()
            .to_string();
        let location = row
            .get("location")
            .and_then(DbValue::as_str)
            .unwrap_or_default()
            .to_string();
        let attendees: Vec<String> = serde_json::from_str(&text("attendees")?)
            .map_err(|_| CalendarStoreError::CorruptRow("attendees".to_string()))?;
        Ok(Self {
            id: text("id")?,
            account_id: text("account_id")?,
            title: text("title")?,
            description,
            location,
            start_at: int("start_at")?,
            end_at: int("end_at")?,
            timezone: text("timezone")?,
            attendees,
            status: text("status")?,
            created_at: text("created_at")?,
            updated_at: text("updated_at")?,
        })
    }
}

/// Computes free slots of `duration_min` minutes within `[start_ms, end_ms)`,
/// skipping windows occupied by any of `events` (overlapping events are
/// merged). Returned slots are aligned to the window start in
/// `duration_min` steps. Pure function so the free-slot search is
/// unit-testable without a database.
pub fn find_free_slots(
    events: &[CalendarEvent],
    start_ms: i64,
    end_ms: i64,
    duration_min: u64,
) -> Vec<FreeSlot> {
    let duration_ms =
        i64::try_from(duration_min).map_or(i64::MAX, |m| m.checked_mul(60_000).unwrap_or(i64::MAX));
    if duration_ms <= 0 || end_ms <= start_ms {
        return Vec::new();
    }
    let mut busy: Vec<(i64, i64)> = events
        .iter()
        .filter(|e| e.status != "cancelled")
        .map(|e| (e.start_at.max(start_ms), e.end_at.min(end_ms)))
        .filter(|(s, e)| *e > *s)
        .collect();
    busy.sort_unstable();
    let mut merged: Vec<(i64, i64)> = Vec::new();
    for (s, e) in busy {
        if let Some(last) = merged.last_mut()
            && e <= last.1
        {
            continue;
        }
        if let Some(last) = merged.last_mut()
            && s <= last.1
        {
            last.1 = e;
        } else {
            merged.push((s, e));
        }
    }
    let mut slots: Vec<FreeSlot> = Vec::new();
    let mut cursor = start_ms;
    for (busy_start, busy_end) in merged {
        emit_slots(&mut slots, cursor, busy_start, duration_ms);
        cursor = cursor.max(busy_end);
    }
    emit_slots(&mut slots, cursor, end_ms, duration_ms);
    slots
}

fn emit_slots(slots: &mut Vec<FreeSlot>, gap_start: i64, gap_end: i64, duration_ms: i64) {
    let mut cursor = gap_start;
    while cursor + duration_ms <= gap_end {
        slots.push(FreeSlot {
            start_at: cursor,
            end_at: cursor + duration_ms,
            duration_min: u64::try_from(duration_ms / 60_000).unwrap_or(u64::MAX),
        });
        cursor += duration_ms;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_db::make_store;
    use ene_plugin_proto::transport::cleanup_path;

    fn sample_input(title: &str, start: &str, end: &str) -> CalendarEventInput {
        CalendarEventInput {
            title: title.to_string(),
            description: "notes".to_string(),
            location: "room 1".to_string(),
            start: start.to_string(),
            end: end.to_string(),
            timezone: "Asia/Tokyo".to_string(),
            attendees: vec!["alice@example.com".to_string()],
            status: String::new(),
        }
    }

    fn base_event(start_at: i64, end_at: i64) -> CalendarEvent {
        CalendarEvent {
            id: "e1".to_string(),
            account_id: "a1".to_string(),
            title: "standup".to_string(),
            description: String::new(),
            location: String::new(),
            start_at,
            end_at,
            timezone: "UTC".to_string(),
            attendees: Vec::new(),
            status: "confirmed".to_string(),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn parses_rfc3339_timestamps() {
        let ms = parse_rfc3339_ms("2026-08-03T10:00:00+09:00").expect("valid timestamp");
        assert_eq!(ms, 1_785_718_800_000);
    }

    #[test]
    fn rejects_invalid_timestamps() {
        assert!(matches!(
            parse_rfc3339_ms("not-a-time"),
            Err(CalendarStoreError::InvalidTimestamp(_))
        ));
    }

    #[test]
    fn free_slots_splits_gaps_into_duration_chunks() {
        let day = 24 * 3_600_000;
        let events = vec![
            base_event(9 * 3_600_000, 10 * 3_600_000),
            base_event(12 * 3_600_000, 13 * 3_600_000),
        ];
        let slots = find_free_slots(&events, 0, day, 30);
        assert_eq!(slots.len(), 18 + 4 + 22, "9h + 2h + 11h of 30-min chunks");
        assert_eq!(slots[0].start_at, 0);
        assert_eq!(slots[0].end_at, 30 * 60_000);
        assert_eq!(slots.last().expect("non-empty").end_at, day);
        assert!(slots.iter().all(|s| s.end_at <= 9 * 3_600_000
            || (s.start_at >= 10 * 3_600_000 && s.end_at <= 12 * 3_600_000)
            || s.start_at >= 13 * 3_600_000));
        assert!(slots.iter().all(|s| (s.end_at - s.start_at) == 30 * 60_000));
    }

    #[test]
    fn free_slots_merges_overlapping_events() {
        let events = vec![
            base_event(10 * 3_600_000, 12 * 3_600_000),
            base_event(11 * 3_600_000, 13 * 3_600_000),
            base_event(12 * 3_600_000, 13 * 3_600_000),
        ];
        let slots = find_free_slots(&events, 0, 24 * 3_600_000, 60);
        assert_eq!(
            slots.len(),
            10 + 11,
            "10h before the merged busy block, 11h after"
        );
        assert_eq!(slots.first().expect("non-empty").end_at, 3_600_000);
        assert!(
            slots
                .iter()
                .all(|s| s.end_at <= 10 * 3_600_000 || s.start_at >= 13 * 3_600_000),
            "no slot may fall inside the merged busy block [10h, 13h)"
        );
        assert_eq!(
            slots[10].start_at,
            13 * 3_600_000,
            "the first slot after the merged busy block starts exactly at 13h"
        );
    }

    #[test]
    fn free_slots_skips_cancelled_events() {
        let mut event = base_event(10 * 3_600_000, 12 * 3_600_000);
        event.status = "cancelled".to_string();
        let slots = find_free_slots(&[event], 0, 24 * 3_600_000, 60);
        assert_eq!(slots.len(), 24);
    }

    #[test]
    fn free_slots_respects_duration_and_boundaries() {
        let events = vec![base_event(60 * 60_000, 3 * 60 * 60_000)];
        let slots = find_free_slots(&events, 0, 24 * 60 * 60_000, 120);
        assert!(slots.iter().all(|s| s.duration_min == 120));
        assert!(
            slots
                .iter()
                .all(|s| s.end_at <= 3 * 60 * 60_000 || s.start_at >= 3 * 60 * 60_000)
        );
        let too_long = find_free_slots(&events, 0, 10 * 60 * 60_000, 8 * 60);
        assert!(too_long.is_empty());
    }

    #[test]
    fn free_slots_empty_calendar_yields_aligned_window() {
        let slots = find_free_slots(&[], 1000, 1000 + 3_600_000, 60);
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].start_at, 1000);
        assert_eq!(slots[0].end_at, 1000 + 3_600_000);
    }

    #[test]
    fn preview_lists_changes_only() {
        let event = CalendarEvent {
            start_at: 1_785_718_800_000,
            ..base_event(0, 0)
        };
        let changes = CalendarEventChanges {
            title: Some("retro".to_string()),
            start: Some("2026-08-03T11:00:00+09:00".to_string()),
            ..CalendarEventChanges::default()
        };
        let preview = format_event_changes("work", &event, &changes);
        assert!(preview.contains("standup"), "preview names the old title");
        assert!(preview.contains("retro"), "preview names the new title");
        assert!(preview.contains("work"), "preview names the calendar");
        assert!(!preview.contains("no changes"));
    }

    #[test]
    fn preview_shows_timezone_only_changes() {
        let event = base_event(1_785_718_800_000, 1_785_722_400_000);
        let changes = CalendarEventChanges {
            timezone: Some("America/New_York".to_string()),
            ..CalendarEventChanges::default()
        };
        let preview = format_event_changes("work", &event, &changes);
        assert!(
            preview.contains("timezone: UTC -> America/New_York"),
            "timezone-only updates must not read as 'no changes': {preview}"
        );
        assert!(!preview.contains("no changes"));
    }

    #[test]
    fn preview_ignores_noop_timezone() {
        let event = CalendarEvent {
            timezone: "Asia/Tokyo".to_string(),
            ..base_event(0, 0)
        };
        let changes = CalendarEventChanges {
            timezone: Some("Asia/Tokyo".to_string()),
            ..CalendarEventChanges::default()
        };
        let preview = format_event_changes("work", &event, &changes);
        assert_eq!(preview, "no changes");
    }

    #[test]
    fn preview_shows_notes_and_location_content() {
        let event = base_event(0, 0);
        let changes = CalendarEventChanges {
            description: Some("bring slides".to_string()),
            location: Some("room 7".to_string()),
            ..CalendarEventChanges::default()
        };
        let preview = format_event_changes("work", &event, &changes);
        assert!(preview.contains("notes"), "preview labels the notes change");
        assert!(
            preview.contains("bring slides"),
            "preview shows the new notes"
        );
        assert!(preview.contains("room 7"), "preview shows the new location");
    }

    #[test]
    fn preview_truncates_long_notes() {
        let event = base_event(0, 0);
        let long = "x".repeat(200);
        let changes = CalendarEventChanges {
            description: Some(long.clone()),
            ..CalendarEventChanges::default()
        };
        let preview = format_event_changes("work", &event, &changes);
        assert!(preview.contains("..."));
        assert!(!preview.contains(&long), "long notes must be truncated");
    }

    #[test]
    fn preview_empty_changes() {
        let event = base_event(0, 0);
        let preview = format_event_changes("work", &event, &CalendarEventChanges::default());
        assert_eq!(preview, "no changes");
    }

    async fn seed_account(store: &CalendarStore, id: &str, name: &str) -> CalendarAccount {
        store
            .add_account(id, name, CalendarKind::Local)
            .await
            .expect("seed account")
    }

    #[tokio::test]
    async fn account_lifecycle() {
        let (store, path, handle) = make_store().await;

        let account = seed_account(&store, "a1", "Work").await;
        assert_eq!(account.name, "Work");
        assert_eq!(account.kind, CalendarKind::Local);
        assert!(account.read_allowed);
        assert!(!account.write_allowed, "writes default to denied");

        assert!(matches!(
            store.add_account("a2", "Work", CalendarKind::Local).await,
            Err(CalendarStoreError::DuplicateName(_))
        ));

        let listed = store.list_accounts().await.expect("list accounts");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "a1");

        let perm = store
            .set_permissions("a1", Some(false), Some(true))
            .await
            .expect("set permissions");
        assert!(!perm.read_allowed);
        assert!(perm.write_allowed);

        store.remove_account("a1").await.expect("remove account");
        assert!(matches!(
            store.get_account("a1").await,
            Err(CalendarStoreError::AccountNotFound(_))
        ));
        assert!(store.list_accounts().await.expect("list").is_empty());

        handle.abort();
        cleanup_path(&path);
    }

    #[tokio::test]
    async fn event_crud_and_window_filtering() {
        let (store, path, handle) = make_store().await;
        seed_account(&store, "a1", "Work").await;

        let input = sample_input(
            "Standup",
            "2026-08-03T10:00:00+09:00",
            "2026-08-03T10:30:00+09:00",
        );
        let event = store
            .create_event("a1", &input)
            .await
            .expect("create event");
        assert_eq!(event.title, "Standup");
        assert_eq!(event.start_at, 1_785_718_800_000);
        assert_eq!(event.timezone, "Asia/Tokyo");
        assert_eq!(event.attendees, vec!["alice@example.com"]);
        assert_eq!(event.status, "confirmed");

        let out_of_window = store
            .list_events("a1", Some(event.start_at + 3_600_000), None, false)
            .await
            .expect("list after window");
        assert!(out_of_window.is_empty());

        let in_window = store
            .list_events(
                "a1",
                Some(event.start_at - 1),
                Some(event.end_at + 1),
                false,
            )
            .await
            .expect("list in window");
        assert_eq!(in_window.len(), 1);

        let changes = CalendarEventChanges {
            title: Some("Daily".to_string()),
            end: Some("2026-08-03T11:00:00+09:00".to_string()),
            ..CalendarEventChanges::default()
        };
        let updated = store
            .update_event("a1", &event.id, &changes)
            .await
            .expect("update event");
        assert_eq!(updated.title, "Daily");
        assert_eq!(updated.end_at, 1_785_722_400_000);

        assert!(matches!(
            store
                .update_event(
                    "a1",
                    &event.id,
                    &CalendarEventChanges {
                        end: Some("2026-08-03T09:00:00+09:00".to_string()),
                        ..CalendarEventChanges::default()
                    }
                )
                .await,
            Err(CalendarStoreError::InvalidTimeRange)
        ));

        let cancelled = store
            .cancel_event("a1", &event.id)
            .await
            .expect("cancel event");
        assert_eq!(cancelled.id, event.id);
        assert_eq!(cancelled.status, "cancelled");
        let after_cancel = store
            .get_event("a1", &event.id)
            .await
            .expect("soft-cancelled event stays stored");
        assert_eq!(after_cancel.status, "cancelled");
        assert!(
            store
                .list_events("a1", None, None, false)
                .await
                .expect("list after cancel")
                .is_empty(),
            "plain listings exclude cancelled events"
        );
        assert_eq!(
            store
                .list_events("a1", None, None, true)
                .await
                .expect("list with cancelled")
                .len(),
            1,
            "include_cancelled still surfaces the event"
        );
        let revived = store
            .update_event(
                "a1",
                &event.id,
                &CalendarEventChanges {
                    status: Some("confirmed".to_string()),
                    ..CalendarEventChanges::default()
                },
            )
            .await
            .expect("cancellation is reversible via update_event");
        assert_eq!(revived.status, "confirmed");

        handle.abort();
        cleanup_path(&path);
    }

    #[tokio::test]
    async fn create_event_rejects_lifecycle_only_status() {
        let (store, path, handle) = make_store().await;
        seed_account(&store, "a1", "Work").await;

        let mut input = sample_input(
            "Doomed",
            "2026-08-03T10:00:00+09:00",
            "2026-08-03T11:00:00+09:00",
        );
        input.status = "cancelled".to_string();
        assert!(matches!(
            store.create_event("a1", &input).await,
            Err(CalendarStoreError::InvalidNewStatus(_))
        ));

        input.status = "tentative".to_string();
        let event = store
            .create_event("a1", &input)
            .await
            .expect("tentative is a valid creation status");
        assert_eq!(event.status, "tentative");

        handle.abort();
        cleanup_path(&path);
    }

    #[tokio::test]
    async fn overlapping_query_catches_events_spanning_the_window() {
        let (store, path, handle) = make_store().await;
        seed_account(&store, "a1", "Work").await;
        store
            .create_event(
                "a1",
                &sample_input(
                    "Crosser",
                    "2026-08-03T09:00:00+09:00",
                    "2026-08-03T11:00:00+09:00",
                ),
            )
            .await
            .expect("seed spanning event");

        let window_start = 1_785_718_800_000; // 10:00+09:00
        let window_end = window_start + 2 * 3_600_000; // 12:00+09:00
        assert!(
            store
                .list_events("a1", Some(window_start), Some(window_end), false)
                .await
                .expect("start_at-window listing")
                .is_empty(),
            "the start_at-based window excludes events starting before it"
        );
        let overlapping = store
            .list_events_overlapping("a1", window_start, window_end)
            .await
            .expect("overlap query");
        assert_eq!(overlapping.len(), 1);
        assert_eq!(overlapping[0].title, "Crosser");

        handle.abort();
        cleanup_path(&path);
    }

    #[tokio::test]
    async fn add_account_rejects_empty_names() {
        let (store, path, handle) = make_store().await;
        assert!(matches!(
            store.add_account("a1", "   ", CalendarKind::Local).await,
            Err(CalendarStoreError::EmptyAccountName)
        ));
        handle.abort();
        cleanup_path(&path);
    }

    #[tokio::test]
    async fn remove_account_cascades_events() {
        let (store, path, handle) = make_store().await;
        seed_account(&store, "a1", "Work").await;
        store
            .create_event(
                "a1",
                &sample_input(
                    "One",
                    "2026-08-03T10:00:00+09:00",
                    "2026-08-03T11:00:00+09:00",
                ),
            )
            .await
            .expect("seed event");
        store
            .create_event(
                "a1",
                &sample_input(
                    "Two",
                    "2026-08-04T10:00:00+09:00",
                    "2026-08-04T11:00:00+09:00",
                ),
            )
            .await
            .expect("seed event");

        store.remove_account("a1").await.expect("remove account");
        assert!(
            store
                .list_events("a1", None, None, false)
                .await
                .expect("list after removal")
                .is_empty(),
            "events must be deleted with the account (unlink is immediate)"
        );
        assert!(matches!(
            store.get_account("a1").await,
            Err(CalendarStoreError::AccountNotFound(_))
        ));

        handle.abort();
        cleanup_path(&path);
    }

    #[tokio::test]
    async fn permission_flags_gate_reads_and_writes() {
        let (store, path, handle) = make_store().await;
        let account = seed_account(&store, "a1", "Work").await;
        assert!(account.read_allowed);
        assert!(!account.write_allowed);

        store
            .require_readable("a1")
            .await
            .expect("read allowed by default");
        assert!(matches!(
            store.require_writable("a1").await,
            Err(CalendarStoreError::WriteDenied(_))
        ));

        store
            .set_permissions("a1", Some(false), Some(true))
            .await
            .expect("set permissions");
        assert!(matches!(
            store.require_readable("a1").await,
            Err(CalendarStoreError::ReadDenied(_))
        ));
        store
            .require_writable("a1")
            .await
            .expect("write allowed after grant");

        handle.abort();
        cleanup_path(&path);
    }

    #[tokio::test]
    async fn events_do_not_leak_across_accounts() {
        let (store, path, handle) = make_store().await;
        seed_account(&store, "a1", "Work").await;
        seed_account(&store, "a2", "Personal").await;
        let event = store
            .create_event(
                "a1",
                &sample_input(
                    "Secret",
                    "2026-08-03T10:00:00+09:00",
                    "2026-08-03T11:00:00+09:00",
                ),
            )
            .await
            .expect("create event");

        let other = store
            .list_events("a2", None, None, false)
            .await
            .expect("list other account");
        assert!(other.is_empty());
        assert!(matches!(
            store.get_event("a2", &event.id).await,
            Err(CalendarStoreError::EventNotFoundInAccount { .. })
        ));

        handle.abort();
        cleanup_path(&path);
    }
}
