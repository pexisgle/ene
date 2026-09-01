//! Slint projection of [`super::DetailUiState`].

use crate::monitor::MonitorInfo;
use crate::settings::DesktopSettings;

use super::{CompanionDisplay, DetailTab, DetailUiState, DisplayAction, onboarding_visible};

pub use super::tabs::{DetailView, project_tab};

pub fn project(
    state: &DetailUiState,
    local: &DesktopSettings,
    companions: &[CompanionDisplay],
    monitors: &[MonitorInfo],
) -> DetailView {
    let mut view = project_tab(state, local, companions, monitors);
    if !state.core_status.is_empty() {
        if view.status.is_empty() {
            view.status.clone_from(&state.core_status);
        } else {
            view.status = format!("{}\n{}", view.status, state.core_status);
        }
    }
    if onboarding_visible(state, local) {
        view.body = format!(
            "{}\n\n{}\n{}",
            view.body,
            crate::i18n::fl("onboarding-title"),
            crate::i18n::fl("onboarding-body")
        );
    }
    view
}

pub fn handle_select_tab(state: &mut DetailUiState, index: i32) {
    if let Some(tab) = DetailTab::ALL.get(index as usize).copied() {
        state.select_tab(tab);
    }
}

pub fn handle_primary(
    state: &mut DetailUiState,
    local: &mut DesktopSettings,
    action: &str,
) -> Option<DetailPrimary> {
    match action {
        "apply-ai" => Some(DetailPrimary::ApplyAi),
        "apply-voice" => Some(DetailPrimary::ApplyVoice),
        "apply-system" => Some(DetailPrimary::ApplySystem),
        "apply-body" => Some(DetailPrimary::ApplyBody),
        "create-job" => Some(DetailPrimary::CreateJob),
        "refresh-memory" => Some(DetailPrimary::RefreshMemory),
        "reload-mcp" => Some(DetailPrimary::ReloadMcp),
        "reload-jobs" => Some(DetailPrimary::ReloadJobs),
        "reload-characters" => Some(DetailPrimary::ReloadCharacters),
        "import-character" => Some(DetailPrimary::ImportCharacter),
        "reload" => Some(DetailPrimary::Reload),
        "chat" => {
            state.request_chat_open = true;
            None
        }
        "open-spotlight" => {
            state.open_spotlight = true;
            None
        }
        "dismiss-onboarding" => {
            local.onboarding_dismissed = true;
            state.save_local_pending = true;
            None
        }
        _ => None,
    }
}

pub fn handle_row(
    state: &mut DetailUiState,
    companions: &[CompanionDisplay],
    id: &str,
) -> Option<DisplayAction> {
    if let Some(tab_id) = id.strip_prefix("tab:")
        && let Ok(index) = tab_id.parse::<usize>()
        && let Some(tab) = DetailTab::ALL.get(index).copied()
    {
        state.select_tab(tab);
        return None;
    }
    if let Some(soul) = id.strip_prefix("companion:") {
        let displayed = companions
            .iter()
            .find(|row| row.soul_id == soul)
            .is_some_and(|row| row.displayed && !row.temporarily_hidden);
        return Some(if displayed {
            DisplayAction::TemporarilyHide(soul.to_owned())
        } else {
            DisplayAction::Show(soul.to_owned())
        });
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailPrimary {
    ApplyAi,
    ApplyVoice,
    ApplySystem,
    ApplyBody,
    CreateJob,
    RefreshMemory,
    ReloadMcp,
    ReloadJobs,
    ReloadCharacters,
    ImportCharacter,
    Reload,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detail::DetailUiState;

    #[test]
    fn select_tab_maps_index() {
        let mut state = DetailUiState::default();
        handle_select_tab(&mut state, 2);
        assert_eq!(state.tab, DetailTab::Conversation);
        handle_select_tab(&mut state, 99);
        assert_eq!(state.tab, DetailTab::Conversation);
    }

    #[test]
    fn home_row_opens_tab() {
        let mut state = DetailUiState::default();
        let action = handle_row(&mut state, &[], "tab:1");
        assert!(action.is_none());
        assert_eq!(state.tab, DetailTab::Companion);
    }

    #[test]
    fn companion_row_toggles_display() {
        let mut state = DetailUiState::default();
        let companions = [crate::detail::CompanionDisplay {
            soul_id: "soul-a".to_owned(),
            display_name: "A".to_owned(),
            body_id: None,
            package_id: None,
            avatar_path: None,
            has_avatar: false,
            displayed: true,
            temporarily_hidden: false,
            active: true,
            order: Some(0),
        }];
        let action = handle_row(&mut state, &companions, "companion:soul-a");
        assert_eq!(
            action,
            Some(DisplayAction::TemporarilyHide("soul-a".to_owned()))
        );
    }

    #[test]
    fn work_create_job_is_not_apply_ai() {
        let mut state = DetailUiState::default();
        let mut local = crate::settings::DesktopSettings::default();
        assert_eq!(
            handle_primary(&mut state, &mut local, "create-job"),
            Some(DetailPrimary::CreateJob)
        );
        assert_eq!(
            handle_primary(&mut state, &mut local, "apply-ai"),
            Some(DetailPrimary::ApplyAi)
        );
        assert_eq!(handle_primary(&mut state, &mut local, "apply"), None);
    }

    #[test]
    fn companion_and_memory_actions_map_to_tab_commands() {
        let mut state = DetailUiState::default();
        let mut local = crate::settings::DesktopSettings::default();
        assert_eq!(
            handle_primary(&mut state, &mut local, "apply-body"),
            Some(DetailPrimary::ApplyBody)
        );
        assert_eq!(
            handle_primary(&mut state, &mut local, "import-character"),
            Some(DetailPrimary::ImportCharacter)
        );
        assert_eq!(
            handle_primary(&mut state, &mut local, "refresh-memory"),
            Some(DetailPrimary::RefreshMemory)
        );
        assert_eq!(
            handle_primary(&mut state, &mut local, "reload-mcp"),
            Some(DetailPrimary::ReloadMcp)
        );
    }

    #[test]
    fn open_spotlight_sets_the_detail_flag() {
        let mut state = DetailUiState::default();
        let mut local = crate::settings::DesktopSettings::default();
        assert!(!state.open_spotlight);
        assert_eq!(
            handle_primary(&mut state, &mut local, "open-spotlight"),
            None
        );
        assert!(state.open_spotlight);
    }
}
