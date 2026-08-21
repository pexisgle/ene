use super::{arg_str, spec};
use base64::Engine;
use ene_plugin_ipc::ToolSpecWire;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const HOST_COMMAND_TIMEOUT: Duration = Duration::from_secs(3);
const SCREENSHOT_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_SCREENSHOT_BYTES: usize = 512 * 1024;

pub(super) fn specs() -> Vec<ToolSpecWire> {
    vec![
        spec(
            "app.screenshot",
            "Capture the primary display as PNG",
            json!({"type":"object","additionalProperties":false}),
            Vec::new(),
        ),
        spec(
            "app.window_list",
            "List visible windows",
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
            "Read the clipboard as text",
            json!({"type":"object","additionalProperties":false}),
            Vec::new(),
        ),
        spec(
            "app.clipboard_set",
            "Write text to the clipboard",
            json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":false}),
            vec!["input".to_owned()],
        ),
        spec(
            "app.click",
            "Click at a screen position",
            json!({"type":"object","properties":{"x":{"type":"integer"},"y":{"type":"integer"},"button":{"type":"integer"}},"required":["x","y"],"additionalProperties":false}),
            vec!["input".to_owned()],
        ),
        spec(
            "app.type",
            "Type text into the focused window",
            json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":false}),
            vec!["input".to_owned()],
        ),
        spec(
            "app.key",
            "Press a key combination (xdotool key syntax)",
            json!({"type":"object","properties":{"combo":{"type":"string"}},"required":["combo"],"additionalProperties":false}),
            vec!["input".to_owned()],
        ),
    ]
}

pub(super) fn execute(name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "app.screenshot" => screenshot(),
        "app.window_list" => window_list(),
        "app.active_window" => active_window(),
        "app.clipboard_get" => clipboard_get(),
        "app.clipboard_set" => clipboard_set(arg_str(args, "text")?),
        "app.click" => click(args),
        "app.type" => type_text(arg_str(args, "text")?),
        "app.key" => key(arg_str(args, "combo")?),
        other => Err(format!("unknown builtin {other}")),
    }
}

fn screenshot() -> Result<Value, String> {
    let png = capture_png()?;
    Ok(json!({
        "mime": "image/png",
        "png_base64": base64::engine::general_purpose::STANDARD.encode(png),
    }))
}

fn capture_png() -> Result<Vec<u8>, String> {
    let candidates: &[&[&str]] = &[
        &["grim", "-s", "0.5", "-"],
        &["grim", "-"],
        &["import", "-window", "root", "-resize", "50%", "png:-"],
        &["import", "-window", "root", "png:-"],
    ];
    let mut last_err = "no screenshot backend (need grim, ImageMagick import, gnome-screenshot, spectacle, or scrot)".to_owned();
    for args in candidates {
        match stdout_bytes_timeout(args[0], &args[1..], SCREENSHOT_TIMEOUT) {
            Ok(bytes) if looks_like_png(&bytes) => {
                return cap_png(bytes);
            }
            Ok(_) => last_err = format!("{} produced a non-PNG screenshot", args[0]),
            Err(err) => last_err = err,
        }
    }
    if let Ok(bytes) = capture_png_file() {
        return cap_png(bytes);
    }
    Err(last_err)
}

fn capture_png_file() -> Result<Vec<u8>, String> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis());
    let tmp = std::env::temp_dir().join(format!("ene-shot-{}-{stamp}.png", std::process::id()));
    let path = tmp.to_string_lossy().into_owned();
    let backends = [
        vec!["gnome-screenshot".to_owned(), "-f".to_owned(), path.clone()],
        vec![
            "spectacle".to_owned(),
            "-b".to_owned(),
            "-n".to_owned(),
            "-o".to_owned(),
            path.clone(),
        ],
        vec!["scrot".to_owned(), "-o".to_owned(), path.clone()],
    ];
    for args in backends {
        let bin = args[0].as_str();
        let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
        if run(bin, &rest).is_ok()
            && let Ok(bytes) = std::fs::read(&tmp)
            && looks_like_png(&bytes)
        {
            drop(std::fs::remove_file(&tmp));
            return Ok(bytes);
        }
        drop(std::fs::remove_file(&tmp));
    }
    Err("file screenshot backends failed".to_owned())
}

fn cap_png(bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    if bytes.len() <= MAX_SCREENSHOT_BYTES {
        return Ok(bytes);
    }
    if let Ok(shrunk) = pipe_bytes(
        "convert",
        &["png:-", "-resize", "50%", "png:-"],
        &bytes,
        SCREENSHOT_TIMEOUT,
    ) && looks_like_png(&shrunk)
        && shrunk.len() <= MAX_SCREENSHOT_BYTES
    {
        return Ok(shrunk);
    }
    if let Ok(shrunk) = pipe_bytes(
        "convert",
        &["png:-", "-resize", "25%", "png:-"],
        &bytes,
        SCREENSHOT_TIMEOUT,
    ) && looks_like_png(&shrunk)
        && shrunk.len() <= MAX_SCREENSHOT_BYTES
    {
        return Ok(shrunk);
    }
    Err("screenshot exceeded size cap after shrink".to_owned())
}

fn looks_like_png(bytes: &[u8]) -> bool {
    bytes.len() > 8 && bytes.starts_with(b"\x89PNG\r\n\x1a\n")
}

fn window_list() -> Result<Value, String> {
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
    Err("window list needs wmctrl, hyprctl, or swaymsg".to_owned())
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

fn active_window() -> Result<Value, String> {
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
    Err("active window needs xdotool or hyprctl".to_owned())
}

fn clipboard_get() -> Result<Value, String> {
    if let Ok(text) = stdout_text("wl-paste", &[]) {
        return Ok(json!({ "text": text }));
    }
    if let Ok(text) = stdout_text("xclip", &["-selection", "clipboard", "-o"]) {
        return Ok(json!({ "text": text }));
    }
    Err("clipboard get needs wl-paste or xclip".to_owned())
}

fn clipboard_set(text: &str) -> Result<Value, String> {
    if stdin_text("wl-copy", &[], text).is_ok() {
        return Ok(json!({ "ok": true }));
    }
    if stdin_text("xclip", &["-selection", "clipboard"], text).is_ok() {
        return Ok(json!({ "ok": true }));
    }
    Err("clipboard set needs wl-copy or xclip".to_owned())
}

fn click(args: &Value) -> Result<Value, String> {
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
    Ok(json!({ "ok": true }))
}

fn type_text(text: &str) -> Result<Value, String> {
    run("xdotool", &["type", "--", text])?;
    Ok(json!({ "ok": true }))
}

fn key(combo: &str) -> Result<Value, String> {
    run("xdotool", &["key", "--", combo])?;
    Ok(json!({ "ok": true }))
}

fn stdout_text(bin: &str, args: &[&str]) -> Result<String, String> {
    let bytes = stdout_bytes_timeout(bin, args, HOST_COMMAND_TIMEOUT)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn stdout_bytes_timeout(bin: &str, args: &[&str], timeout: Duration) -> Result<Vec<u8>, String> {
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| err.to_string())?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("{bin} has no stdout"))?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        stdout.read_to_end(&mut buf).map(|_| buf)
    });
    let waited = wait_child(&mut child, bin, timeout);
    let buf = reader
        .join()
        .map_err(|_| format!("{bin} stdout reader panicked"))?
        .map_err(|err| err.to_string())?;
    waited?;
    Ok(buf)
}

fn pipe_bytes(
    bin: &str,
    args: &[&str],
    input: &[u8],
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| err.to_string())?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input).map_err(|err| err.to_string())?;
    }
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("{bin} has no stdout"))?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        stdout.read_to_end(&mut buf).map(|_| buf)
    });
    let waited = wait_child(&mut child, bin, timeout);
    let buf = reader
        .join()
        .map_err(|_| format!("{bin} stdout reader panicked"))?
        .map_err(|err| err.to_string())?;
    waited?;
    Ok(buf)
}

fn stdin_text(bin: &str, args: &[&str], text: &str) -> Result<(), String> {
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| err.to_string())?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(text.as_bytes())
            .map_err(|err| err.to_string())?;
    }
    wait_child(&mut child, bin, HOST_COMMAND_TIMEOUT)
}

fn run(bin: &str, args: &[&str]) -> Result<(), String> {
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| err.to_string())?;
    wait_child(&mut child, bin, HOST_COMMAND_TIMEOUT)
}

fn wait_child(child: &mut std::process::Child, bin: &str, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => return Err(format!("{bin} failed")),
            Ok(None) if Instant::now() >= deadline => {
                drop(child.kill());
                drop(child.wait());
                return Err(format!("{bin} timed out"));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(err) => return Err(err.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{collect_sway_windows, parse_hypr_clients, parse_wmctrl};
    use serde_json::json;

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
}
