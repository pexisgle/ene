pub(crate) mod compositor;

#[cfg(target_os = "linux")]
pub fn detect_wayland() -> bool {
    std::env::var("XDG_SESSION_TYPE")
        .map(|s| s == "wayland")
        .unwrap_or(false)
        || std::env::var("WAYLAND_DISPLAY").is_ok()
}

#[cfg(not(target_os = "linux"))]
pub fn detect_wayland() -> bool {
    false
}
