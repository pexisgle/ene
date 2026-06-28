#![warn(missing_docs)]

//! # ene-cognition
//!
//! Cognitive runtime for the ene AI companion — memory extraction, recall planning,
//! emotion engine, context management, and prompt composition.
//!
//! ## Architecture
//!
//! The crate implements the [Ene Cognitive Runtime](../../docs/architecture/cognitive-runtime.md)
//! architecture, treating the LLM as an utterance generator from explicitly managed
//! cognitive state rather than as the entity that implicitly holds personality and memory.
//!
//! ## Crate Boundaries
//!
//! - Depends on: `ene-memory`, `ene-config`, `ene-provider`, `ene-common`
//! - Does NOT depend on: `ene-core`, `ene-session` (prevents circular dependencies)
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use ene_cognition::CognitionEngine;
//!
//! let engine = CognitionEngine::new();
//! ```

/// Character processor: Identity Kernel compilation and lorebook indexing.
pub mod character;
/// Companion Commitment Ledger: promise, task, and follow-up tracking.
pub mod commitments;
/// Context budget management and rolling compression.
pub mod context;
/// Emotion Engine: deterministic + optional LLM affect computation.
pub mod emotion;
/// Deterministic/LLM memory extraction and Memory Arbiter.
pub mod memory_writer;
/// Output arbitration: expression validation and hysteresis management.
pub mod output;
/// Pre-turn input analysis and turn intent classification.
pub mod pre_turn;
/// Sectioned prompt packet composition with budget-aware assembly.
pub mod prompt_packet;
/// Memory recall planning and hybrid search orchestration.
pub mod recall;

/// Cognitive runtime configuration section.
pub mod config;
/// Central cognitive engine facade.
pub mod engine;
/// Cognitive runtime error types.
pub mod error;

/// Cognitive configuration section.
pub use config::{
    CharacterMemoryConfig, CognitionConfig, CognitionMemoryConfig, ContextConfig, EmotionConfig,
};
/// Central cognitive engine facade.
pub use engine::CognitionEngine;
/// Cognitive runtime error type.
pub use error::{CognitionError, EneCognitionError};
