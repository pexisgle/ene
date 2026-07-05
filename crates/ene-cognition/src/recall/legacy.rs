//! Legacy table recall merge for unmigrated character cards (#98).

use chrono::Utc;
use ene_memory::keyfact_kind_for_key;
use ene_memory::{
    AffectAnnotation, KeyFact, MemoryCandidateSource, MemoryConfidence, MemoryItem, MemoryKind,
    MemorySalience, MemoryScope, MemoryScoreBreakdown, MemorySource, MemoryStatus, MemoryStore,
    RecalledSummary,
};

use crate::recall::{RecallReason, RecalledMemory};

/// Merge legacy summaries and keyfacts into typed recall results when migration
/// has not completed for the card.
pub async fn merge_legacy_recall(
    store: &MemoryStore,
    card_name: &str,
    query_embedding: &[f32],
    limit: usize,
    similarity_threshold: f32,
    typed_recalled: Vec<RecalledMemory>,
) -> Result<Vec<RecalledMemory>, ene_memory::MemoryError> {
    if store.is_legacy_migrated(card_name).await? {
        return Ok(typed_recalled);
    }

    let (summaries, keyfacts) = store
        .recall_context(card_name, query_embedding, limit, similarity_threshold)
        .await?;

    let mut merged = typed_recalled;
    let mut existing_keys = normalized_content_keys(&merged);

    for summary in summaries {
        let content = summary.entry.summary.clone();
        let key = normalize_content_key(&content);
        if !existing_keys.insert(key) {
            continue;
        }
        merged.push(summary_to_recalled(summary, card_name));
        if merged.len() >= limit {
            return Ok(truncate_to_limit(merged, limit));
        }
    }

    for fact in keyfacts {
        if fact.value.is_empty() {
            continue;
        }
        let content = fact.value.clone();
        let key = normalize_content_key(&content);
        if !existing_keys.insert(key) {
            continue;
        }
        merged.push(keyfact_to_recalled(&fact, card_name));
        if merged.len() >= limit {
            break;
        }
    }

    Ok(truncate_to_limit(merged, limit))
}

fn truncate_to_limit(mut recalled: Vec<RecalledMemory>, limit: usize) -> Vec<RecalledMemory> {
    recalled.truncate(limit);
    recalled
}

fn normalized_content_keys(recalled: &[RecalledMemory]) -> std::collections::HashSet<String> {
    recalled
        .iter()
        .map(|m| normalize_content_key(&m.item.content))
        .collect()
}

fn normalize_content_key(content: &str) -> String {
    content
        .trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn summary_to_recalled(summary: RecalledSummary, card_name: &str) -> RecalledMemory {
    let title = summary
        .entry
        .summary
        .lines()
        .next()
        .unwrap_or(&summary.entry.summary)
        .chars()
        .take(80)
        .collect::<String>();

    RecalledMemory {
        item: MemoryItem {
            id: None,
            scope: MemoryScope::Shared,
            character_id: card_name.to_string(),
            user_id: String::new(),
            kind: MemoryKind::Episodic,
            title,
            content: summary.entry.summary,
            source: MemorySource::Imported,
            source_ref: Some(format!("legacy:summary:{}", summary.entry.session_id)),
            confidence: MemoryConfidence::new(0.7),
            salience: MemorySalience::new(0.5),
            affect: AffectAnnotation::default(),
            relationship_impact: 0.0,
            access_count: 0,
            last_accessed_at: None,
            created_at: summary.entry.ended_at,
            updated_at: summary.entry.ended_at,
            valid_from: None,
            valid_until: None,
            status: MemoryStatus::Active,
            supersedes_id: None,
            pinned: false,
            faded_at: None,
        },
        reason: RecallReason::RecentConversation,
        score_breakdown: MemoryScoreBreakdown {
            vector_similarity: summary.similarity,
            lexical_score: 0.0,
            recency_score: 0.0,
            salience: 0.5,
            confidence: 0.7,
            emotional_match: 0.0,
            relationship: 0.0,
            access_boost: 0.0,
            contradiction_penalty: 0.0,
            stale_penalty: 0.0,
            commitment_boost: 0.0,
            total: summary.similarity,
        },
        sources: vec![MemoryCandidateSource::Vector],
    }
}

fn keyfact_to_recalled(fact: &KeyFact, card_name: &str) -> RecalledMemory {
    let kind = keyfact_kind_for_key(&fact.key);
    let reason = if kind == MemoryKind::Preference {
        RecallReason::UserPreference
    } else {
        RecallReason::SimilarTopic
    };

    RecalledMemory {
        item: MemoryItem {
            id: None,
            scope: MemoryScope::User,
            character_id: card_name.to_string(),
            user_id: String::new(),
            kind,
            title: fact.key.clone(),
            content: fact.value.clone(),
            source: MemorySource::Imported,
            source_ref: None,
            confidence: MemoryConfidence::new(0.7),
            salience: MemorySalience::new(0.5),
            affect: AffectAnnotation::default(),
            relationship_impact: 0.0,
            access_count: 0,
            last_accessed_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            valid_from: None,
            valid_until: None,
            status: MemoryStatus::Active,
            supersedes_id: None,
            pinned: false,
            faded_at: None,
        },
        reason,
        score_breakdown: MemoryScoreBreakdown {
            vector_similarity: 0.0,
            lexical_score: 1.0,
            recency_score: 0.0,
            salience: 0.5,
            confidence: 0.7,
            emotional_match: 0.0,
            relationship: 0.0,
            access_boost: 0.0,
            contradiction_penalty: 0.0,
            stale_penalty: 0.0,
            commitment_boost: 0.0,
            total: 0.7,
        },
        sources: vec![MemoryCandidateSource::Lexical],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_memory::MemoryStore;

    #[tokio::test]
    async fn legacy_summary_surfaces_when_not_migrated() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let emb = vec![1.0_f32, 0.0, 0.0, 0.0];
        store
            .insert_summary(
                "s1",
                "char",
                "User loves matcha latte",
                &[],
                &emb,
                Utc::now(),
            )
            .await
            .unwrap();

        let merged = merge_legacy_recall(&store, "char", &emb, 8, 0.0, vec![])
            .await
            .unwrap();

        assert_eq!(merged.len(), 1);
        assert!(merged[0].item.content.contains("matcha"));
    }
}
