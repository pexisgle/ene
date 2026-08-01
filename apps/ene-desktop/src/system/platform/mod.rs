//! Platform integration systems.
//!
//! The per-frame Linux display-server glue lives in the small
//! systems in this module. Each system runs in a single `AppSet`
//! so the schedule's existing `First` / `PreUpdate` / `Update` /
//! `PostUpdate` / `Last` ordering is preserved.
pub mod click_through;
pub mod cursor;
pub mod gtk_pump;
pub mod should_render_debug;
