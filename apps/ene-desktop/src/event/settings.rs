//! Each variant corresponds to one of the buttons or hotkeys handled
//! by the settings UI. They are written by UI input systems and
//! consumed by the `apply_settings_action` system.
use bevy_ecs::prelude::*;

use crate::settings_ui::PageKind;

#[derive(Message, Debug, Clone, Copy)]
pub struct OpenSettings {
    pub page: Option<PageKind>,
}
