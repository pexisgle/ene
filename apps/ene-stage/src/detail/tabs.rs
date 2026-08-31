//! Per-tab Slint view models. Each tab fills heading / body / rows / drafts.

use crate::i18n;
use crate::monitor::{MonitorInfo, OverlayMonitorMode};
use crate::settings::DesktopSettings;
use crate::ui::DetailListRow;

use super::{
    CompanionDisplay, DetailTab, DetailUiState, approval_mode_label, caption_position_label,
    chat_setup_gap, chat_setup_status, core_lifetime_label, home_chat_next_step, home_status_cards,
    log_empty_copy, memory_kind_label, memory_scope_label, monitor_summary, observation_scope_text,
    overlay_monitor_mode_label, plugin_profile_label,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailActionSpec {
    pub id: &'static str,
    pub label: String,
    pub primary: bool,
}

#[derive(Debug, Clone, Default)]
pub struct DetailView {
    pub heading: String,
    pub body: String,
    pub rows: Vec<DetailListRow>,
    pub draft_a: String,
    pub draft_b: String,
    pub drafts_visible: bool,
    pub status: String,
    pub actions: Vec<DetailActionSpec>,
}

pub fn project_tab(
    state: &DetailUiState,
    local: &DesktopSettings,
    companions: &[CompanionDisplay],
    monitors: &[MonitorInfo],
) -> DetailView {
    match state.tab {
        DetailTab::Home => home(state),
        DetailTab::Companion => companion(state, companions),
        DetailTab::Conversation => conversation(state),
        DetailTab::Voice => voice(state, local),
        DetailTab::Memory => memory(state),
        DetailTab::Work => work(state),
        DetailTab::Connections => connections(state),
        DetailTab::System => system(state, local, monitors),
        DetailTab::Log => log(state),
    }
}

fn action(id: &'static str, label: String, primary: bool) -> DetailActionSpec {
    DetailActionSpec { id, label, primary }
}

fn home(state: &DetailUiState) -> DetailView {
    DetailView {
        heading: i18n::fl("detail-tab-home"),
        body: if state.health.is_empty() {
            home_chat_next_step(state)
        } else {
            state.health.clone()
        },
        rows: home_status_cards(state)
            .into_iter()
            .map(|(tab, card)| {
                let index = DetailTab::ALL
                    .iter()
                    .position(|candidate| *candidate == tab)
                    .unwrap_or(0);
                DetailListRow {
                    id: slint::SharedString::from(format!("tab:{index}")),
                    title: slint::SharedString::from(card.title),
                    subtitle: slint::SharedString::from(card.summary),
                }
            })
            .collect(),
        status: state.core_status.clone(),
        actions: vec![
            action("chat", i18n::fl("detail-open-chat"), true),
            action("reload", i18n::fl("detail-reload"), false),
        ],
        ..DetailView::default()
    }
}

fn companion(state: &DetailUiState, companions: &[CompanionDisplay]) -> DetailView {
    let rows = companions
        .iter()
        .map(|row| DetailListRow {
            id: slint::SharedString::from(format!("companion:{}", row.soul_id)),
            title: slint::SharedString::from(row.display_name.as_str()),
            subtitle: slint::SharedString::from(if row.active {
                i18n::fl("character-active-badge")
            } else if row.displayed {
                i18n::fl("character-displayed")
            } else {
                i18n::fl("character-not-displayed")
            }),
        })
        .collect();
    DetailView {
        heading: i18n::fl("detail-tab-companion"),
        body: state.soul.as_ref().map_or_else(
            || i18n::fl("home-next-companion"),
            |soul| soul.display_name.clone(),
        ),
        rows,
        draft_a: state.body_ref_draft.clone(),
        draft_b: String::new(),
        drafts_visible: true,
        status: state.core_status.clone(),
        actions: vec![
            action("apply-body", i18n::fl("detail-apply"), true),
            action("import-character", i18n::fl("character-import"), false),
            action("reload-characters", i18n::fl("detail-reload"), false),
        ],
    }
}

fn conversation(state: &DetailUiState) -> DetailView {
    let setup =
        chat_setup_gap(state).map_or_else(|| i18n::fl("home-chat-ready"), chat_setup_status);
    DetailView {
        heading: i18n::fl("detail-tab-conversation"),
        body: format!("{setup}\n{}", observation_scope_text(state)),
        draft_a: state.chat_plugin.clone(),
        draft_b: state.chat_model.clone(),
        drafts_visible: true,
        status: state.core_status.clone(),
        actions: vec![
            action("apply-ai", i18n::fl("detail-apply"), true),
            action("reload", i18n::fl("detail-reload"), false),
        ],
        rows: state
            .provider_models
            .iter()
            .take(24)
            .map(|model| DetailListRow {
                id: slint::SharedString::from(format!("model:{model}")),
                title: slint::SharedString::from(model.as_str()),
                subtitle: slint::SharedString::from(state.chat_plugin.as_str()),
            })
            .collect(),
    }
}

fn voice(state: &DetailUiState, local: &DesktopSettings) -> DetailView {
    DetailView {
        heading: i18n::fl("detail-tab-voice"),
        body: format!(
            "TTS {} / {}\nSTT {} / {}\nmic {}\ncaption {}",
            state.tts_plugin,
            state.tts_model,
            state.stt_plugin,
            state.stt_model,
            local.mic_device,
            caption_position_label(&local.caption_position)
        ),
        draft_a: state.tts_plugin.clone(),
        draft_b: state.stt_plugin.clone(),
        drafts_visible: true,
        status: state.core_status.clone(),
        rows: Vec::new(),
        actions: vec![
            action("apply-voice", i18n::fl("detail-apply"), true),
            action("reload", i18n::fl("detail-reload"), false),
        ],
    }
}

fn memory(state: &DetailUiState) -> DetailView {
    let rows = state
        .memories
        .iter()
        .map(|memory| DetailListRow {
            id: slint::SharedString::from(memory.id.as_str()),
            title: slint::SharedString::from(memory.title.as_str()),
            subtitle: slint::SharedString::from(format!(
                "{} / {}",
                memory_kind_label(&memory.kind),
                memory_scope_label(&memory.scope)
            )),
        })
        .collect();
    DetailView {
        heading: i18n::fl("detail-tab-memory"),
        body: format!("{} pending", state.pending_memories.len()),
        rows,
        status: state.core_status.clone(),
        actions: vec![action("refresh-memory", i18n::fl("memory-refresh"), true)],
        ..DetailView::default()
    }
}

fn work(state: &DetailUiState) -> DetailView {
    let rows = state
        .jobs
        .iter()
        .map(|job| DetailListRow {
            id: slint::SharedString::from(format!("job:{}", job.id)),
            title: slint::SharedString::from(job.title.as_str()),
            subtitle: slint::SharedString::from(job.status.as_str()),
        })
        .collect();
    DetailView {
        heading: i18n::fl("detail-tab-work"),
        body: format!("{} schedules", state.schedules.len()),
        rows,
        draft_a: state.new_job_title.clone(),
        draft_b: state.new_job_goal.clone(),
        drafts_visible: true,
        status: state.core_status.clone(),
        actions: vec![
            action("create-job", i18n::fl("jobs-create"), true),
            action("reload-jobs", i18n::fl("detail-reload"), false),
        ],
    }
}

fn connections(state: &DetailUiState) -> DetailView {
    let rows = state
        .mcp_servers
        .iter()
        .map(|server| DetailListRow {
            id: slint::SharedString::from(server.id.as_str()),
            title: slint::SharedString::from(server.id.as_str()),
            subtitle: slint::SharedString::from(server.command.clone().unwrap_or_default()),
        })
        .collect();
    DetailView {
        heading: i18n::fl("detail-tab-connections"),
        body: state.connections_status.clone(),
        rows,
        status: state.core_status.clone(),
        actions: vec![action("reload-mcp", i18n::fl("plugins-mcp-reload"), true)],
        ..DetailView::default()
    }
}

fn system(state: &DetailUiState, local: &DesktopSettings, monitors: &[MonitorInfo]) -> DetailView {
    let mode = OverlayMonitorMode::from_setting(&local.overlay_monitor_mode);
    let mut body = format!(
        "theme {} / language {} / approval {}\noverlay {}\ncore {} / plugins {}",
        super::theme_label(&local.theme),
        super::language_value_label(&local.language),
        approval_mode_label(&state.approval_mode),
        overlay_monitor_mode_label(mode),
        core_lifetime_label(&local.core_lifetime),
        plugin_profile_label(&state.plugins_profile)
    );
    if !state.overlay_monitor_notice.is_empty() {
        body.push('\n');
        body.push_str(&state.overlay_monitor_notice);
    }
    let rows = monitors
        .iter()
        .map(|monitor| DetailListRow {
            id: slint::SharedString::from(format!("monitor:{}", monitor.id)),
            title: slint::SharedString::from(monitor_summary(monitor)),
            subtitle: slint::SharedString::from(if monitor.is_primary {
                overlay_monitor_mode_label(OverlayMonitorMode::Primary)
            } else {
                overlay_monitor_mode_label(OverlayMonitorMode::Selected)
            }),
        })
        .collect();
    DetailView {
        heading: i18n::fl("detail-tab-system"),
        body,
        rows,
        draft_a: local.theme.clone(),
        draft_b: local.language.clone(),
        drafts_visible: true,
        status: state.core_status.clone(),
        actions: vec![
            action("apply-system", i18n::fl("detail-apply"), true),
            action("reload", i18n::fl("detail-reload"), false),
        ],
    }
}

fn log(state: &DetailUiState) -> DetailView {
    let rows = state
        .log
        .iter()
        .rev()
        .take(40)
        .map(|entry| DetailListRow {
            id: slint::SharedString::from(entry.text.as_str()),
            title: slint::SharedString::from(super::log_kind_label(entry.kind)),
            subtitle: slint::SharedString::from(entry.text.as_str()),
        })
        .collect();
    DetailView {
        heading: i18n::fl("detail-tab-log"),
        body: log_empty_copy(state.log.len()).unwrap_or_default(),
        rows,
        status: state.core_status.clone(),
        actions: vec![action("reload", i18n::fl("detail-reload"), false)],
        ..DetailView::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::DesktopSettings;

    fn view_for(tab: DetailTab) -> DetailView {
        let state = DetailUiState {
            tab,
            ..DetailUiState::default()
        };
        project_tab(&state, &DesktopSettings::default(), &[], &[])
    }

    fn action_ids(view: &DetailView) -> Vec<&str> {
        view.actions.iter().map(|action| action.id).collect()
    }

    #[test]
    fn work_tab_creates_a_job_instead_of_applying_ai() {
        let view = view_for(DetailTab::Work);
        let ids = action_ids(&view);
        assert!(ids.contains(&"create-job"));
        assert!(!ids.contains(&"apply"));
        assert!(!ids.contains(&"apply-ai"));
    }

    #[test]
    fn conversation_tab_applies_ai() {
        let view = view_for(DetailTab::Conversation);
        assert!(action_ids(&view).contains(&"apply-ai"));
    }

    #[test]
    fn companion_tab_applies_body_not_ai() {
        let view = view_for(DetailTab::Companion);
        let ids = action_ids(&view);
        assert!(ids.contains(&"apply-body"));
        assert!(ids.contains(&"import-character"));
        assert!(!ids.contains(&"apply-ai"));
    }

    #[test]
    fn memory_and_connections_do_not_share_apply() {
        let memory = view_for(DetailTab::Memory);
        let connections = view_for(DetailTab::Connections);
        assert_eq!(action_ids(&memory), vec!["refresh-memory"]);
        assert_eq!(action_ids(&connections), vec!["reload-mcp"]);
    }
}
