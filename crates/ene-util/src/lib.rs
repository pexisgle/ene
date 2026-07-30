//! # ene-util
//!
//! Pure utility functions with feature-gated heavy dependencies.
//!
//! This crate is the home for small, side-effect-free helpers whose
//! dependency trees are independent of each other. Each helper lives
//! behind a Cargo feature so that consumers only pay for what they use.
//!
//! ## Crate discipline
//!
//! Only **pure functions** belong here: no I/O, no business logic, no
//! state. If a helper needs database access, network calls, or domain
//! knowledge, it belongs in the appropriate domain crate instead. This
//! discipline prevents `ene-util` from becoming a "junk drawer" crate
//! that accumulates unrelated code (the fate of the former `ene-common`).
//!
//! ## Features
//!
//! - `truncate` (default) — Smart string truncation helpers ([`Truncate`]).
//! - `html` — HTML-to-Markdown conversion and content extraction
//!   (pulls in `htmd`, `scraper`, `ego-tree`, `regex`).
#![warn(missing_docs)]

#[cfg(feature = "truncate")]
/// Smart content truncation helpers (by chars, lines, and tail).
pub mod truncate;

#[cfg(feature = "truncate")]
#[doc(no_inline)]
pub use truncate::{Truncate, TruncateResult};

#[cfg(feature = "html")]
/// HTML-to-Markdown conversion and content extraction.
pub mod html;
