use ene_tool_proto::ToolError;

use crate::utils::portal::compositor::dispatch;

pub(super) fn list_windows_wayland() -> Result<String, ToolError> {
    let compositor = dispatch().ok_or_else(|| ToolError::execution_failed("Window listing not supported on this Wayland compositor. Supported: Hyprland, Sway, GNOME, KDE."
                .to_string()))?;
    compositor.list_windows()
}
