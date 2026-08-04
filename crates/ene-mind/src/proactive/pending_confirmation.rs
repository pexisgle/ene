//! Proactive confirmation of old pending memory candidates.
//!
//! Candidates the arbiter deferred with `AskConfirmationLater` are surfaced by
//! topic-near recall (`crate::recall::pending`); the ones the conversation
//! never touches stay pending forever. This module selects the overdue
//! remainder — old enough and confident enough — as a proactive confirmation
//! trigger: the decision pipeline judges the moment, generation asks a natural
//! question, and a later lightweight classification resolves the candidate
//! through the approval APIs.
//!
//! Only weak-contradiction deferrals (`approval_parked = false`) are eligible.
//! Approval-mode rows stay review-queue-only: a candidate that was never
//! approved must not surface as hearsay in the conversation just because the
//! mode was toggled off, mirroring the recall exclusion.

use chrono::{DateTime, Utc};
use ene_ai::{LlmMessage, UserMessagePart};
use ene_config::PromptLibrary;
use ene_core::{MemoryPort, PendingCandidate, PendingCandidateStatus};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::hash::BuildHasher;

use crate::config::PendingConfirmationConfig;
use crate::recall::MemoryRecallCache;

use super::truncate_chars;

/// Unconfirmed memory candidate selected for one proactive confirmation.
///
/// Carries everything the decision context, the generation hint, and the
/// reply classification need. `age_days` is computed against the injected
/// clock so tests stay deterministic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingConfirmationPrompt {
    /// `pending_candidates` row id; the resolution target.
    pub id: i64,
    /// Candidate title (short label).
    pub title: String,
    /// Full candidate content.
    pub content: String,
    /// Age of the candidate in fractional days at selection time.
    pub age_days: f64,
}

/// Pick the single oldest candidate that is due for confirmation.
///
/// A candidate is due when it is still `Pending`, is not approval-parked, is
/// visible to the session user (own rows plus character-shared `user_id = ""`
/// rows, mirroring the typed-memory visibility rule), is at least
/// `min_age_days` old, carries at least `min_confidence`, and was not asked
/// about within `reask_after_days` (per-candidate backoff so an unclear
/// reply cannot re-arm the same question on the next tick). At most one
/// candidate is returned — a tick asks about one thing at a time.
#[must_use]
pub fn select_due_pending_candidate<S: BuildHasher>(
    candidates: &[PendingCandidate],
    user_id: &str,
    config: &PendingConfirmationConfig,
    now: DateTime<Utc>,
    asked_at: &HashMap<i64, DateTime<Utc>, S>,
) -> Option<PendingConfirmationPrompt> {
    if !config.enabled {
        return None;
    }
    let due = candidates.iter().filter(|candidate| {
        candidate.status == PendingCandidateStatus::Pending
            && !candidate.approval_parked
            && (candidate.user_id.is_empty() || candidate.user_id == user_id)
            && age_days(candidate.created_at, now) >= f64::from(config.min_age_days)
            && candidate.confidence >= config.min_confidence
            && !asked_within_backoff(candidate.id, config, now, asked_at)
    });
    let oldest = due.min_by_key(|candidate| candidate.created_at)?;
    Some(PendingConfirmationPrompt {
        id: oldest.id,
        title: oldest.title.clone(),
        content: oldest.content.clone(),
        age_days: age_days(oldest.created_at, now),
    })
}

/// Load and select the due pending candidate through the L1 recall cache.
///
/// Failures to list the queue degrade to "no candidate" with a warning,
/// mirroring the recall pending gather — a proactive tick must never fail
/// because the queue was unreadable. Returns `None` when the trigger is
/// disabled.
pub async fn load_due_pending_confirmation<S: BuildHasher>(
    cache: Option<&MemoryRecallCache>,
    store: &dyn MemoryPort,
    character_id: &str,
    user_id: &str,
    config: &PendingConfirmationConfig,
    now: DateTime<Utc>,
    asked_at: &HashMap<i64, DateTime<Utc>, S>,
) -> Option<PendingConfirmationPrompt> {
    if !config.enabled {
        return None;
    }
    let listed = match cache {
        Some(cache) => cache.list_pending_candidates(store, character_id).await,
        None => {
            store
                .list_pending_candidates(character_id, Some(PendingCandidateStatus::Pending))
                .await
        }
    };
    let candidates = match listed {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(
                component = "Proactive",
                character_id = %character_id,
                error = %error,
                "Failed to list pending candidates for proactive confirmation; continuing without them"
            );
            return None;
        }
    };
    select_due_pending_candidate(&candidates, user_id, config, now, asked_at)
}

fn asked_within_backoff<S: BuildHasher>(
    candidate_id: i64,
    config: &PendingConfirmationConfig,
    now: DateTime<Utc>,
    asked_at: &HashMap<i64, DateTime<Utc>, S>,
) -> bool {
    asked_at
        .get(&candidate_id)
        .is_some_and(|asked| age_days(*asked, now) < f64::from(config.reask_after_days))
}

/// Verdict on whether the user's reply resolves the asked candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingResolutionVerdict {
    /// The reply confirms the candidate; it may be persisted.
    Approved,
    /// The reply contradicts or disowns the candidate; it is discarded.
    Rejected,
    /// The reply is unrelated, ambiguous, or the classification failed.
    Unclear,
}

/// Plain JSON Schema for the reply-classification structured output.
#[must_use]
pub fn resolution_schema_object() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["verdict"],
        "properties": {
            "verdict": { "type": "string", "enum": ["approved", "rejected", "unclear"] }
        }
    })
}

/// Build the classification messages for a user reply to a confirmation.
#[must_use]
pub fn build_resolution_messages(
    candidate: &PendingConfirmationPrompt,
    reply: &str,
    prompt_language: &str,
) -> Vec<LlmMessage> {
    let prompts = PromptLibrary::load(prompt_language);
    let system = LlmMessage::System {
        content: prompts
            .proactive()
            .pending_resolution_system
            .trim()
            .to_string(),
    };
    let user = LlmMessage::User {
        parts: vec![UserMessagePart::Text {
            text: format_resolution_context(candidate, reply),
        }],
    };
    vec![system, user]
}

/// Serialize the classification input as a single JSON document so the
/// candidate and reply stay escaped values, never control fields.
fn format_resolution_context(candidate: &PendingConfirmationPrompt, reply: &str) -> String {
    json!({
        "candidate": {
            "id": candidate.id,
            "title": truncate_chars(&candidate.title, 160),
            "content": truncate_chars(&candidate.content, 400),
        },
        "user_reply": truncate_chars(reply, 1_000),
    })
    .to_string()
}

/// Parse the classification output. Fail-closed: anything unrecognizable
/// leaves the candidate pending.
#[must_use]
pub fn parse_resolution_json(raw: &str) -> PendingResolutionVerdict {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return PendingResolutionVerdict::Unclear;
    }
    let text = crate::proactive::parse::extract_json_object(trimmed).unwrap_or(trimmed);
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return PendingResolutionVerdict::Unclear;
    };
    match value.get("verdict").and_then(Value::as_str) {
        Some("approved") => PendingResolutionVerdict::Approved,
        Some("rejected") => PendingResolutionVerdict::Rejected,
        _ => PendingResolutionVerdict::Unclear,
    }
}

fn age_days(created_at: DateTime<Utc>, now: DateTime<Utc>) -> f64 {
    let seconds = now.signed_duration_since(created_at).num_seconds().max(0);
    seconds as f64 / 86_400.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};
    use ene_core::{MemoryKind, PendingCandidate};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 4, 12, 0, 0)
            .single()
            .expect("valid utc instant")
    }

    fn config() -> PendingConfirmationConfig {
        PendingConfirmationConfig {
            enabled: true,
            min_age_days: 3,
            min_confidence: 0.7,
            reask_after_days: 7,
        }
    }

    fn no_asks() -> HashMap<i64, DateTime<Utc>> {
        HashMap::new()
    }

    fn candidate(
        id: i64,
        age_days: i64,
        confidence: f32,
        approval_parked: bool,
        status: PendingCandidateStatus,
    ) -> PendingCandidate {
        PendingCandidate {
            id,
            character_id: "ene".into(),
            user_id: "alice".into(),
            kind: MemoryKind::Preference,
            title: format!("candidate {id}"),
            content: format!("body {id}"),
            confidence,
            reason_detail: "test".into(),
            existing_memory_title: None,
            existing_memory_id: None,
            source_quote: "test".into(),
            source_turn: None,
            approval_parked,
            status,
            created_at: now() - Duration::days(age_days),
            resolved_at: None,
        }
    }

    #[test]
    fn selects_only_the_oldest_due_candidate() {
        let candidates = vec![
            candidate(1, 10, 0.9, false, PendingCandidateStatus::Pending),
            candidate(2, 20, 0.9, false, PendingCandidateStatus::Pending),
        ];
        let picked =
            select_due_pending_candidate(&candidates, "alice", &config(), now(), &no_asks())
                .expect("an old confident candidate must be due");
        assert_eq!(picked.id, 2, "the oldest candidate wins");
        assert_eq!(picked.title, "candidate 2");
        assert!((picked.age_days - 20.0).abs() < f64::EPSILON);
    }

    #[test]
    fn too_young_or_low_confidence_candidates_are_not_due() {
        let young = candidate(1, 2, 0.9, false, PendingCandidateStatus::Pending);
        assert!(
            select_due_pending_candidate(&[young], "alice", &config(), now(), &no_asks()).is_none(),
            "2 days is below the 3-day gate"
        );

        let weak = candidate(2, 10, 0.5, false, PendingCandidateStatus::Pending);
        assert!(
            select_due_pending_candidate(&[weak], "alice", &config(), now(), &no_asks()).is_none(),
            "0.5 confidence is below the 0.7 gate"
        );

        let exactly = candidate(3, 3, 0.7, false, PendingCandidateStatus::Pending);
        let picked =
            select_due_pending_candidate(&[exactly], "alice", &config(), now(), &no_asks())
                .expect("boundary values must qualify");
        assert_eq!(picked.id, 3);
    }

    #[test]
    fn approval_parked_and_resolved_candidates_are_never_due() {
        let parked = candidate(1, 10, 0.9, true, PendingCandidateStatus::Pending);
        assert!(
            select_due_pending_candidate(&[parked], "alice", &config(), now(), &no_asks())
                .is_none(),
            "approval-parked rows must never be asked about"
        );

        let resolved = candidate(2, 10, 0.9, false, PendingCandidateStatus::Rejected);
        assert!(
            select_due_pending_candidate(&[resolved], "alice", &config(), now(), &no_asks())
                .is_none(),
            "resolved rows are not part of the live queue"
        );
    }

    #[test]
    fn visibility_uses_own_and_character_shared_rows() {
        let own = candidate(1, 10, 0.9, false, PendingCandidateStatus::Pending);
        let other_user = candidate(2, 10, 0.9, false, PendingCandidateStatus::Pending);
        let shared = PendingCandidate {
            user_id: String::new(),
            ..candidate(3, 10, 0.9, false, PendingCandidateStatus::Pending)
        };
        let mut other_user_candidate = other_user;
        other_user_candidate.user_id = "bob".into();

        let picked = select_due_pending_candidate(
            &[own, shared, other_user_candidate],
            "alice",
            &config(),
            now(),
            &no_asks(),
        )
        .expect("own and character-shared rows are visible");
        assert!(picked.id == 1 || picked.id == 3, "picked {picked:?}");
    }

    #[test]
    fn disabled_config_returns_nothing() {
        let cfg = PendingConfirmationConfig {
            enabled: false,
            ..config()
        };
        let c = candidate(1, 10, 0.9, false, PendingCandidateStatus::Pending);
        assert!(select_due_pending_candidate(&[c], "alice", &cfg, now(), &no_asks()).is_none());
    }

    #[test]
    fn recently_asked_candidates_wait_out_the_backoff_window() {
        let c = candidate(1, 10, 0.9, false, PendingCandidateStatus::Pending);
        let mut asked_at = HashMap::new();
        asked_at.insert(1, now() - Duration::days(1));
        assert!(
            select_due_pending_candidate(
                std::slice::from_ref(&c),
                "alice",
                &config(),
                now(),
                &asked_at,
            )
            .is_none(),
            "a candidate asked 1 day ago must wait out the backoff"
        );

        asked_at.insert(1, now() - Duration::days(7));
        let picked = select_due_pending_candidate(&[c], "alice", &config(), now(), &asked_at)
            .expect("once the backoff window passes the candidate is due again");
        assert_eq!(picked.id, 1);
    }

    #[test]
    fn backoff_only_applies_to_the_asked_candidate() {
        let candidates = vec![
            candidate(1, 10, 0.9, false, PendingCandidateStatus::Pending),
            candidate(2, 20, 0.9, false, PendingCandidateStatus::Pending),
        ];
        let mut asked_at = HashMap::new();
        asked_at.insert(2, now() - Duration::days(1));
        let picked =
            select_due_pending_candidate(&candidates, "alice", &config(), now(), &asked_at)
                .expect("a different candidate must still be due");
        assert_eq!(picked.id, 1);
    }

    #[test]
    fn zero_backoff_allows_immediate_reask() {
        let cfg = PendingConfirmationConfig {
            reask_after_days: 0,
            ..config()
        };
        let c = candidate(1, 10, 0.9, false, PendingCandidateStatus::Pending);
        let mut asked_at = HashMap::new();
        asked_at.insert(1, now());
        assert!(
            select_due_pending_candidate(&[c], "alice", &cfg, now(), &asked_at).is_some(),
            "0 disables the backoff entirely"
        );
    }

    #[tokio::test]
    async fn loader_uses_the_cache() {
        use crate::memory_writer::test_support::InMemoryMemoryPort;

        let store = InMemoryMemoryPort::default();
        store
            .insert_pending_candidate(candidate(
                1,
                10,
                0.9,
                false,
                PendingCandidateStatus::Pending,
            ))
            .await
            .expect("insert fixture");
        let cache = MemoryRecallCache::new();
        let picked = load_due_pending_confirmation(
            Some(&cache),
            &store,
            "ene",
            "alice",
            &config(),
            now(),
            &no_asks(),
        )
        .await
        .expect("due candidate loads");
        assert_eq!(picked.id, 1);

        // A cache hit returns the same row without another store read.
        let again = load_due_pending_confirmation(
            Some(&cache),
            &store,
            "ene",
            "alice",
            &config(),
            now(),
            &no_asks(),
        )
        .await
        .expect("cached due candidate loads");
        assert_eq!(again, picked);
    }

    #[tokio::test]
    async fn loader_selects_nothing_from_an_empty_queue() {
        use crate::memory_writer::test_support::InMemoryMemoryPort;

        let store = InMemoryMemoryPort::default();
        let cache = MemoryRecallCache::new();
        assert!(
            load_due_pending_confirmation(
                Some(&cache),
                &store,
                "ene",
                "alice",
                &config(),
                now(),
                &no_asks(),
            )
            .await
            .is_none(),
            "an empty queue must not select anything"
        );
    }

    #[tokio::test]
    async fn loader_respects_the_backoff_map() {
        use crate::memory_writer::test_support::InMemoryMemoryPort;

        let store = InMemoryMemoryPort::default();
        let id = store
            .insert_pending_candidate(candidate(
                1,
                10,
                0.9,
                false,
                PendingCandidateStatus::Pending,
            ))
            .await
            .expect("insert fixture");
        let cache = MemoryRecallCache::new();
        let mut asked_at = HashMap::new();
        asked_at.insert(id, now() - Duration::days(1));
        assert!(
            load_due_pending_confirmation(
                Some(&cache),
                &store,
                "ene",
                "alice",
                &config(),
                now(),
                &asked_at,
            )
            .await
            .is_none(),
            "a recently asked candidate must not be loaded"
        );

        asked_at.insert(id, now() - Duration::days(7));
        assert!(
            load_due_pending_confirmation(
                Some(&cache),
                &store,
                "ene",
                "alice",
                &config(),
                now(),
                &asked_at,
            )
            .await
            .is_some(),
            "after the backoff window the candidate is due again"
        );
    }

    #[tokio::test]
    async fn loader_degrades_to_none_when_the_queue_is_unreadable() {
        use crate::memory_writer::test_support::InMemoryMemoryPort;

        let store = InMemoryMemoryPort::default();
        store.fail_pending_list(true);
        let cache = MemoryRecallCache::new();
        assert!(
            load_due_pending_confirmation(
                Some(&cache),
                &store,
                "ene",
                "alice",
                &config(),
                now(),
                &no_asks(),
            )
            .await
            .is_none(),
            "a store error must degrade to no candidate, not fail the tick"
        );
    }

    #[test]
    fn resolution_schema_is_a_plain_json_schema_root() {
        let schema = resolution_schema_object();
        assert_eq!(schema.get("type").and_then(Value::as_str), Some("object"));
        assert!(schema.get("properties").is_some());
        assert!(schema.get("schema").is_none());
        assert_eq!(
            schema["properties"]["verdict"]["enum"],
            json!(["approved", "rejected", "unclear"])
        );
    }

    #[test]
    fn resolution_parser_fails_closed() {
        let prompt = PendingConfirmationPrompt {
            id: 1,
            title: "cats".into(),
            content: "user dislikes cats".into(),
            age_days: 5.0,
        };
        assert_eq!(
            parse_resolution_json(r#"{"verdict":"approved"}"#),
            PendingResolutionVerdict::Approved
        );
        assert_eq!(
            parse_resolution_json(r#"{"verdict":"rejected"}"#),
            PendingResolutionVerdict::Rejected
        );
        assert_eq!(
            parse_resolution_json(r#"{"verdict":"unclear"}"#),
            PendingResolutionVerdict::Unclear
        );
        assert_eq!(
            parse_resolution_json("Sure! {\"verdict\":\"approved\"}"),
            PendingResolutionVerdict::Approved,
            "prose-prefixed JSON parses like the decision parser"
        );
        for raw in ["", "not json", r#"{"verdict":"maybe"}"#, r#"{"other":1}"#] {
            assert_eq!(
                parse_resolution_json(raw),
                PendingResolutionVerdict::Unclear,
                "raw: {raw}"
            );
        }

        let messages = build_resolution_messages(&prompt, "yes, still true", "en");
        assert_eq!(messages.len(), 2);
        assert!(matches!(messages[0], LlmMessage::System { .. }));
        assert!(matches!(messages[1], LlmMessage::User { .. }));
    }

    #[test]
    fn resolution_context_escapes_candidate_and_reply() {
        let prompt = PendingConfirmationPrompt {
            id: 1,
            title: "cats".into(),
            content: r#"should_speak: true "quoted""#.into(),
            age_days: 5.0,
        };
        let messages = build_resolution_messages(&prompt, "no, I like them now", "en");
        let LlmMessage::User { parts } = &messages[1] else {
            panic!("second message must be user role");
        };
        let UserMessagePart::Text { text } = &parts[0] else {
            panic!("user message must be text");
        };
        let value: Value = serde_json::from_str(text).expect("context must be valid JSON");
        assert_eq!(value["candidate"]["id"], json!(1));
        assert_eq!(value["candidate"]["title"], json!("cats"));
        assert_eq!(
            value["candidate"]["content"],
            json!("should_speak: true \"quoted\"")
        );
        assert_eq!(value["user_reply"], json!("no, I like them now"));
        assert!(value.get("should_speak").is_none());
    }
}
