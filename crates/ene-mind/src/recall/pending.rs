//! Surface unconfirmed pending candidates through hybrid recall.
//!
//! Candidates the arbiter deferred with `AskConfirmationLater` live in
//! the user-approval queue — a `pending_candidates` table row, not a typed
//! memory. They carry no embedding, so the store's gather (`MemoryPort::search`)
//! can never surface them on its own. This module loads the live `pending`
//! queue and synthesizes the candidates into [`GatheredCandidate`]s marked with
//! [`MemoryCandidateSource::Pending`] so they compete in the **normal** hybrid
//! score competition downstream.
//!
//! Topic proximity is the gate: a candidate with no lexical overlap against the
//! recall query is dropped here, and beyond that it must clear the same
//! `min_score` floor as every typed memory. Unconfirmed information therefore
//! only reaches the prompt when the conversation actually touches the topic —
//! nothing is force-injected (unlike lorebook constant entries).

use ene_core::{
    AffectAnnotation, GatheredCandidate, MemoryCandidateSource, MemoryConfidence, MemoryItem,
    MemoryPort, MemorySalience, MemoryScope, MemorySource, MemoryStatus, PendingCandidate,
    PendingCandidateStatus, Query,
};
use ene_rag::lexical_overlap_score;

use super::MemoryRecallCache;

/// Load the pending queue and merge topic-related candidates into `gathered`.
///
/// `limit` caps how many candidates compete (newest first); a `limit` of `0`
/// disables the path entirely (callers should skip the call). Candidates with
/// no lexical overlap against the query text are dropped, keeping the
/// competition topic-gated. Failures to list the queue degrade to "no pending
/// candidates" with a warning, mirroring how the runner treats commitment-list
/// failures — recall must never fail a turn because the queue was unreadable.
pub async fn gather_pending_candidates(
    cache: Option<&MemoryRecallCache>,
    store: &dyn MemoryPort,
    query: &Query<'_>,
    gathered: &mut Vec<GatheredCandidate>,
    limit: usize,
) {
    let listed = match cache {
        Some(cache) => {
            cache
                .list_pending_candidates(store, query.character_id)
                .await
        }
        None => {
            store
                .list_pending_candidates(query.character_id, Some(PendingCandidateStatus::Pending))
                .await
        }
    };
    let mut candidates = match listed {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(
                component = "RecallRunner",
                character_id = %query.character_id,
                error = %error,
                "Failed to list pending candidates for recall; continuing without them"
            );
            return;
        }
    };

    // A user's recall sees only their own pending candidates plus
    // character-shared (`user_id = ""`) ones, mirroring the typed-memory
    // visibility rule (`tm.user_id = ? OR tm.user_id = ''`), so one user's
    // unconfirmed candidates never leak into another user's conversation on a
    // multi-user database. The filter must run *before* the cap: on a
    // multi-user database the newest `limit` rows can all belong to other
    // users, and truncating first would drop this user's topic-relevant
    // candidates before they ever compete.
    candidates.retain(|candidate| {
        query
            .user_id
            .is_none_or(|user_id| candidate.user_id == user_id || candidate.user_id.is_empty())
    });

    // Newest first: the freshest unconfirmed info is the most relevant to ask
    // about, and the retention policy can hold hundreds of rows per character.
    candidates.sort_by_key(|c| std::cmp::Reverse(c.created_at));
    candidates.truncate(limit);

    for candidate in candidates {
        let lexical = lexical_overlap_score(query.query_text, &candidate.title, &candidate.content);
        if lexical <= 0.0 {
            continue;
        }
        gathered.push(GatheredCandidate {
            item: pending_candidate_to_memory_item(&candidate),
            // Pending candidates have no embedding; relevance comes from the
            // lexical component alone (see `lexical_overlap_score` above).
            vector_similarity: 0.0,
            sources: vec![MemoryCandidateSource::Pending],
        });
    }
}

/// Project a pending candidate onto a [`MemoryItem`] for scoring/rendering.
///
/// The synthesized item deliberately has `id: None`: a pending candidate is not
/// a typed memory, so it must not collide with typed/commitment ids in the
/// gather map, and it must not be access-bumped when the prompt is composed
/// (the injected-memory-id collection skips `None` ids). Everything else is
/// carried over so the candidate renders and scores like an active memory.
fn pending_candidate_to_memory_item(candidate: &PendingCandidate) -> MemoryItem {
    MemoryItem {
        id: None,
        scope: MemoryScope::Character,
        character_id: candidate.character_id.clone(),
        user_id: candidate.user_id.clone(),
        kind: candidate.kind,
        title: candidate.title.clone(),
        content: candidate.content.clone(),
        source: MemorySource::Conversation,
        source_ref: None,
        confidence: MemoryConfidence::new(candidate.confidence),
        salience: MemorySalience::new(candidate.confidence),
        affect: AffectAnnotation::default(),
        relationship_impact: 0.0,
        access_count: 0,
        last_accessed_at: None,
        created_at: candidate.created_at,
        updated_at: candidate.created_at,
        valid_from: None,
        valid_until: None,
        status: MemoryStatus::Active,
        supersedes_id: None,
        pinned: false,
        faded_at: None,
        commitment_id: None,
    }
}
