use crate::classify::{ClassifyModel, ClassifyTask};
use crate::config::{MemoryApprovalSettings, RecallSettings};
use crate::error::CompanionError;
use crate::ids::{CandidateId, MemoryId};
use crate::store::{CompanionStore, RecallWeights};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use ene_session::SoulId;
use serde::{Deserialize, Serialize};

/// Memory kind (P-202). Orthogonal to [`MemoryScope`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Episodic,
    Semantic,
    UserProfile,
    Preference,
    Commitment,
}

impl MemoryKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Episodic => "episodic",
            Self::Semantic => "semantic",
            Self::UserProfile => "user_profile",
            Self::Preference => "preference",
            Self::Commitment => "commitment",
        }
    }

    #[must_use]
    pub fn parse(raw: &str) -> Self {
        match raw {
            "episodic" => Self::Episodic,
            "user_profile" => Self::UserProfile,
            "preference" => Self::Preference,
            "commitment" => Self::Commitment,
            _ => Self::Semantic,
        }
    }
}

/// Who may read the row. `soul_id` is always the writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    Private,
    Shared,
}

impl MemoryScope {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Shared => "shared",
        }
    }

    #[must_use]
    pub fn parse(raw: &str) -> Self {
        if raw == "shared" {
            Self::Shared
        } else {
            Self::Private
        }
    }
}

/// Provenance of a memory row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemorySource {
    Extraction,
    UserStated,
    Tool,
    Import,
    Shared,
}

impl MemorySource {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Extraction => "extraction",
            Self::UserStated => "user_stated",
            Self::Tool => "tool",
            Self::Import => "import",
            Self::Shared => "shared",
        }
    }

    #[must_use]
    pub fn parse(raw: &str) -> Self {
        match raw {
            "user_stated" => Self::UserStated,
            "tool" => Self::Tool,
            "import" => Self::Import,
            "shared" => Self::Shared,
            _ => Self::Extraction,
        }
    }
}

/// Append-only journal action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalAction {
    Created,
    Updated,
    Forgotten,
    Superseded,
    Restored,
    UserRequest,
    Expired,
    Completed,
    CandidateAccepted,
    CandidateRejected,
}

impl JournalAction {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Updated => "updated",
            Self::Forgotten => "forgotten",
            Self::Superseded => "superseded",
            Self::Restored => "restored",
            Self::UserRequest => "user_request",
            Self::Expired => "expired",
            Self::Completed => "completed",
            Self::CandidateAccepted => "candidate_accepted",
            Self::CandidateRejected => "candidate_rejected",
        }
    }
}

/// Insert payload.
#[derive(Debug, Clone)]
pub struct NewMemory {
    pub soul_id: SoulId,
    pub scope: MemoryScope,
    pub kind: MemoryKind,
    pub title: String,
    pub content: String,
    pub confidence: f32,
    pub salience: f32,
    pub source: MemorySource,
    pub source_seq: Option<u64>,
    pub expires_at: Option<String>,
}

/// Persisted memory (writer soul retained; recall text omits it).
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryRecord {
    pub id: MemoryId,
    pub soul_id: SoulId,
    pub scope: MemoryScope,
    pub kind: MemoryKind,
    pub title: String,
    pub content: String,
    pub confidence: f32,
    pub salience: f32,
    pub source: MemorySource,
    pub source_seq: Option<u64>,
    pub created_at: String,
    pub last_access: String,
    pub access_count: u32,
    pub superseded_by: Option<MemoryId>,
    pub expires_at: Option<String>,
    pub schedule_id: Option<String>,
    pub forgotten: bool,
}

/// Recall hit presented as the calling soul's own knowledge.
#[derive(Debug, Clone, PartialEq)]
pub struct RecalledMemory {
    pub id: MemoryId,
    pub kind: MemoryKind,
    pub scope: MemoryScope,
    pub title: String,
    pub content: String,
    pub score: f32,
}

impl RecalledMemory {
    /// Prompt line without writer attribution (D-7).
    #[must_use]
    pub fn as_own_knowledge(&self) -> String {
        format!("{}: {}", self.title, self.content)
    }
}

/// Extracted candidate prior to arbitration.
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryCandidate {
    pub id: CandidateId,
    pub soul_id: SoulId,
    pub kind: MemoryKind,
    pub title: String,
    pub content: String,
    pub scope: MemoryScope,
    pub confidence: f32,
    pub salience: f32,
    pub sensitive: bool,
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CandidateResolution {
    pub accept: bool,
    pub title: Option<String>,
    pub content: Option<String>,
    pub kind: Option<String>,
    pub scope: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryJournalEntry {
    pub seq: u64,
    pub ts: String,
    pub memory_id: Option<MemoryId>,
    pub soul_id: SoulId,
    pub action: String,
    pub payload: serde_json::Value,
}

/// Outcome of [`arbitrate`].
#[derive(Debug, Clone, PartialEq)]
pub enum ArbitrateOutcome {
    Inserted(MemoryRecord),
    Updated(MemoryRecord),
    Queued(CandidateId),
    Rejected(&'static str),
}

/// Deterministic extraction plus optional classifier candidates.
pub async fn extract_turn(
    soul_id: SoulId,
    user_text: &str,
    assistant_text: &str,
    classifier: Option<&dyn ClassifyModel>,
) -> Vec<MemoryCandidate> {
    let mut out = deterministic_extract(soul_id, user_text);
    if let Some(classifier) = classifier
        && let Ok(raw) = classifier
            .complete_json(
                ClassifyTask::MemoryExtract,
                &format!("user: {user_text}\nassistant: {assistant_text}"),
            )
            .await
    {
        let extra = parse_extract_json(soul_id, &raw);
        overlay_classifier_scope(&mut out, &extra);
        for cand in extra {
            if !out.iter().any(|existing| same_fact(existing, &cand)) {
                out.push(cand);
            }
        }
    }
    out
}

fn overlay_classifier_scope(base: &mut [MemoryCandidate], classified: &[MemoryCandidate]) {
    for det in base.iter_mut() {
        if let Some(hit) = classified.iter().find(|cand| same_fact(det, cand)) {
            det.scope = hit.scope;
            det.confidence = det.confidence.max(hit.confidence);
            det.salience = det.salience.max(hit.salience);
            if det.expires_at.is_none() {
                det.expires_at.clone_from(&hit.expires_at);
            }
        }
    }
}

fn same_fact(a: &MemoryCandidate, b: &MemoryCandidate) -> bool {
    a.kind == b.kind
        && (a.title.eq_ignore_ascii_case(&b.title) || a.content.eq_ignore_ascii_case(&b.content))
}

/// Pattern safety net: name / like / remember / forget, plus commitments
/// that carry an explicit ISO-8601 or `YYYY-MM-DD` due. Relative dates
/// (`tomorrow`, `next Friday`) are ignored on this path.
#[must_use]
pub fn deterministic_extract(soul_id: SoulId, user_text: &str) -> Vec<MemoryCandidate> {
    let text = user_text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    let lower = text.to_ascii_lowercase();
    let mut out = Vec::new();
    if let Some(name) = capture_after(&lower, &["my name is ", "call me ", "私の名前は"]) {
        out.push(candidate(
            soul_id,
            MemoryKind::UserProfile,
            "user name",
            &name,
            MemoryScope::Shared,
            0.9,
            false,
        ));
    }
    if let Some(pref) = capture_after(&lower, &["i like ", "i prefer ", "好きなのは"]) {
        out.push(candidate(
            soul_id,
            MemoryKind::Preference,
            "preference",
            &pref,
            MemoryScope::Shared,
            0.8,
            false,
        ));
    }
    if let Some(fact) = capture_after(&lower, &["remember that ", "remember: ", "覚えて: "]) {
        let shared = lower.contains("everyone")
            || lower.contains("all companions")
            || lower.contains("みんな");
        out.push(candidate(
            soul_id,
            MemoryKind::Semantic,
            &truncate(&fact, 40),
            &fact,
            if shared {
                MemoryScope::Shared
            } else {
                MemoryScope::Private
            },
            0.85,
            false,
        ));
    }
    if looks_forget(&lower) {
        out.push(candidate(
            soul_id,
            MemoryKind::Semantic,
            "forget request",
            text,
            MemoryScope::Private,
            0.9,
            false,
        ));
    }
    if let Some(cand) = extract_commitment_candidate(soul_id, text) {
        out.push(cand);
    }
    out
}

fn looks_forget(lower: &str) -> bool {
    (lower.contains("forget ") || lower.contains("忘れて")) && !lower.contains('?')
}

fn capture_after(lower: &str, needles: &[&str]) -> Option<String> {
    for needle in needles {
        if let Some(idx) = lower.find(needle) {
            let rest = lower[idx + needle.len()..].trim();
            let rest = rest.trim_end_matches(['.', '!', '。']);
            if !rest.is_empty() {
                return Some(rest.to_owned());
            }
        }
    }
    None
}

fn candidate(
    soul_id: SoulId,
    kind: MemoryKind,
    title: &str,
    content: &str,
    scope: MemoryScope,
    confidence: f32,
    sensitive: bool,
) -> MemoryCandidate {
    MemoryCandidate {
        id: CandidateId::new(),
        soul_id,
        kind,
        title: title.to_owned(),
        content: content.to_owned(),
        scope,
        confidence,
        salience: confidence,
        sensitive,
        expires_at: None,
    }
}

fn parse_extract_json(soul_id: SoulId, raw: &str) -> Vec<MemoryCandidate> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw.trim()) else {
        return Vec::new();
    };
    let Some(items) = value.get("candidates").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let title = item.get("title")?.as_str()?.trim();
            let content = item.get("content")?.as_str()?.trim();
            if title.is_empty() || content.is_empty() {
                return None;
            }
            let scope = match item.get("scope").and_then(|v| v.as_str()) {
                Some("shared") => MemoryScope::Shared,
                _ => MemoryScope::Private,
            };
            let mut kind =
                MemoryKind::parse(item.get("kind").and_then(|v| v.as_str()).unwrap_or(""));
            let confidence = item
                .get("confidence")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.5) as f32;
            let expires_at = item
                .get("commitment_due")
                .or_else(|| item.get("expires_at"))
                .or_else(|| item.get("due"))
                .and_then(serde_json::Value::as_str)
                .and_then(normalize_explicit_due);
            if expires_at.is_some() {
                kind = MemoryKind::Commitment;
            }
            Some(MemoryCandidate {
                id: CandidateId::new(),
                soul_id,
                kind,
                title: title.to_owned(),
                content: content.to_owned(),
                scope,
                confidence: confidence.clamp(0.0, 1.0),
                salience: item
                    .get("salience")
                    .and_then(serde_json::Value::as_f64)
                    .map_or(confidence, |v| v as f32)
                    .clamp(0.0, 1.0),
                sensitive: looks_sensitive(content),
                expires_at,
            })
        })
        .collect()
}

fn looks_sensitive(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains('@')
        || lower.contains("ssn")
        || lower.contains("password")
        || lower.contains("bank")
        || lower.chars().filter(char::is_ascii_digit).count() >= 8
}

fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

fn persist_due(
    store: &CompanionStore,
    id: MemoryId,
    cand: &MemoryCandidate,
) -> Result<(), CompanionError> {
    if let Some(due) = cand.expires_at.as_deref() {
        store.set_memory_expires_at(id, Some(due))?;
    }
    Ok(())
}

fn extract_commitment_candidate(soul_id: SoulId, text: &str) -> Option<MemoryCandidate> {
    let (due, range) = find_explicit_due(text)?;
    if !has_commitment_cue(text, range.start) {
        return None;
    }
    let stripped = strip_due_clause(text, range);
    let title = if stripped.is_empty() {
        "commitment".to_owned()
    } else {
        truncate(&stripped, 40)
    };
    Some(MemoryCandidate {
        id: CandidateId::new(),
        soul_id,
        kind: MemoryKind::Commitment,
        title,
        content: stripped,
        scope: MemoryScope::Private,
        confidence: 0.88,
        salience: 0.88,
        sensitive: looks_sensitive(text),
        expires_at: Some(due),
    })
}

fn find_explicit_due(text: &str) -> Option<(String, std::ops::Range<usize>)> {
    let mut i = 0;
    while i + 10 <= text.len() {
        if text.is_char_boundary(i)
            && text.is_char_boundary(i + 10)
            && looks_ymd(&text[i..i + 10])
            && let Some((norm, end)) = take_due_at(text, i)
        {
            return Some((norm, i..end));
        }
        i += 1;
        while i < text.len() && !text.is_char_boundary(i) {
            i += 1;
        }
    }
    None
}

fn looks_ymd(slice: &str) -> bool {
    let bytes = slice.as_bytes();
    bytes.len() >= 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[8..10].iter().all(u8::is_ascii_digit)
}

fn take_due_at(text: &str, start: usize) -> Option<(String, usize)> {
    let rest = &text[start..];
    if rest.len() > 10 && rest.as_bytes()[10] == b'T' {
        let rel_end = rest
            .find(|ch: char| ch.is_whitespace() || matches!(ch, ',' | ';' | '。' | '、'))
            .unwrap_or(rest.len());
        let candidate = rest[..rel_end].trim_end_matches(['.', '!', '?', ')', ']']);
        if let Some(norm) = normalize_explicit_due(candidate) {
            return Some((norm, start + candidate.len()));
        }
    }
    normalize_explicit_due(&rest[..10]).map(|norm| (norm, start + 10))
}

fn normalize_explicit_due(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Some(dt.with_timezone(&Utc).to_rfc3339());
    }
    if let Ok(naive) = NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S") {
        return Some(DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc).to_rfc3339());
    }
    let date = NaiveDate::parse_from_str(raw, "%Y-%m-%d").ok()?;
    let end = date.and_hms_opt(23, 59, 59)?;
    Some(DateTime::<Utc>::from_naive_utc_and_offset(end, Utc).to_rfc3339())
}

fn has_commitment_cue(text: &str, due_start: usize) -> bool {
    let lower = text.to_ascii_lowercase();
    if [
        "remind",
        "remember to",
        "don't forget",
        "dont forget",
        "until",
        "約束",
        "期限",
        "リマインド",
        "までに",
    ]
    .iter()
    .any(|cue| {
        if cue.is_ascii() {
            lower.contains(cue)
        } else {
            text.contains(cue)
        }
    }) {
        return true;
    }
    if contains_ascii_word(&lower, "due") {
        return true;
    }
    let before = lower.get(..due_start).unwrap_or("").trim_end();
    before.ends_with("by") || before.ends_with("by:")
}

fn contains_ascii_word(hay: &str, needle: &str) -> bool {
    hay.split(|ch: char| !ch.is_ascii_alphabetic())
        .any(|word| word == needle)
}

fn strip_due_clause(text: &str, range: std::ops::Range<usize>) -> String {
    let mut owned = String::new();
    owned.push_str(text[..range.start].trim());
    let after = text[range.end..].trim();
    if !after.is_empty() {
        if !owned.is_empty() {
            owned.push(' ');
        }
        owned.push_str(after);
    }
    let trimmed = owned.trim();
    let lower = trimmed.to_ascii_lowercase();
    for suffix in [" by", " due", " until", " 期限", " までに"] {
        if lower.ends_with(&suffix.to_ascii_lowercase()) {
            return trimmed[..trimmed.len() - suffix.len()]
                .trim()
                .trim_end_matches([':', '-', ',', '、'])
                .to_owned();
        }
    }
    trimmed.trim_end_matches([':', '-', ',', '、']).to_owned()
}

/// Adopt / merge / queue a candidate.
pub fn arbitrate(
    store: &CompanionStore,
    cand: &MemoryCandidate,
    approval: &MemoryApprovalSettings,
) -> Result<ArbitrateOutcome, CompanionError> {
    if cand.scope == MemoryScope::Shared
        && let Some(existing) = store.find_shared_by_title(&cand.title, cand.kind)?
    {
        store.update_memory_content(existing.id, &cand.content, cand.soul_id)?;
        persist_due(store, existing.id, cand)?;
        return Ok(ArbitrateOutcome::Updated(
            store
                .get_memory(existing.id)?
                .ok_or_else(|| CompanionError::UnknownMemory(existing.id.to_string()))?,
        ));
    }
    if let Some(existing) = store.find_by_title(cand.soul_id, &cand.title, cand.kind)? {
        if contents_conflict(&existing.content, &cand.content) {
            let inserted = store.insert_memory(NewMemory {
                soul_id: cand.soul_id,
                scope: cand.scope,
                kind: cand.kind,
                title: cand.title.clone(),
                content: cand.content.clone(),
                confidence: cand.confidence,
                salience: cand.salience,
                source: MemorySource::Extraction,
                source_seq: None,
                expires_at: cand.expires_at.clone(),
            })?;
            store.supersede(existing.id, inserted.id, cand.soul_id)?;
            return Ok(ArbitrateOutcome::Updated(inserted));
        }
        store.update_memory_content(existing.id, &cand.content, cand.soul_id)?;
        persist_due(store, existing.id, cand)?;
        return Ok(ArbitrateOutcome::Updated(
            store
                .get_memory(existing.id)?
                .ok_or_else(|| CompanionError::UnknownMemory(existing.id.to_string()))?,
        ));
    }
    let threshold = if cand.scope == MemoryScope::Shared {
        approval.shared_confidence_threshold
    } else {
        approval.confidence_threshold
    };
    if cand.sensitive || (approval.require_approval && cand.confidence < threshold) {
        store.insert_candidate(cand)?;
        return Ok(ArbitrateOutcome::Queued(cand.id));
    }
    if cand.salience < 0.15 {
        return Ok(ArbitrateOutcome::Rejected("salience"));
    }
    let inserted = store.insert_memory(NewMemory {
        soul_id: cand.soul_id,
        scope: cand.scope,
        kind: cand.kind,
        title: cand.title.clone(),
        content: cand.content.clone(),
        confidence: cand.confidence,
        salience: cand.salience,
        source: if cand.scope == MemoryScope::Shared {
            MemorySource::Shared
        } else {
            MemorySource::Extraction
        },
        source_seq: None,
        expires_at: cand.expires_at.clone(),
    })?;
    Ok(ArbitrateOutcome::Inserted(inserted))
}

fn contents_conflict(old: &str, new: &str) -> bool {
    let a = old.trim().to_ascii_lowercase();
    let b = new.trim().to_ascii_lowercase();
    a != b && (a.contains("not ") || b.contains("not ") || a.len().abs_diff(b.len()) > 12)
}

/// Apply a user "forget X" against matching titles.
pub fn apply_forget_request(
    store: &CompanionStore,
    soul_id: SoulId,
    text: &str,
    mode: crate::config::ForgettingMode,
) -> Result<u32, CompanionError> {
    let lower = text.to_ascii_lowercase();
    let target = capture_after(&lower, &["forget ", "忘れて"]).unwrap_or_default();
    if target.is_empty() {
        return Ok(0);
    }
    if mode == crate::config::ForgettingMode::Confirm {
        let cand = candidate(
            soul_id,
            MemoryKind::Semantic,
            "forget request",
            text,
            MemoryScope::Private,
            0.9,
            false,
        );
        store.insert_candidate(&cand)?;
        store.journal(
            None,
            soul_id,
            JournalAction::UserRequest,
            &serde_json::json!({ "target": target, "mode": "confirm" }),
        )?;
        return Ok(1);
    }
    if target.len() < 3 {
        return Ok(0);
    }
    let hits = store.recall(
        soul_id,
        &target,
        8,
        &chrono::Utc::now().to_rfc3339(),
        RecallWeights::default(),
    )?;
    let mut n = 0u32;
    for hit in hits {
        if forget_target_matches(&hit.title, &target) {
            store.forget(hit.id, soul_id, JournalAction::UserRequest)?;
            n += 1;
        }
    }
    Ok(n)
}

fn forget_target_matches(title: &str, target: &str) -> bool {
    let title_lower = title.to_ascii_lowercase();
    let target_lower = target.to_ascii_lowercase();
    title_lower == target_lower
        || title_lower.starts_with(&target_lower)
        || title_lower
            .split(|ch: char| !ch.is_alphanumeric())
            .any(|word| word == target_lower)
}

#[must_use]
pub fn recall_weights(settings: &RecallSettings) -> RecallWeights {
    RecallWeights {
        lexical: settings.weight_lexical,
        recency: settings.weight_recency,
        salience: settings.weight_salience,
        embedding: settings.weight_embedding,
        mmr_lambda: settings.mmr_lambda,
    }
}
