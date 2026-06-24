//! Platform integration systems.
//!
//! Phase 6 splits the per-frame Linux display-server glue
//! out of `Runtime::about_to_wait` into the four
//! small systems in this module. Each system runs in a
//! single `AppSet` so the schedule's existing
//! `First` / `PreUpdate` / `Update` / `PostUpdate` / `Last`
//! ordering is preserved.
pub mod click_through;
pub mod cursor;
pub mod gtk_pump;
pub mod input_region;
pub mod should_render_debug;
