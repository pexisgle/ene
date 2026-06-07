pub(crate) mod compositor;

#[cfg(target_os = "linux")]
pub fn detect_wayland() -> bool {
    std::env::var("XDG_SESSION_TYPE").is_ok_and(|s| s == "wayland")
        || std::env::var("WAYLAND_DISPLAY").is_ok()
}

#[cfg(not(target_os = "linux"))]
pub fn detect_wayland() -> bool {
    false
}
