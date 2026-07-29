pub mod compositor;

#[cfg(target_os = "linux")]
pub fn detect_wayland() -> bool {
    use std::sync::OnceLock;
    static IS_WAYLAND: OnceLock<bool> = OnceLock::new();
    *IS_WAYLAND.get_or_init(|| {
        std::env::var("XDG_SESSION_TYPE").is_ok_and(|s| s == "wayland")
            || std::env::var("WAYLAND_DISPLAY").is_ok()
    })
}

#[cfg(not(target_os = "linux"))]
pub fn detect_wayland() -> bool {
    false
}
