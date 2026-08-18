use super::{arg_str, spec};
use base64::Engine;
use ene_plugin_ipc::ToolSpecWire;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const HOST_COMMAND_TIMEOUT: Duration = Duration::from_secs(3);

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
    if let Ok(bytes) = stdout_bytes("grim", &["-"])
        && looks_like_png(&bytes)
    {
        return Ok(bytes);
    }
    if let Ok(bytes) = stdout_bytes("import", &["-window", "root", "png:-"])
        && looks_like_png(&bytes)
    {
        return Ok(bytes);
    }
    Err("no screenshot backend (need grim or ImageMagick import)".to_owned())
}

fn looks_like_png(bytes: &[u8]) -> bool {
    bytes.len() > 8 && bytes.starts_with(b"\x89PNG\r\n\x1a\n")
}

fn window_list() -> Result<Value, String> {
    if let Ok(text) = stdout_text("wmctrl", &["-l"]) {
        let windows: Vec<Value> = text
            .lines()
            .filter_map(|line| {
                let mut parts = line.splitn(4, ' ');
                let id = parts.next()?;
                let _desk = parts.next()?;
                let host = parts.next()?;
                let title = parts.next().unwrap_or("").trim();
                Some(json!({ "id": id, "host": host, "title": title }))
            })
            .collect();
        return Ok(json!({ "windows": windows }));
    }
    Err("window list needs wmctrl".to_owned())
}

fn active_window() -> Result<Value, String> {
    if let Ok(id) = stdout_text("xdotool", &["getactivewindow"]) {
        let title = stdout_text("xdotool", &["getwindowname", id.trim()]).unwrap_or_default();
        return Ok(json!({ "id": id.trim(), "title": title.trim() }));
    }
    Err("active window needs xdotool".to_owned())
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
    let bytes = stdout_bytes(bin, args)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn stdout_bytes(bin: &str, args: &[&str]) -> Result<Vec<u8>, String> {
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
    let waited = wait_child(&mut child, bin);
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
    wait_child(&mut child, bin)
}

fn run(bin: &str, args: &[&str]) -> Result<(), String> {
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| err.to_string())?;
    wait_child(&mut child, bin)
}

fn wait_child(child: &mut std::process::Child, bin: &str) -> Result<(), String> {
    let deadline = Instant::now() + HOST_COMMAND_TIMEOUT;
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
