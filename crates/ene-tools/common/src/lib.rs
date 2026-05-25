//! # ene-tools-common
//!
//! Shared utilities used by all tool provider crates.
//!
//! ## Modules
//!
//! - [`html`] — HTML-to-Markdown conversion and content extraction ((scraper-based)
//! - [`truncate`] — Smart content truncation helpers (by chars, lines, and tail)
//!
//! ## Tool Design Philosophy
//!
//! Currently the `ene-tools` family uses two architectural patterns:
//!
//! 1. **Mega-tool approach** (fs, app, browser): A single binary per domain with multiple actions
//!    dispatched internally. Minimizes process overhead and IPC round-trips.
//! 2. **Individual-tool approach** (web, utility): Multiple smaller tools, each with a focused responsibility.
//!    Improves semantic matching precision in Tool RAG.
//!
//! A future unification to a single approach is possible but not yet decided.
//! When designing new tools, consider the trade-offs between startup overhead and
//! retrieval precision for your specific use case.
#![warn(missing_docs)]

/// HTML-to-Markdown conversion and content extraction.
pub mod html;
/// Smart content truncation (by chars, lines, and tail).
pub mod truncate;
