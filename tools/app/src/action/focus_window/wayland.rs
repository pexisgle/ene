use ene_tool_proto::ToolError;

use crate::utils::portal::compositor::dispatch;

pub(super) fn focus_window_wayland(title: &str) -> Result<String, ToolError> {
    let compositor = dispatch().ok_or_else(|| ToolError::execution_failed("Window focusing not supported on this Wayland compositor. Supported: Hyprland, Sway, GNOME, KDE."
                .to_string()))?;
    compositor.focus_window(title)
}
