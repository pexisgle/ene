#![cfg_attr(test, expect(clippy::expect_used, reason = "tests"))]

//! Product stage client library.

pub mod app;
pub mod audio;
pub mod avatar;
pub mod core;
pub mod detail;
pub mod i18n;
pub mod platform;
pub mod settings;
pub mod shell;
pub mod surface;
