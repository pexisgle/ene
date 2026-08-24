//! Alt+Space command palette.

use egui::{Align, Align2, Id, Key, ScrollArea, TextEdit};

use crate::detail::DetailTab;
use crate::i18n;
use crate::shell::ShellCommand;
use crate::surface::SurfaceUiState;

const SEARCH_ID: &str = "spotlight-search";
const LIST_MAX_HEIGHT: f32 = 320.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpotlightAction {
    Command(ShellCommand),
    Close,
}

impl SpotlightAction {
    /// Every palette action leaves the destination in front; the palette must
    /// not stay open behind it.
    #[must_use]
    pub const fn dismisses_palette(self) -> bool {
        match self {
            Self::Command(_) | Self::Close => true,
        }
    }
}

/// One searchable command. The registry is the single source of truth for the
/// palette, so new actions belong here rather than in `show`.
#[derive(Debug, Clone)]
pub struct SpotlightEntry {
    pub action: SpotlightAction,
    pub label: String,
    pub keywords: &'static [&'static str],
    /// Reserved for per-action shortcut hints; no binding exists yet.
    #[expect(
        dead_code,
        reason = "palette UI gains a shortcut column once bindings exist"
    )]
    pub shortcut: Option<&'static str>,
}

impl SpotlightEntry {
    fn command(action: ShellCommand, key: &str, keywords: &'static [&'static str]) -> Self {
        Self {
            action: SpotlightAction::Command(action),
            label: i18n::fl(key),
            keywords,
            shortcut: None,
        }
    }

    #[must_use]
    fn close() -> Self {
        Self {
            action: SpotlightAction::Close,
            label: i18n::fl("spotlight-close"),
            keywords: &[],
            shortcut: None,
        }
    }

    /// Lower is a better match. Mirrors `DetailTab::search_rank` so both
    /// search boxes behave identically.
    fn rank(&self, query: &str) -> Option<u8> {
        if query.is_empty() {
            return Some(0);
        }
        let q = query.to_ascii_lowercase();
        let label = self.label.to_lowercase();
        if label.starts_with(&q) {
            return Some(1);
        }
        if label.contains(&q) {
            return Some(2);
        }
        self.keywords
            .iter()
            .any(|word| word.starts_with(&q) || q.contains(word))
            .then_some(3)
    }
}

/// All actions the palette can run, in display order.
#[must_use]
pub fn palette_entries() -> Vec<SpotlightEntry> {
    let mut entries: Vec<_> = DetailTab::ALL
        .into_iter()
        .map(|tab| SpotlightEntry {
            action: SpotlightAction::Command(ShellCommand::OpenDetail(tab)),
            label: i18n::format("spotlight-open-detail", &[("tab", tab.label().as_str())]),
            keywords: tab.keywords(),
            shortcut: None,
        })
        .collect();
    entries.push(SpotlightEntry::command(
        ShellCommand::ToggleMic,
        "spotlight-toggle-mic",
        &["mic", "microphone", "voice"],
    ));
    entries.push(SpotlightEntry::command(
        ShellCommand::OpenChat,
        "spotlight-action-open-chat",
        &["chat", "conversation"],
    ));
    entries.push(SpotlightEntry::command(
        ShellCommand::Quit,
        "spotlight-quit",
        &["exit", "shutdown"],
    ));
    entries.push(SpotlightEntry::close());
    entries
}

/// Same ordering rule as `DetailTab::search_rank`: label prefix beats label
/// substring beats keyword hit.
#[must_use]
pub fn filter_entries<'a>(query: &str, entries: &'a [SpotlightEntry]) -> Vec<&'a SpotlightEntry> {
    let mut matches: Vec<(u8, usize, &SpotlightEntry)> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| entry.rank(query).map(|rank| (rank, index, entry)))
        .collect();
    matches.sort_by_key(|(rank, index, _)| (*rank, *index));
    matches.into_iter().map(|(_, _, entry)| entry).collect()
}

pub fn show(ctx: &egui::Context, state: &mut SurfaceUiState) -> Option<SpotlightAction> {
    let query = &mut state.spotlight_query;
    let selected = &mut state.spotlight_selected;
    let mut confirmed = false;
    let mut action = None;

    let search_id = Id::new(SEARCH_ID);
    if !ctx.memory(|mem| mem.has_focus(search_id)) {
        ctx.memory_mut(|mem| mem.request_focus(search_id));
    }

    egui::Window::new(i18n::fl("spotlight-title"))
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.add(
                TextEdit::singleline(query)
                    .id(Id::new(SEARCH_ID))
                    .hint_text(i18n::fl("spotlight-placeholder"))
                    .desired_width(f32::INFINITY),
            );

            let entries = palette_entries();
            let filtered = filter_entries(query, &entries);
            if filtered.is_empty() {
                ui.label(i18n::fl("spotlight-no-match"));
                return;
            }

            let move_up = ui.input(|i| i.key_pressed(Key::ArrowUp));
            let move_down = ui.input(|i| i.key_pressed(Key::ArrowDown));
            let confirm = ui.input(|i| i.key_pressed(Key::Enter));
            let cancel = ui.input(|i| i.key_pressed(Key::Escape));
            if cancel {
                action = Some(SpotlightAction::Close);
                return;
            }
            *selected = (*selected).min(filtered.len().saturating_sub(1));
            if move_down && *selected + 1 < filtered.len() {
                *selected += 1;
            } else if move_up && *selected > 0 {
                *selected -= 1;
            }
            if confirm {
                confirmed = true;
            }

            ScrollArea::vertical()
                .id_salt("spotlight-results")
                .max_height(LIST_MAX_HEIGHT)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (index, entry) in filtered.iter().enumerate() {
                        let is_selected = index == *selected;
                        let response = ui.selectable_label(is_selected, &entry.label);
                        if is_selected {
                            response.scroll_to_me(Some(Align::Center));
                        }
                        if response.clicked() || response.hovered() {
                            *selected = index;
                        }
                        if response.clicked() {
                            confirmed = true;
                        }
                    }
                });

            if confirmed {
                action = Some(filtered[*selected].action);
            }
        });
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find_action(entries: &[&SpotlightEntry], needle: &str) -> SpotlightAction {
        entries
            .iter()
            .find(|entry| entry.label.contains(needle))
            .unwrap_or_else(|| panic!("missing entry containing {needle}"))
            .action
    }

    #[test]
    fn every_quick_command_dismisses_the_palette() {
        for entry in palette_entries() {
            assert!(entry.action.dismisses_palette());
        }
    }

    #[test]
    fn empty_query_returns_all_entries() {
        let entries = palette_entries();
        let filtered = filter_entries("", &entries);
        assert_eq!(filtered.len(), entries.len());
    }

    #[test]
    fn query_filters_case_insensitively() {
        let entries = palette_entries();
        let lower = filter_entries("mic", &entries);
        let upper = filter_entries("MIC", &entries);
        assert_eq!(lower.len(), upper.len());
        assert!(!lower.is_empty());
    }

    #[test]
    fn japanese_label_substring_match() {
        crate::i18n::select_language("ja");
        let entries = palette_entries();
        let ja_matches = filter_entries("マイク", &entries);
        assert!(
            ja_matches
                .iter()
                .any(|entry| entry.action == SpotlightAction::Command(ShellCommand::ToggleMic))
        );
    }

    #[test]
    fn keyword_match_finds_entry() {
        let entries = palette_entries();
        let mic_hits = filter_entries("microphone", &entries);
        assert!(!mic_hits.is_empty());
        assert!(
            mic_hits
                .iter()
                .any(|entry| entry.action == SpotlightAction::Command(ShellCommand::ToggleMic))
        );
        let chat_hits = filter_entries("chat", &entries);
        assert!(
            chat_hits
                .iter()
                .any(|entry| entry.action == SpotlightAction::Command(ShellCommand::OpenChat))
        );
    }

    #[test]
    fn no_match_returns_empty() {
        let entries = palette_entries();
        assert!(filter_entries("zzzznope", &entries).is_empty());
    }

    #[test]
    fn rank_order_prefix_before_contains_before_keyword() {
        // Explicit labels keep the ranking assertion independent of the test
        // machine's locale.
        let entries = vec![
            SpotlightEntry {
                action: SpotlightAction::Command(ShellCommand::Quit),
                label: "Quit application".to_owned(),
                keywords: &[],
                shortcut: None,
            },
            SpotlightEntry {
                action: SpotlightAction::Close,
                label: "Quit later".to_owned(),
                keywords: &[],
                shortcut: None,
            },
            SpotlightEntry {
                action: SpotlightAction::Command(ShellCommand::OpenChat),
                label: "Unrelated".to_owned(),
                keywords: &["quit"],
                shortcut: None,
            },
        ];
        let ranked = filter_entries("quit", &entries);
        assert_eq!(ranked.len(), 3, "all three entries must match");
        assert_eq!(
            find_action(&ranked, "Quit application"),
            SpotlightAction::Command(ShellCommand::Quit)
        );
        assert_eq!(find_action(&ranked, "later"), SpotlightAction::Close);
        assert_eq!(
            find_action(&ranked, "Unrelated"),
            SpotlightAction::Command(ShellCommand::OpenChat)
        );
    }
}
