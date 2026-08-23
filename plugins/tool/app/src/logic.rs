use ene_plugin_ipc::ToolSpecWire;
use ene_registry::{arg_str, spec};
use serde_json::{Value, json};
use std::collections::HashMap;

#[path = "backend.rs"]
mod backend;
#[path = "capability.rs"]
mod capability;
#[path = "capture.rs"]
mod capture;
#[path = "clipboard.rs"]
mod clipboard;
#[path = "hostcmd.rs"]
mod hostcmd;
#[cfg(windows)]
#[path = "win32.rs"]
mod win32;

use backend::{Backend, BackendAvailability};
use capability::{ActionCap, PlatformCaps, fail, window_availability_for_env};
use hostcmd::{run, stdout_text};

pub(crate) fn specs() -> Vec<ToolSpecWire> {
    let env: HashMap<String, String> = std::env::vars().collect();
    let caps = PlatformCaps::from_pairs(std::env::vars());
    let availability = window_availability_for_env(&env);
    specs_for(&PlatformCaps {
        window_list: cap_from_availability(&availability),
        ..caps
    })
}

fn run_window_backend(backend: &Backend) -> Result<String, String> {
    match backend.name {
        "wmctrl" => stdout_text(backend.executable, &["-l"]),
        "hyprctl" => stdout_text(backend.executable, &["clients"]),
        "swaymsg" => stdout_text(backend.executable, &["-t", "get_tree"]),
        "win32" => {
            #[cfg(windows)]
            {
                win32::list_windows()
            }
            #[cfg(not(windows))]
            {
                Err("Win32 window API is only available on Windows".to_owned())
            }
        }
        other => Err(format!("{other} has no window-list command")),
    }
}

pub(crate) fn specs_for(caps: &PlatformCaps) -> Vec<ToolSpecWire> {
    let mut out = vec![
        spec(
            "app.capabilities",
            "Platform session, available actions, and reasons when an action is missing",
            json!({"type":"object","additionalProperties":false}),
            Vec::new(),
        ),
        spec(
            "app.screenshot",
            "Capture a display as PNG (portal on Wayland, CLI fallback, GDI on Windows)",
            json!({"type":"object","additionalProperties":false}),
            Vec::new(),
        ),
        spec(
            "app.list_monitors",
            "List monitors with pixel size and scale",
            json!({"type":"object","additionalProperties":false}),
            Vec::new(),
        ),
        spec(
            "app.window_list",
            "List visible windows when the compositor exposes them",
            json!({"type":"object","additionalProperties":false}),
            Vec::new(),
        ),
        spec(
            "app.active_window",
            "Title of the focused window",
            json!({"type":"object","additionalProperties":false}),
            Vec::new(),
        ),
        spec(
            "app.clipboard_get",
            "Read the clipboard as text (native first, CLI fallback)",
            json!({"type":"object","additionalProperties":false}),
            Vec::new(),
        ),
        spec(
            "app.clipboard_set",
            "Write text to the clipboard (native first, CLI fallback)",
            json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":false}),
            vec!["input".to_owned()],
        ),
    ];
    if caps.advertise_input() {
        out.extend([
            spec(
                "app.click",
                "Click at a screen position (X11/Windows only)",
                json!({"type":"object","properties":{"x":{"type":"integer"},"y":{"type":"integer"},"button":{"type":"integer"}},"required":["x","y"],"additionalProperties":false}),
                vec!["input".to_owned()],
            ),
            spec(
                "app.type",
                "Type text into the focused window (X11/Windows only)",
                json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":false}),
                vec!["input".to_owned()],
            ),
            spec(
                "app.key",
                "Press a key combination (X11/Windows only)",
                json!({"type":"object","properties":{"combo":{"type":"string"}},"required":["combo"],"additionalProperties":false}),
                vec!["input".to_owned()],
            ),
        ]);
    }
    out
}

pub(crate) fn execute(name: &str, args: &Value) -> Result<Value, String> {
    let env: HashMap<String, String> = std::env::vars().collect();
    let mut caps = PlatformCaps::from_pairs(std::env::vars());
    if matches!(name, "app.capabilities" | "app.window_list") {
        caps.window_list = cap_from_availability(&window_availability_for_env(&env));
    }
    match name {
        "app.capabilities" => Ok(caps.to_json()),
        "app.screenshot" => capture::screenshot(),
        "app.list_monitors" => capture::list_monitors(),
        "app.window_list" => {
            let availability = window_availability_for_env(&env);
            window_list(&caps, &availability, run_window_backend)
        }
        "app.active_window" => active_window(&caps),
        "app.clipboard_get" => clipboard::clipboard_get(),
        "app.clipboard_set" => clipboard::clipboard_set(arg_str(args, "text")?),
        "app.click" => click(&caps, args),
        "app.type" => type_text(&caps, arg_str(args, "text")?),
        "app.key" => key(&caps, arg_str(args, "combo")?),
        other => Err(format!("unknown builtin {other}")),
    }
}

fn window_list(
    caps: &PlatformCaps,
    availability: &BackendAvailability,
    run_backend: impl Fn(&Backend) -> Result<String, String>,
) -> Result<Value, String> {
    if !caps.window_list.available {
        let (code, backend, reason) = match availability {
            BackendAvailability::Missing(backend) => (
                "dependency_missing",
                backend.name,
                availability
                    .reason()
                    .unwrap_or("window-list backend is missing"),
            ),
            BackendAvailability::Unsupported(backend, reason) => {
                ("unsupported", backend.name, *reason)
            }
            BackendAvailability::Available(backend) => (
                "unavailable",
                backend.name,
                "window-list capability changed before execution",
            ),
        };
        return Err(fail(code, backend, reason));
    }
    run_window_list(availability, run_backend)
}

fn run_window_list(
    availability: &BackendAvailability,
    run_backend: impl Fn(&Backend) -> Result<String, String>,
) -> Result<Value, String> {
    match run_backend(availability.backend()) {
        Ok(text) => Ok(parse_backend_windows(availability.backend(), &text)),
        Err(err) => {
            let (code, message) = match availability {
                BackendAvailability::Missing(backend) => (
                    "dependency_missing",
                    format!("{} ({err})", backend.install_hint),
                ),
                BackendAvailability::Unsupported(_, reason) => {
                    ("unsupported", (*reason).to_owned())
                }
                BackendAvailability::Available(_) => ("backend_failed", err),
            };
            Err(fail(code, availability.backend().name, message))
        }
    }
}

fn parse_backend_windows(backend: &Backend, text: &str) -> Value {
    match backend.name {
        "wmctrl" => json!({ "windows": parse_wmctrl(text), "backend": backend.name }),
        "hyprctl" => json!({ "windows": parse_hypr_clients(text), "backend": backend.name }),
        "win32" => serde_json::from_str(text)
            .unwrap_or_else(|_| json!({ "windows": [], "backend": backend.name })),
        _ => {
            let mut windows = Vec::new();
            if let Ok(tree) = serde_json::from_str::<Value>(text) {
                collect_sway_windows(&tree, &mut windows);
            }
            json!({ "windows": windows, "backend": backend.name })
        }
    }
}

pub(crate) fn cap_from_availability(availability: &BackendAvailability) -> ActionCap {
    let backend = availability.backend();
    ActionCap {
        available: availability.available(),
        backend: backend.name,
        reason: availability.reason(),
    }
}

fn parse_wmctrl(text: &str) -> Vec<Value> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let id = parts.next()?;
            let _desk = parts.next()?;
            let host = parts.next()?;
            let title = parts.collect::<Vec<_>>().join(" ");
            Some(json!({ "id": id, "host": host, "title": title }))
        })
        .collect()
}

fn parse_hypr_clients(text: &str) -> Vec<Value> {
    let mut windows = Vec::new();
    let mut id = String::new();
    let mut title = String::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Window ") {
            if !id.is_empty() {
                windows.push(json!({ "id": id, "host": "", "title": title }));
            }
            rest.split_whitespace()
                .next()
                .unwrap_or_default()
                .clone_into(&mut id);
            title.clear();
        } else if let Some(rest) = line.strip_prefix("title:") {
            rest.trim().clone_into(&mut title);
        }
    }
    if !id.is_empty() {
        windows.push(json!({ "id": id, "host": "", "title": title }));
    }
    windows
}

fn collect_sway_windows(node: &Value, out: &mut Vec<Value>) {
    let focused_con = node.get("type").and_then(Value::as_str) == Some("con")
        || node.get("type").and_then(Value::as_str) == Some("floating_con");
    if focused_con
        && let Some(name) = node.get("name").and_then(Value::as_str)
        && !name.is_empty()
    {
        let id = node.get("id").cloned().unwrap_or(json!(null));
        out.push(json!({ "id": id, "host": "", "title": name }));
    }
    for key in ["nodes", "floating_nodes"] {
        if let Some(children) = node.get(key).and_then(Value::as_array) {
            for child in children {
                collect_sway_windows(child, out);
            }
        }
    }
}

fn active_window(caps: &PlatformCaps) -> Result<Value, String> {
    if !caps.active_window.available {
        return Err(fail(
            "unsupported",
            caps.active_window.backend,
            caps.active_window
                .reason
                .unwrap_or("active window is not available"),
        ));
    }
    if let Ok(id) = stdout_text("xdotool", &["getactivewindow"]) {
        let title = stdout_text("xdotool", &["getwindowname", id.trim()]).unwrap_or_default();
        return Ok(json!({ "id": id.trim(), "title": title.trim(), "backend": "xdotool" }));
    }
    if let Ok(text) = stdout_text("hyprctl", &["activewindow"]) {
        let title = text
            .lines()
            .find_map(|line| line.trim().strip_prefix("title:"))
            .map_or("", str::trim);
        if !title.is_empty() {
            return Ok(json!({ "id": "", "title": title, "backend": "hyprctl" }));
        }
    }
    Err(fail(
        "unavailable",
        caps.active_window.backend,
        "active window needs xdotool or hyprctl",
    ))
}

fn require_input(caps: &PlatformCaps) -> Result<(), String> {
    if caps.advertise_input() {
        Ok(())
    } else {
        Err(fail(
            "unsupported",
            caps.input.backend,
            caps.input
                .reason
                .unwrap_or("input injection is not advertised on this session"),
        ))
    }
}

fn click(caps: &PlatformCaps, args: &Value) -> Result<Value, String> {
    require_input(caps)?;
    let x = args
        .get("x")
        .and_then(Value::as_i64)
        .ok_or_else(|| "missing x".to_owned())?;
    let y = args
        .get("y")
        .and_then(Value::as_i64)
        .ok_or_else(|| "missing y".to_owned())?;
    let button = args.get("button").and_then(Value::as_i64).unwrap_or(1);
    let x = x.to_string();
    let y = y.to_string();
    let button = button.to_string();
    run("xdotool", &["mousemove", &x, &y, "click", &button])?;
    Ok(json!({ "ok": true, "backend": "xdotool" }))
}

fn type_text(caps: &PlatformCaps, text: &str) -> Result<Value, String> {
    require_input(caps)?;
    run("xdotool", &["type", "--", text])?;
    Ok(json!({ "ok": true, "backend": "xdotool" }))
}

fn key(caps: &PlatformCaps, combo: &str) -> Result<Value, String> {
    require_input(caps)?;
    run("xdotool", &["key", "--", combo])?;
    Ok(json!({ "ok": true, "backend": "xdotool" }))
}

#[cfg(test)]
mod tests {
    use super::backend::{BackendAvailability, HYPRLAND, WMCTRL};
    use super::capability::PlatformCaps;
    use super::{
        cap_from_availability, collect_sway_windows, parse_hypr_clients, parse_wmctrl,
        run_window_list, specs_for, window_list,
    };
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn parse_wmctrl_splits_id_host_title() {
        let rows = parse_wmctrl("0x123  0 host Terminal title here\n");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], "0x123");
        assert_eq!(rows[0]["title"], "Terminal title here");
    }

    #[test]
    fn parse_hypr_clients_reads_window_blocks() {
        let text =
            "Window abcd -> mapped: 1\n\ttitle: Code\nWindow efgh -> mapped: 1\n\ttitle: Browser\n";
        let rows = parse_hypr_clients(text);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["id"], "abcd");
        assert_eq!(rows[0]["title"], "Code");
        assert_eq!(rows[1]["title"], "Browser");
    }

    #[test]
    fn collect_sway_windows_walks_tree() {
        let tree = json!({
            "type": "root",
            "name": "root",
            "nodes": [{
                "type": "con",
                "id": 7,
                "name": "Firefox",
                "floating_nodes": []
            }]
        });
        let mut out = Vec::new();
        collect_sway_windows(&tree, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["title"], "Firefox");
        assert_eq!(out[0]["id"], json!(7));
    }

    #[test]
    fn runtime_window_backend_failure_is_not_a_dependency_error() {
        let error = run_window_list(&BackendAvailability::Available(HYPRLAND), |_| {
            Err("command exited with status 1".to_owned())
        })
        .unwrap_err();
        let error: serde_json::Value = serde_json::from_str(&error).unwrap();
        assert_eq!(error["code"], "backend_failed");
        assert_eq!(error["backend"], "hyprctl");
        assert!(error["message"].as_str().unwrap().contains("status 1"));
    }

    #[test]
    fn missing_window_backend_is_a_dependency_error_before_execution() {
        let availability = BackendAvailability::Missing(WMCTRL);
        let mut caps = PlatformCaps::from_env(&HashMap::new());
        caps.window_list = cap_from_availability(&availability);
        let error = window_list(&caps, &availability, |_| {
            Err("backend should not run".to_owned())
        })
        .unwrap_err();
        let error: serde_json::Value = serde_json::from_str(&error).unwrap();
        assert_eq!(error["code"], "dependency_missing");
        assert_eq!(error["backend"], "wmctrl");
        assert!(error["message"].as_str().unwrap().contains("not installed"));
    }

    #[cfg(not(windows))]
    #[test]
    fn gnome_wayland_specs_omit_input_tools() {
        let env = HashMap::from([
            ("XDG_SESSION_TYPE".to_owned(), "wayland".to_owned()),
            ("WAYLAND_DISPLAY".to_owned(), "wayland-0".to_owned()),
            ("XDG_CURRENT_DESKTOP".to_owned(), "GNOME".to_owned()),
        ]);
        let caps = PlatformCaps::from_env(&env);
        let names: Vec<_> = specs_for(&caps).into_iter().map(|s| s.name).collect();
        assert!(names.contains(&"app.capabilities".to_owned()));
        assert!(names.contains(&"app.list_monitors".to_owned()));
        assert!(!names.iter().any(|n| n == "app.click"));
    }
}
