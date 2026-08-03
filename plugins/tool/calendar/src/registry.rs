use crate::store::{
    CalendarEvent, CalendarEventChanges, CalendarEventInput, CalendarKind, CalendarStore,
    CalendarStoreError, FreeSlot, find_free_slots,
};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

/// Backend that owns the events of accounts of one [`CalendarKind`].
///
/// This is the seam external providers (Google Calendar, `CalDAV`, ...) will
/// implement once the connector framework (credential handling for external
/// services) lands; the built-in local provider stores events in the
/// plugin's own DB tables. Every method takes the shared [`CalendarStore`] so providers stay
/// stateless — they operate on their account's events, never on account
/// metadata or permissions, which the store owns.
#[async_trait]
pub trait CalendarProvider: Send + Sync {
    /// The kind of account this provider serves.
    fn kind(&self) -> CalendarKind;

    /// Fetches an account's events within the `[start_ms, end_ms)` window
    /// (either bound optional); cancelled events are excluded unless
    /// `include_cancelled` is set.
    async fn list_events(
        &self,
        store: &CalendarStore,
        account_id: &str,
        start_ms: Option<i64>,
        end_ms: Option<i64>,
        include_cancelled: bool,
    ) -> Result<Vec<CalendarEvent>, CalendarStoreError>;

    /// Creates an event in the account and returns it.
    async fn create_event(
        &self,
        store: &CalendarStore,
        account_id: &str,
        input: &CalendarEventInput,
    ) -> Result<CalendarEvent, CalendarStoreError>;

    /// Applies partial changes to an event and returns the updated event.
    async fn update_event(
        &self,
        store: &CalendarStore,
        account_id: &str,
        event_id: &str,
        changes: &CalendarEventChanges,
    ) -> Result<CalendarEvent, CalendarStoreError>;

    /// Cancels an event, returning the removed event.
    async fn cancel_event(
        &self,
        store: &CalendarStore,
        account_id: &str,
        event_id: &str,
    ) -> Result<CalendarEvent, CalendarStoreError>;

    /// Finds free slots of `duration_min` minutes within
    /// `[start_ms, end_ms)`. The default implementation computes slots from
    /// [`Self::list_events`]; providers with native busy-time APIs may
    /// override it.
    async fn find_free_slots(
        &self,
        store: &CalendarStore,
        account_id: &str,
        start_ms: i64,
        end_ms: i64,
        duration_min: u64,
    ) -> Result<Vec<FreeSlot>, CalendarStoreError> {
        let events = self
            .list_events(store, account_id, Some(start_ms), Some(end_ms), false)
            .await?;
        Ok(find_free_slots(&events, start_ms, end_ms, duration_min))
    }
}

/// Looks up the provider for an account kind.
#[derive(Clone, Default)]
pub struct ProviderRegistry {
    providers: Arc<HashMap<String, Arc<dyn CalendarProvider>>>,
}

impl ProviderRegistry {
    /// Registers a provider; later registrations for the same kind replace
    /// earlier ones.
    pub fn register(&mut self, provider: Arc<dyn CalendarProvider>) {
        let mut providers = HashMap::clone(&self.providers);
        providers.insert(provider.kind().as_str().to_string(), provider);
        self.providers = Arc::new(providers);
    }

    /// Resolves the provider for an account kind.
    pub fn resolve(
        &self,
        kind: CalendarKind,
    ) -> Result<Arc<dyn CalendarProvider>, CalendarStoreError> {
        self.providers
            .get(kind.as_str())
            .cloned()
            .ok_or_else(|| CalendarStoreError::UnknownProvider(kind.as_str().to_string()))
    }
}

/// Default built-in provider: events live in the plugin's `calendar_events`
/// table, owned by the host-service `db`.
#[derive(Clone, Default)]
pub struct LocalCalendarProvider;

impl LocalCalendarProvider {
    /// Creates a local provider instance.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl CalendarProvider for LocalCalendarProvider {
    fn kind(&self) -> CalendarKind {
        CalendarKind::Local
    }

    async fn list_events(
        &self,
        store: &CalendarStore,
        account_id: &str,
        start_ms: Option<i64>,
        end_ms: Option<i64>,
        include_cancelled: bool,
    ) -> Result<Vec<CalendarEvent>, CalendarStoreError> {
        store
            .list_events(account_id, start_ms, end_ms, include_cancelled)
            .await
    }

    async fn create_event(
        &self,
        store: &CalendarStore,
        account_id: &str,
        input: &CalendarEventInput,
    ) -> Result<CalendarEvent, CalendarStoreError> {
        store.create_event(account_id, input).await
    }

    async fn update_event(
        &self,
        store: &CalendarStore,
        account_id: &str,
        event_id: &str,
        changes: &CalendarEventChanges,
    ) -> Result<CalendarEvent, CalendarStoreError> {
        store.update_event(account_id, event_id, changes).await
    }

    async fn cancel_event(
        &self,
        store: &CalendarStore,
        account_id: &str,
        event_id: &str,
    ) -> Result<CalendarEvent, CalendarStoreError> {
        store.cancel_event(account_id, event_id).await
    }
}
