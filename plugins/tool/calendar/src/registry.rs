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

    async fn cancel_event(
        &self,
        store: &CalendarStore,
        account_id: &str,
        event_id: &str,
    ) -> Result<CalendarEvent, CalendarStoreError>;

    /// Finds free slots of `duration_min` minutes within
    /// `[start_ms, end_ms)`. The default implementation derives busy
    /// intervals from [`Self::list_events`], which bounds events by their
    /// `start_at`; providers whose listings use that window semantics must
    /// override this to query by interval overlap, otherwise an event
    /// spanning the window edge is reported as free.
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

    async fn find_free_slots(
        &self,
        store: &CalendarStore,
        account_id: &str,
        start_ms: i64,
        end_ms: i64,
        duration_min: u64,
    ) -> Result<Vec<FreeSlot>, CalendarStoreError> {
        let events = store
            .list_events_overlapping(account_id, start_ms, end_ms)
            .await?;
        Ok(find_free_slots(&events, start_ms, end_ms, duration_min))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::CalendarEventInput;
    use crate::test_db::make_store;

    fn sample_input(title: &str, start: &str, end: &str) -> CalendarEventInput {
        CalendarEventInput {
            title: title.to_string(),
            description: String::new(),
            location: String::new(),
            start: start.to_string(),
            end: end.to_string(),
            timezone: "Asia/Tokyo".to_string(),
            attendees: Vec::new(),
            status: String::new(),
        }
    }

    #[tokio::test]
    async fn free_slots_block_events_spanning_the_search_window() {
        // A 9:00–11:00 meeting must block the 10:00–12:00 search window,
        // not just events that start inside it.
        let (store, path, handle) = make_store().await;
        store
            .add_account("a1", "Work", CalendarKind::Local)
            .await
            .expect("seed account");
        store
            .create_event(
                "a1",
                &sample_input(
                    "Morning block",
                    "2026-08-03T09:00:00+09:00",
                    "2026-08-03T11:00:00+09:00",
                ),
            )
            .await
            .expect("seed spanning event");

        let provider = LocalCalendarProvider::new();
        let window_start = 1_785_718_800_000; // 10:00+09:00
        let window_end = window_start + 2 * 3_600_000; // 12:00+09:00
        let slots = provider
            .find_free_slots(&store, "a1", window_start, window_end, 60)
            .await
            .expect("find free slots");
        assert_eq!(
            slots.len(),
            1,
            "only the 11:00–12:00 hour is free; the 10:00–11:00 hour is busy"
        );
        assert_eq!(slots[0].start_at, window_start + 3_600_000);
        assert_eq!(slots[0].end_at, window_end);

        handle.abort();
        ene_plugin_proto::transport::cleanup_path(&path);
    }
}
