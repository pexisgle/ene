//! Runtime platform detection for app tools.

use serde_json::{Value, json};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::Path;

const SCREENSHOT_CLI_BACKENDS: &[&str] =
    &["grim", "import", "gnome-screenshot", "spectacle", "scrot"];
const X11_WINDOW_LIST_BACKENDS: &[&str] = &["wmctrl"];
const HYPRLAND_WINDOW_LIST_BACKENDS: &[&str] = &["hyprctl"];
const SWAY_WINDOW_LIST_BACKENDS: &[&str] = &["swaymsg"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionKind {
    Wayland,
    X11,
    Windows,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DesktopKind {
    Gnome,
    Kde,
    Hyprland,
    Sway,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActionCap {
    pub available: bool,
    pub backend: &'static str,
    pub reason: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlatformCaps {
    pub session: SessionKind,
    pub desktop: DesktopKind,
    pub screenshot: ActionCap,
    pub clipboard: ActionCap,
    pub window_list: ActionCap,
    pub active_window: ActionCap,
    pub list_monitors: ActionCap,
    pub input: ActionCap,
}

impl PlatformCaps {
    #[must_use]
    pub(crate) fn detect() -> Self {
        Self::from_pairs(std::env::vars())
    }

    #[must_use]
    pub(crate) fn from_pairs(vars: impl IntoIterator<Item = (String, String)>) -> Self {
        let env: HashMap<String, String> = vars.into_iter().collect();
        Self::from_env(&env)
    }

    #[must_use]
    pub(crate) fn from_env(env: &HashMap<String, String>) -> Self {
        let session = session_kind(env);
        let desktop = desktop_kind(env);
        let input = input_cap(session, desktop);
        Self {
            session,
            desktop,
            screenshot: screenshot_cap(session, env),
            clipboard: ActionCap {
                available: true,
                backend: if cfg!(windows) {
                    "arboard"
                } else {
                    "arboard+cli"
                },
                reason: None,
            },
            window_list: window_cap(session, desktop, env.get("PATH").map(String::as_str)),
            active_window: active_cap(session, desktop),
            list_monitors: monitor_cap(session, desktop),
            input,
        }
    }

    #[must_use]
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "session": session_name(self.session),
            "desktop": desktop_name(self.desktop),
            "actions": {
                "app.screenshot": action_json(&self.screenshot),
                "app.clipboard_get": action_json(&self.clipboard),
                "app.clipboard_set": action_json(&self.clipboard),
                "app.window_list": action_json(&self.window_list),
                "app.active_window": action_json(&self.active_window),
                "app.list_monitors": action_json(&self.list_monitors),
                "app.click": action_json(&self.input),
                "app.type": action_json(&self.input),
                "app.key": action_json(&self.input),
            }
        })
    }

    #[must_use]
    pub(crate) fn advertise_input(&self) -> bool {
        self.input.available
    }
}

fn session_kind(env: &HashMap<String, String>) -> SessionKind {
    if cfg!(windows) {
        return SessionKind::Windows;
    }
    let xdg = env
        .get("XDG_SESSION_TYPE")
        .map_or("", String::as_str)
        .to_ascii_lowercase();
    if !env.get("WAYLAND_DISPLAY").is_none_or(String::is_empty) || xdg == "wayland" {
        return SessionKind::Wayland;
    }
    if !env.get("DISPLAY").is_none_or(String::is_empty) || xdg == "x11" {
        return SessionKind::X11;
    }
    SessionKind::Unknown
}

fn desktop_kind(env: &HashMap<String, String>) -> DesktopKind {
    let raw = env
        .get("XDG_CURRENT_DESKTOP")
        .or_else(|| env.get("DESKTOP_SESSION"))
        .map_or("", String::as_str)
        .to_ascii_uppercase();
    if raw.contains("GNOME") {
        DesktopKind::Gnome
    } else if raw.contains("KDE") || raw.contains("PLASMA") {
        DesktopKind::Kde
    } else if raw.contains("HYPR") {
        DesktopKind::Hyprland
    } else if raw.contains("SWAY") {
        DesktopKind::Sway
    } else {
        DesktopKind::Other
    }
}

fn screenshot_cap(session: SessionKind, env: &HashMap<String, String>) -> ActionCap {
    match session {
        SessionKind::Wayland if portal_session_available(env) => ActionCap {
            available: true,
            backend: "portal",
            reason: None,
        },
        SessionKind::Wayland | SessionKind::X11 => screenshot_cli_cap(env),
        SessionKind::Windows => ActionCap {
            available: true,
            backend: "gdi",
            reason: None,
        },
        SessionKind::Unknown => ActionCap {
            available: false,
            backend: "none",
            reason: Some("no display session (WAYLAND_DISPLAY / DISPLAY / Windows)"),
        },
    }
}

fn screenshot_cli_cap(env: &HashMap<String, String>) -> ActionCap {
    match screenshot_cli_backend(env.get("PATH").map(String::as_str)) {
        Some(backend) => ActionCap {
            available: true,
            backend,
            reason: None,
        },
        None => ActionCap {
            available: false,
            backend: "none",
            reason: Some(
                "no screenshot backend (grim, ImageMagick import, gnome-screenshot, spectacle, or scrot)",
            ),
        },
    }
}

fn portal_session_available(env: &HashMap<String, String>) -> bool {
    env.get("DBUS_SESSION_BUS_ADDRESS")
        .is_some_and(|value| !value.is_empty())
}

pub(crate) fn screenshot_cli_backend(path: Option<&str>) -> Option<&'static str> {
    executable_backend(path, SCREENSHOT_CLI_BACKENDS)
}

pub(crate) fn window_list_cli_backend(
    session: SessionKind,
    desktop: DesktopKind,
    path: Option<&str>,
) -> Option<&'static str> {
    let candidates = match (session, desktop) {
        (SessionKind::X11, _) => X11_WINDOW_LIST_BACKENDS,
        (SessionKind::Wayland, DesktopKind::Hyprland) => HYPRLAND_WINDOW_LIST_BACKENDS,
        (SessionKind::Wayland, DesktopKind::Sway) => SWAY_WINDOW_LIST_BACKENDS,
        _ => return None,
    };
    executable_backend(path, candidates)
}

fn executable_backend(path: Option<&str>, candidates: &[&'static str]) -> Option<&'static str> {
    let path = OsStr::new(path?);
    candidates.iter().copied().find(|backend| {
        std::env::split_paths(path).any(|directory| executable_file(&directory.join(backend)))
    })
}

fn executable_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        path.metadata()
            .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn window_cap(session: SessionKind, desktop: DesktopKind, path: Option<&str>) -> ActionCap {
    match (session, desktop) {
        (SessionKind::Windows, _) => ActionCap {
            available: false,
            backend: "none",
            reason: Some("Windows window-list enumeration is not implemented"),
        },
        (SessionKind::X11, _)
        | (SessionKind::Wayland, DesktopKind::Hyprland | DesktopKind::Sway) => {
            match window_list_cli_backend(session, desktop, path) {
                Some(backend) => ActionCap {
                    available: true,
                    backend,
                    reason: None,
                },
                None => ActionCap {
                    available: false,
                    backend: "none",
                    reason: Some(match (session, desktop) {
                        (SessionKind::X11, _) => {
                            "install wmctrl and ensure it is executable on PATH"
                        }
                        (SessionKind::Wayland, DesktopKind::Hyprland) => {
                            "install hyprctl and ensure it is executable on PATH"
                        }
                        (SessionKind::Wayland, DesktopKind::Sway) => {
                            "install swaymsg and ensure it is executable on PATH"
                        }
                        _ => "window-list helper is missing or not executable on PATH",
                    }),
                },
            }
        }
        (SessionKind::Wayland, DesktopKind::Gnome | DesktopKind::Kde) => ActionCap {
            available: false,
            backend: "none",
            reason: Some("GNOME/KDE Wayland has no stable window-list protocol for this tool"),
        },
        _ => ActionCap {
            available: false,
            backend: "none",
            reason: Some("window list backend unknown for this session"),
        },
    }
}

fn active_cap(session: SessionKind, desktop: DesktopKind) -> ActionCap {
    match (session, desktop) {
        (SessionKind::Windows, _) => ActionCap {
            available: true,
            backend: "win32",
            reason: None,
        },
        (SessionKind::X11, _) => ActionCap {
            available: true,
            backend: "xdotool",
            reason: Some("needs xdotool"),
        },
        (SessionKind::Wayland, DesktopKind::Hyprland) => ActionCap {
            available: true,
            backend: "hyprctl",
            reason: None,
        },
        (SessionKind::Wayland, DesktopKind::Sway) => ActionCap {
            available: true,
            backend: "swaymsg",
            reason: None,
        },
        _ => ActionCap {
            available: false,
            backend: "none",
            reason: Some("active window is not exposed on this compositor"),
        },
    }
}

fn monitor_cap(session: SessionKind, desktop: DesktopKind) -> ActionCap {
    match (session, desktop) {
        (SessionKind::Windows, _) => ActionCap {
            available: true,
            backend: "gdi",
            reason: None,
        },
        (SessionKind::X11, _) => ActionCap {
            available: true,
            backend: "xrandr",
            reason: Some("needs xrandr"),
        },
        (SessionKind::Wayland, DesktopKind::Hyprland) => ActionCap {
            available: true,
            backend: "hyprctl",
            reason: None,
        },
        (SessionKind::Wayland, DesktopKind::Sway) => ActionCap {
            available: true,
            backend: "swaymsg",
            reason: None,
        },
        (SessionKind::Wayland, _) => ActionCap {
            available: true,
            backend: "portal",
            reason: Some(
                "logical size comes from the captured PNG; compositor layout may be absent",
            ),
        },
        (SessionKind::Unknown, _) => ActionCap {
            available: false,
            backend: "none",
            reason: Some("no display session"),
        },
    }
}

fn input_cap(session: SessionKind, desktop: DesktopKind) -> ActionCap {
    match (session, desktop) {
        (SessionKind::Windows, _) => ActionCap {
            available: true,
            backend: "win32",
            reason: None,
        },
        (SessionKind::X11, _) => ActionCap {
            available: true,
            backend: "xdotool",
            reason: Some("needs xdotool"),
        },
        (SessionKind::Wayland, DesktopKind::Gnome | DesktopKind::Kde) => ActionCap {
            available: false,
            backend: "none",
            reason: Some("pointer/key injection is not guaranteed on GNOME/KDE Wayland"),
        },
        (SessionKind::Wayland, _) => ActionCap {
            available: false,
            backend: "none",
            reason: Some("Wayland input injection is compositor-specific and not advertised"),
        },
        (SessionKind::Unknown, _) => ActionCap {
            available: false,
            backend: "none",
            reason: Some("no display session"),
        },
    }
}

fn session_name(kind: SessionKind) -> &'static str {
    match kind {
        SessionKind::Wayland => "wayland",
        SessionKind::X11 => "x11",
        SessionKind::Windows => "windows",
        SessionKind::Unknown => "unknown",
    }
}

fn desktop_name(kind: DesktopKind) -> &'static str {
    match kind {
        DesktopKind::Gnome => "gnome",
        DesktopKind::Kde => "kde",
        DesktopKind::Hyprland => "hyprland",
        DesktopKind::Sway => "sway",
        DesktopKind::Other => "other",
    }
}

fn action_json(cap: &ActionCap) -> Value {
    json!({
        "available": cap.available,
        "backend": cap.backend,
        "reason": cap.reason,
    })
}

pub(crate) fn fail(
    code: &'static str,
    backend: impl Into<String>,
    message: impl Into<String>,
) -> String {
    json!({
        "code": code,
        "backend": backend.into(),
        "message": message.into(),
    })
    .to_string()
}

pub(crate) fn dependency_missing(backend: &'static str, package: &'static str) -> String {
    json!({
        "code": "dependency_missing",
        "backend": backend,
        "package": package,
        "message": format!("install {package} and ensure it is executable on PATH"),
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        DesktopKind, PlatformCaps, SessionKind, screenshot_cli_backend, window_list_cli_backend,
    };
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[cfg(not(windows))]
    #[test]
    fn gnome_wayland_hides_input_and_explains_why() {
        let caps = PlatformCaps::from_env(&env(&[
            ("XDG_SESSION_TYPE", "wayland"),
            ("WAYLAND_DISPLAY", "wayland-0"),
            ("XDG_CURRENT_DESKTOP", "GNOME"),
            ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/tmp/session-bus"),
        ]));
        assert_eq!(caps.session, SessionKind::Wayland);
        assert_eq!(caps.desktop, DesktopKind::Gnome);
        assert!(!caps.advertise_input());
        assert!(!caps.window_list.available);
        let json = caps.to_json();
        assert_eq!(json["actions"]["app.click"]["available"], false);
        assert!(
            json["actions"]["app.click"]["reason"]
                .as_str()
                .unwrap()
                .contains("GNOME")
        );
        assert_eq!(json["actions"]["app.screenshot"]["backend"], "portal");
    }

    #[cfg(not(windows))]
    #[test]
    fn x11_advertises_input() {
        let caps = PlatformCaps::from_env(&env(&[("XDG_SESSION_TYPE", "x11"), ("DISPLAY", ":1")]));
        assert_eq!(caps.session, SessionKind::X11);
        assert!(caps.advertise_input());
        assert_eq!(caps.input.backend, "xdotool");
    }

    #[cfg(unix)]
    #[test]
    fn screenshot_cli_probe_requires_an_executable_backend() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let backend = dir.path().join("grim");
        std::fs::write(&backend, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&backend, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = dir.path().to_str().unwrap();

        assert_eq!(screenshot_cli_backend(Some(path)), Some("grim"));
        assert_eq!(screenshot_cli_backend(Some("/definitely/missing")), None);
    }

    #[cfg(unix)]
    #[test]
    fn window_list_probe_requires_an_executable_backend() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let backend = dir.path().join("wmctrl");
        std::fs::write(&backend, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&backend, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = dir.path().to_str().unwrap();

        assert_eq!(
            window_list_cli_backend(SessionKind::X11, DesktopKind::Other, Some(path)),
            Some("wmctrl")
        );
        assert_eq!(
            window_list_cli_backend(
                SessionKind::X11,
                DesktopKind::Other,
                Some("/definitely/missing")
            ),
            None
        );
    }

    #[test]
    fn x11_without_a_screenshot_backend_is_unavailable() {
        let caps = PlatformCaps::from_env(&env(&[
            ("XDG_SESSION_TYPE", "x11"),
            ("DISPLAY", ":1"),
            ("PATH", "/definitely/missing"),
        ]));

        assert!(!caps.screenshot.available);
        assert_eq!(caps.screenshot.backend, "none");
        assert!(caps.screenshot.reason.is_some());
        assert!(!caps.window_list.available);
        assert_eq!(caps.window_list.backend, "none");
    }
}
