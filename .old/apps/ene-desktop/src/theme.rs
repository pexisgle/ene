//! App-wide light/dark theme resolution shared by every egui window.
//!
//! The runtime pushes the persisted `desktop.theme` preference and the
//! OS color scheme (XDG settings portal on Linux, winit `ThemeChanged`
//! on Windows) into process-wide atomics. Each window applies the
//! resolved palette per frame, so a preference change or an OS theme
//! switch takes effect immediately without cross-window messaging.

use std::sync::atomic::{AtomicU8, Ordering};

use crate::settings::DesktopThemePreference;

/// Resolved color scheme an egui window should render with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeMode {
    Light,
    Dark,
}

const OS_UNKNOWN: u8 = 0;
const OS_LIGHT: u8 = 1;
const OS_DARK: u8 = 2;
const PREF_SYSTEM: u8 = 0;
const PREF_LIGHT: u8 = 1;
const PREF_DARK: u8 = 2;

static OS_THEME: AtomicU8 = AtomicU8::new(OS_UNKNOWN);
static PREFERENCE: AtomicU8 = AtomicU8::new(PREF_SYSTEM);
static RESOLVED: AtomicU8 = AtomicU8::new(OS_DARK);

fn preference_to_u8(preference: DesktopThemePreference) -> u8 {
    match preference {
        DesktopThemePreference::System => PREF_SYSTEM,
        DesktopThemePreference::Light => PREF_LIGHT,
        DesktopThemePreference::Dark => PREF_DARK,
    }
}

fn os_u8_to_mode(value: u8) -> ThemeMode {
    if value == OS_LIGHT {
        ThemeMode::Light
    } else {
        ThemeMode::Dark
    }
}

fn resolve_mode(preference: u8, os_theme: u8) -> ThemeMode {
    match preference {
        PREF_LIGHT => ThemeMode::Light,
        PREF_DARK => ThemeMode::Dark,
        _ => {
            if os_theme == OS_UNKNOWN {
                ThemeMode::Dark
            } else {
                os_u8_to_mode(os_theme)
            }
        }
    }
}

/// Record the OS color scheme. Unknown until the platform reports one;
/// the fallback is dark (see [`resolved_theme`]).
pub fn set_os_theme(mode: ThemeMode) {
    OS_THEME.store(
        if mode == ThemeMode::Light {
            OS_LIGHT
        } else {
            OS_DARK
        },
        Ordering::Relaxed,
    );
    resolve();
}

/// Push the persisted preference; `System` resolves against the latest
/// OS color scheme.
pub fn set_preference(preference: DesktopThemePreference) {
    PREFERENCE.store(preference_to_u8(preference), Ordering::Relaxed);
    resolve();
}

fn resolve() {
    let mode = resolve_mode(
        PREFERENCE.load(Ordering::Relaxed),
        OS_THEME.load(Ordering::Relaxed),
    );
    RESOLVED.store(
        if mode == ThemeMode::Light {
            OS_LIGHT
        } else {
            OS_DARK
        },
        Ordering::Relaxed,
    );
}

/// Theme every window should render with this frame.
pub fn resolved_theme() -> ThemeMode {
    os_u8_to_mode(RESOLVED.load(Ordering::Relaxed))
}

/// Translate the resolved theme into the winit decoration hint. The
/// runtime calls this whenever the resolved theme changes and passes it
/// to every window (`Window::set_theme`) so native title bars match the
/// egui content.
pub fn resolved_winit_theme() -> winit::window::Theme {
    mode_to_winit(resolved_theme())
}

fn mode_to_winit(mode: ThemeMode) -> winit::window::Theme {
    match mode {
        ThemeMode::Light => winit::window::Theme::Light,
        ThemeMode::Dark => winit::window::Theme::Dark,
    }
}

/// Keep a newly created window's native decorations in sync immediately;
/// later changes are propagated by the runtime's theme reconciliation.
pub fn apply_native_theme(window: &winit::window::Window) {
    window.set_theme(Some(resolved_winit_theme()));
}

/// Seed the system preference before an explicit window theme override is
/// applied. Windows exposes the current OS scheme through winit; Linux uses
/// the XDG settings portal watcher instead.
#[cfg(target_os = "windows")]
pub fn observe_winit_system_theme(window: &winit::window::Window) {
    if let Some(theme) = window.theme() {
        set_os_theme(match theme {
            winit::window::Theme::Light => ThemeMode::Light,
            winit::window::Theme::Dark => ThemeMode::Dark,
        });
    }
}

/// Apply the resolved theme's egui palette to `ctx`. Both palettes are
/// registered so popups and menus spawned by either theme render with the
/// app's colors, then the active theme selects which one egui uses.
pub fn apply_egui_visuals(ctx: &egui::Context) {
    ctx.set_visuals_of(egui::Theme::Dark, palette(ThemeMode::Dark));
    ctx.set_visuals_of(egui::Theme::Light, palette(ThemeMode::Light));
    ctx.set_theme(match resolved_theme() {
        ThemeMode::Dark => egui::Theme::Dark,
        ThemeMode::Light => egui::Theme::Light,
    });
}

fn palette(mode: ThemeMode) -> egui::Visuals {
    let mut visuals = match mode {
        ThemeMode::Dark => egui::Visuals::dark(),
        ThemeMode::Light => egui::Visuals::light(),
    };
    match mode {
        ThemeMode::Dark => {
            visuals.panel_fill = egui::Color32::from_rgb(26, 28, 33);
            visuals.window_fill = egui::Color32::from_rgb(20, 22, 28);
            visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(30, 33, 38);
            visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(38, 42, 50);
            visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(52, 57, 66);
            visuals.widgets.active.bg_fill = egui::Color32::from_rgb(72, 77, 89);
            visuals.widgets.inactive.fg_stroke.color = egui::Color32::from_rgb(220, 224, 232);
            visuals.widgets.hovered.fg_stroke.color = egui::Color32::from_rgb(240, 243, 248);
            visuals.widgets.active.fg_stroke.color = egui::Color32::from_rgb(247, 248, 250);
        }
        ThemeMode::Light => {
            visuals.panel_fill = egui::Color32::from_rgb(244, 245, 247);
            visuals.window_fill = egui::Color32::from_rgb(250, 250, 252);
            visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(233, 235, 239);
            visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(226, 229, 234);
            visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(211, 215, 222);
            visuals.widgets.active.bg_fill = egui::Color32::from_rgb(196, 202, 211);
            visuals.widgets.inactive.fg_stroke.color = egui::Color32::from_rgb(46, 50, 58);
            visuals.widgets.hovered.fg_stroke.color = egui::Color32::from_rgb(24, 27, 32);
            visuals.widgets.active.fg_stroke.color = egui::Color32::from_rgb(12, 14, 18);
        }
    }
    visuals
}

/// Subscribe to the XDG settings portal color scheme and feed changes
/// into [`set_os_theme`]. Runs until the process exits; a missing portal
/// or read failure only leaves the OS scheme unknown (dark fallback).
#[cfg(target_os = "linux")]
pub fn spawn_os_theme_watch() {
    tokio::spawn(async {
        use futures::StreamExt;

        let settings = match ashpd::desktop::settings::Settings::new().await {
            Ok(settings) => settings,
            Err(error) => {
                tracing::debug!(%error, "XDG settings portal unavailable; using default theme");
                return;
            }
        };
        match settings.color_scheme().await {
            Ok(scheme) => set_os_theme(color_scheme_to_mode(scheme)),
            Err(error) => tracing::debug!(%error, "Failed to read XDG color scheme"),
        }
        let Ok(mut stream) = settings.receive_color_scheme_changed().await else {
            return;
        };
        while let Some(scheme) = stream.next().await {
            set_os_theme(color_scheme_to_mode(scheme));
        }
    });
}

#[cfg(target_os = "linux")]
fn color_scheme_to_mode(scheme: ashpd::desktop::settings::ColorScheme) -> ThemeMode {
    match scheme {
        ashpd::desktop::settings::ColorScheme::PreferLight => ThemeMode::Light,
        ashpd::desktop::settings::ColorScheme::PreferDark
        | ashpd::desktop::settings::ColorScheme::NoPreference => ThemeMode::Dark,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static THEME_STATE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn unknown_os_falls_back_to_dark() {
        assert_eq!(resolve_mode(PREF_SYSTEM, OS_UNKNOWN), ThemeMode::Dark);
    }

    #[test]
    fn system_preference_follows_os() {
        assert_eq!(resolve_mode(PREF_SYSTEM, OS_LIGHT), ThemeMode::Light);
        assert_eq!(resolve_mode(PREF_SYSTEM, OS_DARK), ThemeMode::Dark);
    }

    #[test]
    fn explicit_preference_overrides_os() {
        assert_eq!(resolve_mode(PREF_DARK, OS_LIGHT), ThemeMode::Dark);
        assert_eq!(resolve_mode(PREF_LIGHT, OS_DARK), ThemeMode::Light);
    }

    #[test]
    fn winit_translation_is_stable() {
        assert_eq!(mode_to_winit(ThemeMode::Dark), winit::window::Theme::Dark);
        assert_eq!(mode_to_winit(ThemeMode::Light), winit::window::Theme::Light);
    }

    #[test]
    fn explicit_preference_beats_os_change_in_shared_state() {
        let _guard = THEME_STATE_LOCK.lock().unwrap();
        set_os_theme(ThemeMode::Dark);
        set_preference(DesktopThemePreference::Light);
        assert_eq!(resolved_theme(), ThemeMode::Light);
        set_os_theme(ThemeMode::Light);
        assert_eq!(resolved_theme(), ThemeMode::Light);
    }

    #[test]
    fn system_preference_follows_os_in_shared_state() {
        let _guard = THEME_STATE_LOCK.lock().unwrap();
        set_os_theme(ThemeMode::Light);
        set_preference(DesktopThemePreference::System);
        assert_eq!(resolved_theme(), ThemeMode::Light);
        set_os_theme(ThemeMode::Dark);
        assert_eq!(resolved_theme(), ThemeMode::Dark);
    }

    #[test]
    fn unresolved_os_falls_back_to_dark_in_shared_state() {
        let _guard = THEME_STATE_LOCK.lock().unwrap();
        OS_THEME.store(OS_UNKNOWN, Ordering::Relaxed);
        set_preference(DesktopThemePreference::System);
        assert_eq!(resolved_theme(), ThemeMode::Dark);
    }
}
