mod gnome;
mod hyprland;
mod kde;
mod sway;

use ene_tool_proto::ToolError;

// ==================== Compositor trait ====================

trait WlCompositor {
    fn list_windows(&self) -> Result<String, ToolError>;
    fn focus_window(&self, title: &str) -> Result<String, ToolError>;
}

// ==================== Detection ====================

fn detect() -> Option<Box<dyn WlCompositor>> {
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
    None
}

// ==================== Public dispatchers ====================

#[cfg(target_os = "linux")]
pub fn list_windows_wayland() -> Result<String, ToolError> {
    let compositor = detect().ok_or_else(|| ToolError::ExecutionFailed {
        message:
            "Window listing not supported on this Wayland compositor. Supported: Hyprland, Sway, GNOME, KDE."
                .to_string(),
    })?;
    compositor.list_windows()
}

#[cfg(not(target_os = "linux"))]
pub fn list_windows_wayland() -> Result<String, ToolError> {
    Err(ToolError::ExecutionFailed {
        message: "Wayland not available on this platform".to_string(),
    })
}

#[cfg(target_os = "linux")]
pub fn focus_window_wayland(title: &str) -> Result<String, ToolError> {
    let compositor = detect().ok_or_else(|| ToolError::ExecutionFailed {
        message:
            "Window focusing not supported on this Wayland compositor. Supported: Hyprland, Sway, GNOME, KDE."
                .to_string(),
    })?;
    compositor.focus_window(title)
}

#[cfg(not(target_os = "linux"))]
pub fn focus_window_wayland(_title: &str) -> Result<String, ToolError> {
    Err(ToolError::ExecutionFailed {
        message: "Wayland not available on this platform".to_string(),
    })
}

// ==================== Shared helpers (gdbus) ====================

#[cfg(target_os = "linux")]
fn gvariant_string(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{}\"", escaped)
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
                Some('\\') => result.push('\\'),
                Some('\'') => result.push('\''),
                Some('"') => result.push('"'),
                Some(c2) => {
                    result.push('\\');
                    result.push(c2);
                }
                None => result.push('\\'),
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
