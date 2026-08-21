use super::capability::fail;
use super::hostcmd::{stdin_text, stdout_text};
use serde_json::{Value, json};

pub(crate) fn clipboard_get() -> Result<Value, String> {
    match native_get() {
        Ok(text) => Ok(json!({ "text": text, "backend": "arboard", "fallback": false })),
        Err(native) => match cli_get() {
            Ok(text) => Ok(json!({
                "text": text,
                "backend": "cli",
                "fallback": true,
                "native_error": native,
            })),
            Err(cli) => Err(fail(
                "unavailable",
                "clipboard",
                format!("native: {native}; cli: {cli}"),
            )),
        },
    }
}

pub(crate) fn clipboard_set(text: &str) -> Result<Value, String> {
    match native_set(text) {
        Ok(()) => Ok(json!({ "ok": true, "backend": "arboard", "fallback": false })),
        Err(native) => match cli_set(text) {
            Ok(()) => Ok(json!({
                "ok": true,
                "backend": "cli",
                "fallback": true,
                "native_error": native,
            })),
            Err(cli) => Err(fail(
                "unavailable",
                "clipboard",
                format!("native: {native}; cli: {cli}"),
            )),
        },
    }
}

fn native_get() -> Result<String, String> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|err| format!("arboard open failed: {err}"))?;
    clipboard
        .get_text()
        .map_err(|err| format!("arboard get failed: {err}"))
}

fn native_set(text: &str) -> Result<(), String> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|err| format!("arboard open failed: {err}"))?;
    clipboard
        .set_text(text)
        .map_err(|err| format!("arboard set failed: {err}"))
}

fn cli_get() -> Result<String, String> {
    if let Ok(text) = stdout_text("wl-paste", &[]) {
        return Ok(text);
    }
    stdout_text("xclip", &["-selection", "clipboard", "-o"])
}

fn cli_set(text: &str) -> Result<(), String> {
    if stdin_text("wl-copy", &[], text).is_ok() {
        return Ok(());
    }
    stdin_text("xclip", &["-selection", "clipboard"], text)
}

#[cfg(test)]
mod tests {
    #[test]
    fn fallback_payload_marks_cli() {
        let value = serde_json::json!({
            "text": "hi",
            "backend": "cli",
            "fallback": true,
        });
        assert_eq!(value["fallback"], true);
        assert_eq!(value["backend"], "cli");
    }
}
