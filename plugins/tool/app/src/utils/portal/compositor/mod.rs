#[cfg(target_os = "linux")]
mod gnome;
#[cfg(target_os = "linux")]
mod hyprland;
#[cfg(target_os = "linux")]
mod kde;
#[cfg(target_os = "linux")]
mod sway;

use ene_tool_proto::ToolError;

// ==================== Compositor trait ====================

#[cfg_attr(
    not(target_os = "linux"),
    expect(
        dead_code,
        reason = "Linux-only compositor dispatch is stubbed on other platforms"
    )
)]
pub trait WlCompositor {
    fn list_windows(&self) -> Result<String, ToolError>;
    fn focus_window(&self, title: &str) -> Result<String, ToolError>;
}

// ==================== Detection ====================

#[cfg_attr(
    not(target_os = "linux"),
    expect(
        dead_code,
        reason = "Linux-only compositor dispatch is stubbed on other platforms"
    )
)]
pub fn dispatch() -> Option<Box<dyn WlCompositor>> {
    #[cfg(target_os = "linux")]
    {
        if std::env::var("HYPRLAND_INSTANCE_SIGNATURE").is_ok() {
            return Some(Box::new(hyprland::Hyprland));
        }
        if std::env::var("SWAYSOCK").is_ok() {
            return Some(Box::new(sway::Sway));
        }
        if let Ok(desktop) = std::env::var("XDG_CURRENT_DESKTOP") {
            let d = desktop.to_lowercase();
            if d.contains("gnome") {
                return Some(Box::new(gnome::Gnome));
            }
            if d.contains("kde") || d.contains("plasma") {
                return Some(Box::new(kde::Kde));
            }
        }
        if std::env::var("KDE_FULL_SESSION").is_ok() {
            return Some(Box::new(kde::Kde));
        }
    }
    None
}

// ==================== Shared helpers (gdbus) ====================

#[cfg(target_os = "linux")]
fn gvariant_string(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Produces a safely-escaped JavaScript string literal (including surrounding
/// double quotes) suitable for embedding in JS source passed to GNOME Shell or
/// `KWin` scripting engines. Uses JSON escaping which is a valid JS string
/// literal and handles all special characters (backslash, quotes, newlines,
/// control characters, `</script>` sequences, etc.).
#[cfg(target_os = "linux")]
fn js_string_literal(s: &str) -> String {
    // serde_json::to_string produces a JSON string with proper escaping.
    // JSON string literals are valid JavaScript string literals.
    // Fallback: manual escaping if serialization fails (should never happen
    // for &str, but we avoid unwrap in production).
    serde_json::to_string(s).unwrap_or_else(|_| {
        let mut escaped = String::with_capacity(s.len() + 2);
        escaped.push('"');
        for c in s.chars() {
            match c {
                '\\' => escaped.push_str("\\\\"),
                '"' => escaped.push_str("\\\""),
                '\n' => escaped.push_str("\\n"),
                '\r' => escaped.push_str("\\r"),
                '\t' => escaped.push_str("\\t"),
                '\u{2028}' => escaped.push_str("\\u2028"),
                '\u{2029}' => escaped.push_str("\\u2029"),
                c if c.is_control() => {
                    use std::fmt::Write;
                    let _ = write!(escaped, "\\u{:04x}", c as u32);
                }
                c => escaped.push(c),
            }
        }
        escaped.push('"');
        escaped
    })
}

#[cfg(target_os = "linux")]
fn unescape_gvariant(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => result.push('\n'),
                Some('t') => result.push('\t'),
                Some('r') => result.push('\r'),
                Some('\\') | None => result.push('\\'),
                Some('\'') => result.push('\''),
                Some('"') => result.push('"'),
                Some(c2) => {
                    result.push('\\');
                    result.push(c2);
                }
            }
        } else {
            result.push(c);
        }
    }
    result
}

#[cfg(target_os = "linux")]
fn parse_gdbus_tuple_string(output: &str) -> Option<String> {
    let output = output.trim();
    let inner = output.strip_prefix('(')?.strip_suffix(')')?;
    let comma = inner.find(", '")?;
    if !inner[..comma].starts_with("true") {
        return None;
    }
    let content = &inner[comma + 3..];
    let content = content.strip_suffix('\'')?;
    Some(unescape_gvariant(content))
}
