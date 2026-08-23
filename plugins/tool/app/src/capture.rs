use super::capability::{PlatformCaps, fail};
use super::hostcmd::{pipe_bytes, run, stdout_bytes_timeout, stdout_text};
use base64::Engine;
use serde_json::{Value, json};
use std::time::Duration;

const SCREENSHOT_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_SCREENSHOT_BYTES: usize = 512 * 1024;
#[cfg(target_os = "linux")]
const PORTAL_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) fn screenshot() -> Result<Value, String> {
    let caps = PlatformCaps::detect();
    if !caps.screenshot.available {
        return Err(fail(
            "unsupported",
            caps.screenshot.backend,
            caps.screenshot.reason.unwrap_or("screenshot unavailable"),
        ));
    }
    let monitors = list_monitors().ok();
    #[cfg(target_os = "linux")]
    if caps.screenshot.backend == "portal" {
        match portal_png() {
            Ok(png) => return capture_json(&png, "portal", "granted", monitors.as_ref()),
            Err(err) if err.contains("\"code\":\"cancelled\"") => return Err(err),
            Err(err) if err.contains("\"code\":\"denied\"") => return Err(err),
            Err(err) if err.contains("\"code\":\"waiting\"") => return Err(err),
            Err(_) => {}
        }
    }
    #[cfg(windows)]
    if let Ok(png) = super::win32::capture_png() {
        return capture_json(&png, "gdi", "granted", monitors.as_ref());
    }
    let png = capture_png_cli()?;
    capture_json(&png, "cli", "granted", monitors.as_ref())
}

fn capture_json(
    png: &[u8],
    backend: &str,
    permission: &str,
    monitors: Option<&Value>,
) -> Result<Value, String> {
    let (width, height) = png_size(png).ok_or_else(|| {
        fail(
            "unavailable",
            backend,
            "captured bytes are not a PNG with IHDR",
        )
    })?;
    let monitor = matching_monitor(width, height, monitors);
    let scale = monitor
        .as_ref()
        .and_then(|row| row.get("scale"))
        .and_then(Value::as_f64)
        .unwrap_or(1.0);
    Ok(json!({
        "mime": "image/png",
        "png_base64": base64::engine::general_purpose::STANDARD.encode(png),
        "width": width,
        "height": height,
        "scale": scale,
        "format": "png",
        "backend": backend,
        "permission": permission,
        "monitor": monitor,
    }))
}

pub(crate) fn list_monitors() -> Result<Value, String> {
    if let Ok(text) = stdout_text("hyprctl", &["monitors", "-j"])
        && let Ok(parsed) = parse_hypr_monitors(&text)
        && !parsed.is_empty()
    {
        return Ok(json!({ "monitors": parsed, "backend": "hyprctl" }));
    }
    if let Ok(text) = stdout_text("swaymsg", &["-t", "get_outputs"])
        && let Ok(parsed) = parse_sway_outputs(&text)
        && !parsed.is_empty()
    {
        return Ok(json!({ "monitors": parsed, "backend": "swaymsg" }));
    }
    if let Ok(text) = stdout_text("xrandr", &["--query"]) {
        let parsed = parse_xrandr(&text);
        if !parsed.is_empty() {
            return Ok(json!({ "monitors": parsed, "backend": "xrandr" }));
        }
    }
    #[cfg(windows)]
    if let Ok(parsed) = super::win32::list_monitors() {
        return Ok(json!({ "monitors": parsed, "backend": "gdi" }));
    }
    Err(fail(
        "unavailable",
        "none",
        "no monitor backend (hyprctl, swaymsg, xrandr)",
    ))
}

fn matching_monitor(width: u32, height: u32, monitors: Option<&Value>) -> Option<Value> {
    let rows = monitors?.get("monitors")?.as_array()?;
    rows.iter()
        .find(|row| {
            row.get("width").and_then(Value::as_u64) == Some(u64::from(width))
                && row.get("height").and_then(Value::as_u64) == Some(u64::from(height))
        })
        .cloned()
        .or_else(|| {
            rows.iter()
                .find(|row| row.get("primary").and_then(Value::as_bool) == Some(true))
                .cloned()
        })
        .or_else(|| rows.first().cloned())
}

pub(crate) fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if !looks_like_png(bytes) || bytes.len() < 24 {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((width, height))
}

fn looks_like_png(bytes: &[u8]) -> bool {
    bytes.len() > 8 && bytes.starts_with(b"\x89PNG\r\n\x1a\n")
}

#[cfg(target_os = "linux")]
fn portal_png() -> Result<Vec<u8>, String> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("ene-app-portal".to_owned())
        .spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|err| fail("unavailable", "portal", err.to_string()))
                .and_then(|rt| {
                    rt.block_on(async {
                        tokio::time::timeout(PORTAL_TIMEOUT, portal_png_async()).await
                    })
                    .map_err(|_| {
                        fail(
                            "waiting",
                            "portal",
                            "portal screenshot did not finish before the wait budget",
                        )
                    })
                    .and_then(std::convert::identity)
                });
            drop(tx.send(result));
        })
        .map_err(|err| fail("unavailable", "portal", err.to_string()))?;
    rx.recv()
        .unwrap_or_else(|_| Err(fail("unavailable", "portal", "portal worker exited")))
        .and_then(|bytes| cap_png(bytes, "portal"))
}

#[cfg(target_os = "linux")]
fn map_portal(err: &ashpd::Error) -> String {
    let text = err.to_string();
    let lower = text.to_ascii_lowercase();
    if lower.contains("cancel") {
        fail("cancelled", "portal", "user cancelled the portal prompt")
    } else if lower.contains("not found") || lower.contains("does not exist") {
        fail(
            "unsupported",
            "portal",
            "no screenshot portal on this session",
        )
    } else if lower.contains("denied") || lower.contains("not allowed") {
        fail("denied", "portal", text)
    } else {
        fail("unavailable", "portal", text)
    }
}

#[cfg(target_os = "linux")]
async fn portal_png_async() -> Result<Vec<u8>, String> {
    let request = ashpd::desktop::screenshot::Screenshot::request()
        .interactive(false)
        .send()
        .await
        .map_err(|err| map_portal(&err))?;
    let picture = request.response().map_err(|err| map_portal(&err))?;
    let uri = picture.uri().to_string();
    let path = uri.strip_prefix("file://").ok_or_else(|| {
        fail(
            "unavailable",
            "portal",
            format!("unexpected portal uri {uri}"),
        )
    })?;
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|err| fail("unavailable", "portal", err.to_string()))?;
    Ok(bytes)
}

fn capture_png_cli() -> Result<Vec<u8>, String> {
    let candidates: &[&[&str]] = &[
        &["grim", "-s", "0.5", "-"],
        &["grim", "-"],
        &["import", "-window", "root", "-resize", "50%", "png:-"],
        &["import", "-window", "root", "png:-"],
    ];
    let mut last_err = fail(
        "unavailable",
        "cli",
        "no screenshot backend (grim or ImageMagick import)",
    );
    for args in candidates {
        match stdout_bytes_timeout(args[0], &args[1..], SCREENSHOT_TIMEOUT) {
            Ok(bytes) if looks_like_png(&bytes) => return cap_png(bytes, "cli"),
            Ok(_) => {
                last_err = fail(
                    "unavailable",
                    "cli",
                    format!("{} produced a non-PNG screenshot", args[0]),
                );
            }
            Err(err) => {
                last_err = fail("unavailable", "cli", err);
            }
        }
    }
    if let Ok(bytes) = capture_png_file() {
        return cap_png(bytes, "cli");
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
    Err(fail(
        "unavailable",
        "cli",
        "file screenshot backends failed",
    ))
}

fn cap_png(bytes: Vec<u8>, backend: &str) -> Result<Vec<u8>, String> {
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
    Err(fail(
        "unavailable",
        backend,
        "screenshot exceeded size cap after shrink",
    ))
}

pub(crate) fn parse_hypr_monitors(text: &str) -> Result<Vec<Value>, String> {
    let value: Value = serde_json::from_str(text)
        .map_err(|err| fail("unavailable", "hyprctl", err.to_string()))?;
    let Some(rows) = value.as_array() else {
        return Ok(Vec::new());
    };
    Ok(rows
        .iter()
        .filter_map(|row| {
            Some(json!({
                "id": row.get("name")?,
                "width": row.get("width")?,
                "height": row.get("height")?,
                "scale": row.get("scale").cloned().unwrap_or(json!(1.0)),
                "primary": row.get("focused").and_then(Value::as_bool).unwrap_or(false),
            }))
        })
        .collect())
}

pub(crate) fn parse_sway_outputs(text: &str) -> Result<Vec<Value>, String> {
    let value: Value = serde_json::from_str(text)
        .map_err(|err| fail("unavailable", "swaymsg", err.to_string()))?;
    let Some(rows) = value.as_array() else {
        return Ok(Vec::new());
    };
    Ok(rows
        .iter()
        .filter_map(|row| {
            let rect = row.get("rect")?;
            Some(json!({
                "id": row.get("name")?,
                "width": rect.get("width")?,
                "height": rect.get("height")?,
                "scale": row.get("scale").cloned().unwrap_or(json!(1.0)),
                "primary": row.get("primary").and_then(Value::as_bool).unwrap_or(false),
            }))
        })
        .collect())
}

pub(crate) fn parse_xrandr(text: &str) -> Vec<Value> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let id = parts.next()?;
            if parts.next()? != "connected" {
                return None;
            }
            let mut primary = false;
            let mut geom = parts.next()?;
            if geom == "primary" {
                primary = true;
                geom = parts.next()?;
            }
            let (wh, _) = geom.split_once('+')?;
            let (w, h) = wh.split_once('x')?;
            Some(json!({
                "id": id,
                "width": w.parse::<u32>().ok()?,
                "height": h.parse::<u32>().ok()?,
                "scale": 1.0,
                "primary": primary,
            }))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{matching_monitor, parse_hypr_monitors, parse_xrandr, png_size};
    use serde_json::json;

    const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn png_ihdr_reads_pixel_size() {
        assert_eq!(png_size(TINY_PNG), Some((1, 1)));
    }

    #[test]
    fn xrandr_and_hypr_layout_match_capture_size() {
        let monitors = json!({
            "monitors": parse_xrandr("HDMI-1 connected primary 1920x1080+0+0\n")
        });
        let matched = matching_monitor(1920, 1080, Some(&monitors)).unwrap();
        assert_eq!(matched["id"], "HDMI-1");
        assert_eq!(matched["scale"], 1.0);
        let hypr = parse_hypr_monitors(
            r#"[{"name":"eDP-1","width":1920,"height":1080,"scale":1.5,"focused":true}]"#,
        )
        .unwrap();
        assert_eq!(hypr[0]["scale"], 1.5);
        let wrapped = json!({ "monitors": hypr });
        let shot = matching_monitor(1920, 1080, Some(&wrapped)).unwrap();
        assert_eq!(shot["scale"], 1.5);
    }

    #[test]
    fn portal_cancelled_payload_is_structured() {
        let err = super::fail("cancelled", "portal", "user cancelled the portal prompt");
        let value: serde_json::Value = serde_json::from_str(&err).unwrap();
        assert_eq!(value["code"], "cancelled");
        assert_eq!(value["backend"], "portal");
    }
}
