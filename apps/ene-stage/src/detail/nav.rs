//! Detail window top navigation: search field and scrollable tab chips.

use egui::{Button, CornerRadius, Frame, Margin, RichText, Ui};

use crate::i18n;

use super::DetailTab;

/// Returns the tab the user picked, if any.
pub(crate) fn show_nav_bar(
    ui: &mut Ui,
    search: &mut String,
    selected: DetailTab,
) -> Option<DetailTab> {
    let mut clicked = None;
    Frame::new()
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            render_search(ui, search);
            ui.add_space(8.0);
            egui::ScrollArea::horizontal()
                .id_salt("detail-tab-scroll")
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let mut first = true;
                        for tab in DetailTab::ALL {
                            if !tab.matches_search(search) {
                                continue;
                            }
                            if tab_group_starts_here(tab) {
                                if !first {
                                    ui.add_space(6.0);
                                    ui.separator();
                                    ui.add_space(6.0);
                                }
                                first = false;
                            }
                            if tab_chip(ui, tab, selected == tab) {
                                clicked = Some(tab);
                            }
                        }
                    });
                });
        });
    clicked
}

fn render_search(ui: &mut Ui, search: &mut String) {
    ui.horizontal(|ui| {
        let clear_width = if search.is_empty() {
            0.0
        } else {
            52.0
        };
        let input_width =
            (ui.available_width() - clear_width - ui.spacing().item_spacing.x).max(120.0);
        ui.add(
            egui::TextEdit::singleline(search)
                .id_salt("detail-nav-search")
                .hint_text(i18n::fl("detail-search-hint"))
                .desired_width(input_width),
        );
        if !search.is_empty()
            && ui
                .small_button(i18n::fl("detail-search-clear"))
                .clicked()
        {
            search.clear();
        }
    });
}

#[must_use]
fn tab_group_starts_here(tab: DetailTab) -> bool {
    matches!(
        tab,
        DetailTab::Home | DetailTab::Companion | DetailTab::Connections | DetailTab::System
    )
}

fn tab_chip(ui: &mut Ui, tab: DetailTab, active: bool) -> bool {
    let visuals = ui.visuals();
    let label = tab.label();
    let text = if active {
        RichText::new(label).strong().color(visuals.strong_text_color())
    } else {
        RichText::new(label).color(visuals.weak_text_color())
    };
    let (fill, stroke) = if active {
        (
            visuals.selection.bg_fill,
            visuals.selection.stroke,
        )
    } else {
        (
            visuals.faint_bg_color,
            visuals.widgets.noninteractive.bg_stroke,
        )
    };
    let button = Button::new(text)
        .fill(fill)
        .stroke(stroke)
        .corner_radius(CornerRadius::same(6));
    ui.add(button).clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_starts_mark_section_boundaries() {
        assert!(tab_group_starts_here(DetailTab::Home));
        assert!(tab_group_starts_here(DetailTab::Companion));
        assert!(!tab_group_starts_here(DetailTab::Conversation));
        assert!(tab_group_starts_here(DetailTab::Connections));
        assert!(tab_group_starts_here(DetailTab::System));
        assert!(!tab_group_starts_here(DetailTab::Log));
    }
}
