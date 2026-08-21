#![cfg_attr(test, expect(clippy::expect_used, reason = "tests"))]
#![cfg_attr(test, expect(clippy::unwrap_used, reason = "tests"))]

//! Product stage client library.

pub mod app;
pub mod audio;
pub mod avatar;
pub mod bundle;
pub mod chrome;
pub mod core;
pub mod detail;
pub mod fonts;
pub mod gpu;
pub mod i18n;
pub mod overlay;
pub mod platform;
pub mod settings;
pub mod shell;
pub mod surface;
pub mod tasks;
