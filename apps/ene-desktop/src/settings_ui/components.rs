//! Shared layout components for the settings pages.
//!
//! Every page builds on these primitives so the whole window shares one
//! visual language: a page header, rounded section cards, label/control
//! rows, toggles, sliders, badges, warnings, empty states, and danger
//! buttons. Section cards register a stable id that the settings search
//! uses to navigate and temporarily highlight a specific section.
#![expect(
    dead_code,
    reason = "shared widgets stay for pages that have not been rewired to core JSON yet"
)]

use std::ops::RangeInclusive;
use std::time::{Duration, Instant};

use egui::{
    Align, Color32, CornerRadius, FontId, Frame, Id, Layout, Margin, RichText, Sense, Ui, vec2,
};

/// Below this content width the sidebar collapses into a page picker so the
/// window stays usable at its minimum size.
pub const NARROW_NAV_THRESHOLD: f32 = 720.0;

const SECTION_FOCUS_KEY: &str = "settings_section_focus";
const SECTION_HIGHLIGHT_SECS: Duration = Duration::from_millis(1400);

/// One-shot request to reveal and highlight a section. Created by the
/// settings search when the user picks a section result; consumed by
/// [`section_card`] / [`section_card_collapsible`] on the target page.
#[derive(Clone, Copy, Debug)]
pub struct SectionFocus {
    pub section: &'static str,
    pub until: Instant,
    pub scrolled: bool,
}

/// Ask the current frame's render pass to reveal `section` (scroll to it,
/// force it open, and tint it briefly).
pub fn request_section_focus(ctx: &egui::Context, section: &'static str) {
    ctx.data_mut(|data| {
        data.insert_temp(
            Id::new(SECTION_FOCUS_KEY),
            SectionFocus {
                section,
                until: Instant::now() + SECTION_HIGHLIGHT_SECS,
                scrolled: false,
            },
        );
    });
}

fn focus_state(ctx: &egui::Context) -> Option<SectionFocus> {
    ctx.data_mut(|data| data.get_temp::<SectionFocus>(Id::new(SECTION_FOCUS_KEY)))
}

fn update_focus(ctx: &egui::Context, focus: SectionFocus) {
    ctx.data_mut(|data| {
        data.insert_temp(Id::new(SECTION_FOCUS_KEY), focus);
    });
}

fn focus_accent(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(45, 58, 82)
    } else {
        Color32::from_rgb(222, 232, 247)
    }
}

pub(super) const SECTION_TITLE_KEYS: [(&str, &str); 56] = [
    ("character-model", "section-character-model"),
    ("character-transform", "section-character-transform"),
    ("character-expressions", "section-character-expressions"),
    ("editor-identity", "character-editor-section-identity"),
    ("editor-personality", "character-editor-section-personality"),
    ("editor-scenario", "character-editor-section-scenario"),
    ("editor-greetings", "character-editor-section-greetings"),
    (
        "editor-memory",
        "character-editor-section-memory-instructions",
    ),
    ("editor-lorebook", "character-editor-section-lorebook"),
    ("editor-motions", "character-editor-section-motions"),
    ("graphics-quality", "section-graphics-quality"),
    ("graphics-language", "section-graphics-language"),
    ("graphics-theme", "section-graphics-theme"),
    ("ai-chat", "section-ai-chat"),
    ("ai-embedding", "section-ai-embedding"),
    ("ai-health", "section-ai-health"),
    ("voice-tts", "audio-tts-section"),
    ("voice-stt", "audio-stt-section"),
    ("voice-mic", "audio-mic-section"),
    ("features-mind", "features-mind"),
    ("accessibility-overlays", "section-accessibility-overlays"),
    ("memory-browse", "memory-page-tab-browse"),
    ("memory-recall", "memory-page-tab-recall"),
    ("memory-pending", "memory-page-tab-pending"),
    ("memory-commitments", "memory-commitments-title"),
    ("ledger-browse", "memory-ledger-title"),
    ("ledger-commitments", "memory-ledger-commitments-title"),
    ("permissions-pending", "permissions-pending"),
    ("permissions-grants", "permissions-granted"),
    ("approvals-policy", "approvals-title"),
    ("engines-catalog", "engines-catalog"),
    ("engines-list", "engines-list"),
    ("connectors-list", "connectors-list"),
    ("connectors-detail", "connectors-status-title"),
    ("sessions-list", "sessions-list-title"),
    ("sessions-search", "sessions-search-title"),
    ("sessions-import", "sessions-import-title"),
    ("debug-overlays", "section-debug-overlays"),
    ("debug-pipeline", "section-debug-pipeline"),
    ("overview-needs-config", "overview-needs-config"),
    ("overview-issues", "overview-issues"),
    ("overview-restart-pending", "overview-restart-pending"),
    ("overview-credentials", "overview-credentials"),
    ("schedules-list", "schedules-list-title"),
    ("schedules-history", "schedules-history-title"),
    ("schedules-pending", "schedules-pending-title"),
    ("schedules-add", "schedules-add-title"),
    ("memory-config", "memory-config-storage"),
    ("memory-approval", "memory-config-approval"),
    ("memory-limits", "memory-config-limits"),
    ("plugins-general", "plugins-general-title"),
    ("plugins-tools", "plugins-tab-tools"),
    ("plugins-providers", "plugins-tab-providers"),
    ("plugins-mcp", "plugins-mcp-title"),
    ("plugins-discovered", "plugins-discovered-title"),
    ("advanced-sections", "advanced-sections"),
];

/// Localized title for a registered section id, if the section is
/// searchable. Section ids are stable `snake_case` strings shared between
/// the page metadata in `mod.rs` and the page renderers.
pub fn section_title(section: &str) -> Option<String> {
    SECTION_TITLE_KEYS
        .iter()
        .find(|(id, _)| *id == section)
        .map(|(_, key)| crate::i18n::loader().get(key))
}

pub fn page_header(ui: &mut Ui, title: &str, description: &str) {
    ui.add_space(6.0);
    ui.heading(RichText::new(title).size(21.0));
    if !description.is_empty() {
        ui.add_space(2.0);
        ui.weak(description);
    }
    ui.add_space(8.0);
}

/// Rounded card wrapping one logical settings section. `id` is the stable
/// section identifier used by the search / focus system.
pub fn section_card(
    ui: &mut Ui,
    id: &str,
    title: &str,
    body: impl FnOnce(&mut Ui),
) -> egui::Response {
    let ctx = ui.ctx().clone();
    let focus = focus_state(&ctx);
    let is_target = focus.is_some_and(|f| f.section == id);
    let should_scroll = is_target && focus.is_some_and(|f| !f.scrolled);
    let highlight = is_target && focus.is_some_and(|f| Instant::now() < f.until);

    let mut frame = Frame::group(ui.style())
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(10));
    if highlight {
        frame = frame.fill(focus_accent(ui));
    }
    let response = frame
        .show(ui, |ui| {
            ui.label(RichText::new(title).strong());
            ui.add_space(6.0);
            body(ui);
        })
        .response;

    if should_scroll && let Some(focus) = focus {
        ui.scroll_to_rect(response.rect, Some(Align::Center));
        update_focus(
            &ctx,
            SectionFocus {
                section: focus.section,
                until: focus.until,
                scrolled: true,
            },
        );
    }
    response
}

/// Collapsible variant used by dense editors; a pending focus request
/// forces the section open and scrolls its header into view.
pub fn section_card_collapsible(
    ui: &mut Ui,
    id: &str,
    title: &str,
    default_open: bool,
    body: impl FnOnce(&mut Ui),
) -> egui::CollapsingResponse<()> {
    let ctx = ui.ctx().clone();
    let focus = focus_state(&ctx);
    let is_target = focus.is_some_and(|f| f.section == id);
    let should_scroll = is_target && focus.is_some_and(|f| !f.scrolled);
    let highlight = is_target && focus.is_some_and(|f| Instant::now() < f.until);

    let mut header = egui::CollapsingHeader::new(RichText::new(title).strong())
        .id_salt(id)
        .default_open(default_open);
    if is_target {
        header = header.open(Some(true));
    }
    let response = header.show(ui, body);

    if highlight {
        let tint = Color32::from_rgba_unmultiplied(
            focus_accent(ui).r(),
            focus_accent(ui).g(),
            focus_accent(ui).b(),
            90,
        );
        let rect = response.header_response.rect.expand2(vec2(4.0, 2.0));
        ui.painter().rect_filled(rect, CornerRadius::same(6), tint);
    }
    if should_scroll && let Some(focus) = focus {
        ui.scroll_to_rect(response.header_response.rect, Some(Align::Center));
        update_focus(
            &ctx,
            SectionFocus {
                section: focus.section,
                until: focus.until,
                scrolled: true,
            },
        );
    }
    response
}

/// Label column (title + optional hint) with the control right-aligned.
pub fn setting_row(
    ui: &mut Ui,
    id_salt: &str,
    title: &str,
    hint: &str,
    content: impl FnOnce(&mut Ui),
) -> egui::Response {
    ui.push_id(id_salt, |ui| {
        if ui.available_width() < 560.0 {
            ui.vertical(|ui| {
                ui.label(RichText::new(title));
                if !hint.is_empty() {
                    ui.weak(RichText::new(hint).small());
                }
                ui.add_space(2.0);
                ui.horizontal_wrapped(content);
            })
            .response
        } else {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new(title));
                    if !hint.is_empty() {
                        ui.weak(RichText::new(hint).small());
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_space(8.0);
                    ui.horizontal_wrapped(content);
                });
            })
            .response
        }
    })
    .inner
}

/// Toggle row with the switch right-aligned and the label + hint on the
/// left. Returns `true` when the toggle changed.
pub fn toggle_row(ui: &mut Ui, id_salt: &str, title: &str, hint: &str, checked: &mut bool) -> bool {
    let mut changed = false;
    setting_row(ui, id_salt, title, hint, |ui| {
        changed = ui.add(egui::Checkbox::without_text(checked)).changed();
    });
    changed
}

/// Slider with a numeric readout, both right-aligned behind the label.
pub fn slider_row(
    ui: &mut Ui,
    id_salt: &str,
    title: &str,
    hint: &str,
    value: &mut f32,
    range: RangeInclusive<f32>,
    step: f32,
    format: impl Fn(f32) -> String,
) -> bool {
    let mut changed = false;
    setting_row(ui, id_salt, title, hint, |ui| {
        let slider_width = (ui.available_width() - 72.0).clamp(120.0, 240.0);
        changed = ui
            .add_sized(
                [slider_width, 0.0],
                egui::Slider::new(value, range)
                    .step_by(f64::from(step))
                    .show_value(false),
            )
            .changed();
        ui.monospace(format(*value));
    });
    changed
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadgeTone {
    Neutral,
    Ok,
    Warn,
    Error,
}

pub fn status_badge(ui: &mut Ui, text: &str, tone: BadgeTone) {
    let dark = ui.visuals().dark_mode;
    let (bg, fg) = match tone {
        BadgeTone::Neutral => (
            if dark {
                Color32::from_rgb(52, 56, 64)
            } else {
                Color32::from_rgb(224, 227, 232)
            },
            if dark {
                Color32::from_rgb(210, 214, 222)
            } else {
                Color32::from_rgb(60, 64, 72)
            },
        ),
        BadgeTone::Ok => (
            if dark {
                Color32::from_rgb(38, 78, 52)
            } else {
                Color32::from_rgb(208, 238, 214)
            },
            if dark {
                Color32::from_rgb(150, 220, 165)
            } else {
                Color32::from_rgb(25, 92, 44)
            },
        ),
        BadgeTone::Warn => (
            if dark {
                Color32::from_rgb(88, 66, 26)
            } else {
                Color32::from_rgb(250, 236, 196)
            },
            if dark {
                Color32::from_rgb(235, 195, 110)
            } else {
                Color32::from_rgb(125, 82, 12)
            },
        ),
        BadgeTone::Error => (
            if dark {
                Color32::from_rgb(94, 38, 40)
            } else {
                Color32::from_rgb(250, 220, 220)
            },
            if dark {
                Color32::from_rgb(240, 150, 150)
            } else {
                Color32::from_rgb(160, 35, 38)
            },
        ),
    };
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), FontId::proportional(11.5), fg);
    let size = vec2(galley.size().x + 14.0, galley.size().y + 5.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(10), bg);
    ui.painter().galley(rect.min + vec2(7.0, 2.5), galley, fg);
}

/// For recoverable problems (missing assets, validation warnings).
pub fn warning_box(ui: &mut Ui, text: &str) {
    let dark = ui.visuals().dark_mode;
    let (bg, fg) = if dark {
        (
            Color32::from_rgb(62, 48, 24),
            Color32::from_rgb(235, 195, 110),
        )
    } else {
        (
            Color32::from_rgb(250, 240, 208),
            Color32::from_rgb(125, 82, 12),
        )
    };
    Frame::new()
        .fill(bg)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.colored_label(fg, RichText::new(text));
        });
}

/// For errors that block or failed an operation.
pub fn error_box(ui: &mut Ui, text: &str) {
    let dark = ui.visuals().dark_mode;
    let (bg, fg) = if dark {
        (
            Color32::from_rgb(74, 32, 34),
            Color32::from_rgb(240, 150, 150),
        )
    } else {
        (
            Color32::from_rgb(252, 228, 228),
            Color32::from_rgb(160, 35, 38),
        )
    };
    Frame::new()
        .fill(bg)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.colored_label(fg, RichText::new(text));
        });
}

pub fn empty_state(ui: &mut Ui, title: &str, detail: &str) {
    ui.add_space(8.0);
    ui.vertical_centered(|ui| {
        ui.weak(RichText::new(title).strong());
        if !detail.is_empty() {
            ui.weak(detail);
        }
    });
    ui.add_space(8.0);
}

/// Destructive-action button with a red treatment.
pub fn danger_button(ui: &mut Ui, text: &str) -> egui::Response {
    let dark = ui.visuals().dark_mode;
    let (bg, fg) = if dark {
        (
            Color32::from_rgb(96, 32, 34),
            Color32::from_rgb(255, 190, 190),
        )
    } else {
        (
            Color32::from_rgb(255, 228, 228),
            Color32::from_rgb(160, 30, 34),
        )
    };
    ui.add(egui::Button::new(RichText::new(text).color(fg)).fill(bg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_section_titles_resolve() {
        for id in [
            "character-model",
            "editor-lorebook",
            "graphics-theme",
            "ai-chat",
            "voice-tts",
            "memory-commitments",
            "ledger-browse",
            "permissions-pending",
            "connectors-list",
            "sessions-import",
            "debug-pipeline",
        ] {
            assert!(section_title(id).is_some(), "missing title for {id}");
        }
    }

    #[test]
    fn unknown_section_has_no_title() {
        assert!(section_title("not-a-section").is_none());
    }
}
