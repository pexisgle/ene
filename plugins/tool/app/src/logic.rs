use ene_plugin_ipc::ToolSpecWire;
use ene_registry::{arg_str, spec};
use serde_json::{Value, json};

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

use capability::{PlatformCaps, fail};
use hostcmd::{run, stdout_text};

pub(crate) fn specs() -> Vec<ToolSpecWire> {
    specs_for(&PlatformCaps::detect())
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
    if caps.screenshot.available {
        out.insert(
            1,
            spec(
                "app.screenshot",
                "Capture a display as PNG (portal on Wayland, CLI fallback, GDI on Windows)",
                json!({"type":"object","additionalProperties":false}),
                Vec::new(),
            ),
        );
    }
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
    let caps = PlatformCaps::detect();
    match name {
        "app.capabilities" => Ok(caps.to_json()),
        "app.screenshot" => capture::screenshot(),
        "app.list_monitors" => capture::list_monitors(),
        "app.window_list" => window_list(&caps),
        "app.active_window" => active_window(&caps),
        "app.clipboard_get" => clipboard::clipboard_get(),
        "app.clipboard_set" => clipboard::clipboard_set(arg_str(args, "text")?),
        "app.click" => click(&caps, args),
        "app.type" => type_text(&caps, arg_str(args, "text")?),
        "app.key" => key(&caps, arg_str(args, "combo")?),
        other => Err(format!("unknown builtin {other}")),
    }
}

fn window_list(caps: &PlatformCaps) -> Result<Value, String> {
    if !caps.window_list.available {
        return Err(fail(
            "unsupported",
            caps.window_list.backend,
            caps.window_list
                .reason
                .unwrap_or("window list is not available"),
        ));
    }
    if let Ok(text) = stdout_text("wmctrl", &["-l"]) {
        return Ok(json!({ "windows": parse_wmctrl(&text), "backend": "wmctrl" }));
    }
    if let Ok(text) = stdout_text("hyprctl", &["clients"]) {
        let windows = parse_hypr_clients(&text);
        if !windows.is_empty() {
            return Ok(json!({ "windows": windows, "backend": "hyprctl" }));
        }
    }
    if let Ok(text) = stdout_text("swaymsg", &["-t", "get_tree"])
        && let Ok(tree) = serde_json::from_str::<Value>(&text)
    {
        let mut windows = Vec::new();
        collect_sway_windows(&tree, &mut windows);
        if !windows.is_empty() {
            return Ok(json!({ "windows": windows, "backend": "swaymsg" }));
        }
    }
    Err(fail(
        "unavailable",
        caps.window_list.backend,
        "window list needs wmctrl, hyprctl, or swaymsg",
    ))
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
    use super::capability::PlatformCaps;
    use super::{collect_sway_windows, parse_hypr_clients, parse_wmctrl, specs_for};
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

    #[test]
    fn unavailable_screenshot_is_not_advertised() {
        let env = HashMap::from([
            ("XDG_SESSION_TYPE".to_owned(), "x11".to_owned()),
            ("DISPLAY".to_owned(), ":1".to_owned()),
            ("PATH".to_owned(), "/definitely/missing".to_owned()),
        ]);
        let caps = PlatformCaps::from_env(&env);
        let names: Vec<_> = specs_for(&caps).into_iter().map(|s| s.name).collect();

        assert!(!names.iter().any(|name| name == "app.screenshot"));
        assert!(names.iter().any(|name| name == "app.capabilities"));
    }
}
