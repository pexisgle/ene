//! Host <-> Client wire-neutral DTOs (remote-capable only).
//!
//! Serde shapes with no Ene-internal dependencies: every reference crossing
//! this boundary is an opaque [`String`] or a minted [`uuid::Uuid`], never a
//! Host domain newtype, secret, or durable row. Payload framing and transport
//! arrive in Stage 2; this crate fixes module layout and field shapes only.
//!
//! The five modules below are the whole Stage 1 contract: [`envelope`] routes
//! (compatibility, correlation, sender marks) while [`handshake`],
//! [`round`], [`presence`], and [`management`] carry typed domain payloads.
//! Unknown fields are tolerated on deserialization (never
//! `deny_unknown_fields`); unknown enum variants and unknown
//! `message_type` values are rejected by the Host, never guessed.

/// Routing-only envelope: compatibility, correlation, and sender marks.
pub mod envelope;
/// Pairing, authentication, capability advertisement, and reconnect shapes.
pub mod handshake;
/// Setup intents and Host-filtered management views.
pub mod management;
/// Host-to-Client presence facts.
pub mod presence;
/// Text rounds, streams, presentation confirmations, and history views.
pub mod round;
