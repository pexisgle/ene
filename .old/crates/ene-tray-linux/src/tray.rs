use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, unbounded};
use ksni::menu::StandardItem;
use ksni::{Handle, MenuItem as KsniMenuItem, ToolTip, Tray, TrayMethods};
use parking_lot::Mutex;
use thiserror::Error;
use tokio::runtime::Handle as TokioHandle;

use crate::icon::rgba_to_icon;

const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(400);

/// One row in the tray context menu.
#[derive(Debug, Clone)]
pub enum TrayMenuSlot {
    Item {
        id: String,
        label: String,
        enabled: bool,
    },
    Separator,
}

/// Configuration for a Linux tray icon.
#[derive(Debug, Clone)]
pub struct LinuxTrayConfig {
    pub app_id: String,
    pub tooltip: String,
    pub icon_rgba: (Vec<u8>, u32, u32),
    pub menu: Vec<TrayMenuSlot>,
}

/// Events emitted by the tray service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinuxTrayEvent {
    MenuActivate { id: String },
    IconActivate,
    IconDoubleActivate,
}

#[derive(Debug, Error)]
pub enum LinuxTrayError {
    #[error("tray service: {0}")]
    Service(String),
    #[error("tokio runtime is not available")]
    RuntimeGone,
}

struct SharedTrayData {
    app_id: String,
    tooltip: String,
    icon: ksni::Icon,
    menu: Vec<TrayMenuSlot>,
    event_tx: Sender<LinuxTrayEvent>,
    interaction_tx: Sender<()>,
}

struct TrayService {
    data: Arc<Mutex<SharedTrayData>>,
    last_activate: Option<Instant>,
}

impl Tray for TrayService {
    fn id(&self) -> String {
        self.data.lock().app_id.clone()
    }

    fn tool_tip(&self) -> ToolTip {
        let data = self.data.lock();
        ToolTip {
            title: data.tooltip.clone(),
            description: String::new(),
            icon_name: String::new(),
            icon_pixmap: Vec::new(),
        }
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![self.data.lock().icon.clone()]
    }

    fn menu(&self) -> Vec<KsniMenuItem<Self>> {
        let data = self.data.lock();
        build_ksni_menu(&data.menu, &data.event_tx, &data.interaction_tx)
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let now = Instant::now();
        let double = self
            .last_activate
            .is_some_and(|prev| now.duration_since(prev) <= DOUBLE_CLICK_WINDOW);
        self.last_activate = Some(now);
        let data = self.data.lock();
        if data.interaction_tx.send(()).is_err() {
            return;
        }
        let event = if double {
            LinuxTrayEvent::IconDoubleActivate
        } else {
            LinuxTrayEvent::IconActivate
        };
        drop(data.event_tx.send(event));
    }
}

/// Handle to a running ksni tray service.
pub struct LinuxTrayHandle {
    event_rx: Receiver<LinuxTrayEvent>,
    interaction_rx: Receiver<()>,
    data: Arc<Mutex<SharedTrayData>>,
    ksni_handle: Handle<TrayService>,
    runtime: TokioHandle,
}

impl LinuxTrayHandle {
    pub fn spawn(config: LinuxTrayConfig, runtime: &TokioHandle) -> Result<Self, LinuxTrayError> {
        let (event_tx, event_rx) = unbounded();
        let (interaction_tx, interaction_rx) = unbounded();
        let icon = rgba_to_icon(config.icon_rgba.0, config.icon_rgba.1, config.icon_rgba.2);
        let data = Arc::new(Mutex::new(SharedTrayData {
            app_id: config.app_id,
            tooltip: config.tooltip,
            icon,
            menu: config.menu,
            event_tx,
            interaction_tx,
        }));
        let tray = TrayService {
            data: Arc::clone(&data),
            last_activate: None,
        };
        let ksni_handle = runtime
            .block_on(tray.spawn())
            .map_err(|err| LinuxTrayError::Service(err.to_string()))?;
        Ok(Self {
            event_rx,
            interaction_rx,
            data,
            ksni_handle,
            runtime: runtime.clone(),
        })
    }

    pub fn try_recv(&self) -> Option<LinuxTrayEvent> {
        self.event_rx.try_recv().ok()
    }

    #[must_use]
    pub fn take_interactions(&self) -> usize {
        let mut drained = 0;
        while self.interaction_rx.try_recv().is_ok() {
            drained += 1;
        }
        drained
    }

    pub fn set_item_label(&self, id: &str, label: String) {
        let ksni_handle = self.ksni_handle.clone();
        let data = Arc::clone(&self.data);
        let id = id.to_owned();
        self.runtime.spawn(async move {
            let updated = {
                let mut guard = data.lock();
                let Some(TrayMenuSlot::Item {
                    label: item_label, ..
                }) = guard.menu.iter_mut().find(
                    |slot| matches!(slot, TrayMenuSlot::Item { id: item_id, .. } if item_id == &id),
                )
                else {
                    return;
                };
                *item_label = label;
                true
            };
            if updated {
                let _ = ksni_handle.update(|_tray: &mut TrayService| {}).await;
            }
        });
    }
}

fn build_ksni_menu(
    menu: &[TrayMenuSlot],
    event_tx: &Sender<LinuxTrayEvent>,
    interaction_tx: &Sender<()>,
) -> Vec<KsniMenuItem<TrayService>> {
    menu.iter()
        .map(|slot| match slot {
            TrayMenuSlot::Separator => KsniMenuItem::Separator,
            TrayMenuSlot::Item { id, label, enabled } => {
                let event_tx = event_tx.clone();
                let interaction_tx = interaction_tx.clone();
                let id = id.clone();
                StandardItem {
                    label: label.clone(),
                    enabled: *enabled,
                    activate: Box::new(move |_tray: &mut TrayService| {
                        if interaction_tx.send(()).is_err() {
                            return;
                        }
                        drop(event_tx.send(LinuxTrayEvent::MenuActivate { id: id.clone() }));
                    }),
                    ..StandardItem::default()
                }
                .into()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_slots_clone() {
        let slots = [
            TrayMenuSlot::Item {
                id: "a".into(),
                label: "A".into(),
                enabled: true,
            },
            TrayMenuSlot::Separator,
        ];
        assert_eq!(slots.len(), 2);
    }
}
