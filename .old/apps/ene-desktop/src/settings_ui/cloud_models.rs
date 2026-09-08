//! Refresh cloud model combos through core (`POST /api/v1/providers/models`).

use std::collections::BTreeMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use tokio::sync::oneshot;

use super::provider_form::CloudModelComboState;
use crate::core_session::CoreSession;

const DEBOUNCE: Duration = Duration::from_millis(400);

#[derive(Debug, Default)]
pub struct CloudModelListUi {
    slots: BTreeMap<String, Slot>,
}

#[derive(Debug)]
struct Slot {
    want: String,
    want_since: Instant,
    active: String,
    receiver: Option<oneshot::Receiver<Result<Vec<String>, String>>>,
    ids: Option<Vec<String>>,
    error: Option<String>,
}

impl CloudModelListUi {
    pub fn poll(&mut self) {
        for slot in self.slots.values_mut() {
            let Some(receiver) = slot.receiver.as_mut() else {
                continue;
            };
            match receiver.try_recv() {
                Ok(Ok(ids)) => {
                    slot.ids = Some(ids);
                    slot.error = None;
                    slot.receiver = None;
                }
                Ok(Err(error)) => {
                    slot.error = Some(error);
                    slot.receiver = None;
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
                Err(oneshot::error::TryRecvError::Closed) => {
                    slot.error = Some("fetch cancelled".to_owned());
                    slot.receiver = None;
                }
            }
        }
    }

    pub fn sync(
        &mut self,
        session: &CoreSession,
        task: &str,
        plugin: &str,
        base_url: &str,
        api_key: &str,
    ) {
        if !lists_remote_models(plugin) {
            return;
        }
        let identity = fetch_identity(plugin, base_url, api_key);
        let slot = self.slots.entry(task.to_owned()).or_insert_with(|| Slot {
            want: String::new(),
            want_since: Instant::now(),
            active: String::new(),
            receiver: None,
            ids: None,
            error: None,
        });
        if slot.want != identity {
            slot.want.clone_from(&identity);
            slot.want_since = Instant::now();
        }
        if slot.active == identity {
            return;
        }
        if slot.receiver.is_some() {
            return;
        }
        let wait = if slot.active.is_empty() {
            Duration::ZERO
        } else {
            DEBOUNCE
        };
        if slot.want_since.elapsed() < wait {
            return;
        }
        slot.receiver = Some(session.fetch_provider_models(
            plugin.to_owned(),
            task.to_owned(),
            base_url.to_owned(),
            api_key.to_owned(),
        ));
        slot.active.clone_from(&identity);
        slot.error = None;
    }

    #[must_use]
    pub fn combo_state(&self, task: &str) -> CloudModelComboState<'_> {
        let slot = self.slots.get(task);
        CloudModelComboState {
            live: slot.and_then(|slot| slot.ids.as_deref()).unwrap_or(&[]),
            loading: slot.is_some_and(|slot| slot.receiver.is_some()),
            error: slot.and_then(|slot| slot.error.as_deref()),
        }
    }
}

#[must_use]
pub fn lists_remote_models(plugin: &str) -> bool {
    matches!(plugin_kind(plugin), "openai_compat" | "anthropic")
}

fn plugin_kind(plugin: &str) -> &str {
    plugin.strip_prefix("provider.").unwrap_or(plugin)
}

fn fetch_identity(plugin: &str, base_url: &str, api_key: &str) -> String {
    let mut hasher = DefaultHasher::new();
    api_key.hash(&mut hasher);
    format!(
        "{}\n{}\n{:x}",
        plugin_kind(plugin),
        base_url.trim(),
        hasher.finish()
    )
}

#[cfg(test)]
mod tests {
    use super::{lists_remote_models, plugin_kind};

    #[test]
    fn openai_and_anthropic_list_remotely() {
        assert!(lists_remote_models("provider.openai_compat"));
        assert!(lists_remote_models("provider.anthropic"));
        assert!(!lists_remote_models("provider.gguf"));
        assert!(!lists_remote_models("provider.elevenlabs"));
        assert_eq!(plugin_kind("provider.anthropic"), "anthropic");
    }
}
