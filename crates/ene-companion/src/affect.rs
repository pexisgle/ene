use crate::config::AffectSettings;
use crate::inner::EmotionReport;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Discrete vocabulary (24 labels) with PAD centroids.
pub const VOCABULARY: &[&str] = &[
    "happy",
    "joyful",
    "excited",
    "amused",
    "content",
    "calm",
    "relaxed",
    "sleepy",
    "bored",
    "curious",
    "interested",
    "surprised",
    "confused",
    "worried",
    "anxious",
    "sad",
    "lonely",
    "disappointed",
    "embarrassed",
    "shy",
    "angry",
    "annoyed",
    "jealous",
    "determined",
];

/// PAD + relationship metrics persisted on the soul row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AffectState {
    pub valence: f32,
    pub arousal: f32,
    pub dominance: f32,
    pub trust: f32,
    pub affinity: f32,
    pub irritation: f32,
    pub curiosity: f32,
    pub fatigue: f32,
    pub mood_label: String,
    pub last_report_ts: String,
}

impl Default for AffectState {
    fn default() -> Self {
        Self::baseline(&AffectBaseline::default())
    }
}

impl AffectState {
    #[must_use]
    pub fn baseline(baseline: &AffectBaseline) -> Self {
        Self {
            valence: baseline.valence,
            arousal: baseline.arousal,
            dominance: baseline.dominance,
            trust: baseline.trust,
            affinity: baseline.affinity,
            irritation: baseline.irritation,
            curiosity: baseline.curiosity,
            fatigue: baseline.fatigue,
            mood_label: "calm".to_owned(),
            last_report_ts: Utc::now().to_rfc3339(),
        }
    }

    pub fn clamp(&mut self) {
        self.valence = self.valence.clamp(-1.0, 1.0);
        self.arousal = self.arousal.clamp(-1.0, 1.0);
        self.dominance = self.dominance.clamp(-1.0, 1.0);
        self.trust = self.trust.clamp(-1.0, 1.0);
        self.affinity = self.affinity.clamp(-1.0, 1.0);
        self.irritation = self.irritation.clamp(0.0, 1.0);
        self.curiosity = self.curiosity.clamp(0.0, 1.0);
        self.fatigue = self.fatigue.clamp(0.0, 1.0);
    }

    /// Coarse words for System Context. PAD numbers stay off the surface.
    #[must_use]
    pub fn summary_words(&self) -> String {
        format!(
            "mood={} energy={} rapport={}",
            self.mood_label,
            if self.arousal > 0.3 {
                "high"
            } else if self.arousal < -0.2 {
                "low"
            } else {
                "steady"
            },
            if self.affinity > 0.4 {
                "close"
            } else if self.trust < 0.0 {
                "wary"
            } else {
                "neutral"
            }
        )
    }
}

/// Card baseline (decay attractor). Trust/affinity still stored here but
/// decay does not pull those fields.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AffectBaseline {
    pub valence: f32,
    pub arousal: f32,
    pub dominance: f32,
    pub trust: f32,
    pub affinity: f32,
    pub irritation: f32,
    pub curiosity: f32,
    pub fatigue: f32,
}

impl Default for AffectBaseline {
    fn default() -> Self {
        Self {
            valence: 0.2,
            arousal: 0.1,
            dominance: 0.0,
            trust: 0.3,
            affinity: 0.3,
            irritation: 0.0,
            curiosity: 0.4,
            fatigue: 0.0,
        }
    }
}

/// Surface presentation: discrete label + intensity. No PAD numbers.
#[derive(Debug, Clone, PartialEq)]
pub struct AffectPresentation {
    pub label: String,
    pub intensity: f32,
}

/// Project `state` from `last_report_ts` to `now` (skip if clock rewound).
pub fn project_decay(
    state: &mut AffectState,
    baseline: &AffectBaseline,
    settings: &AffectSettings,
    now: DateTime<Utc>,
) {
    let Ok(then) = DateTime::parse_from_rfc3339(&state.last_report_ts) else {
        return;
    };
    let then = then.with_timezone(&Utc);
    if now < then {
        return;
    }
    let elapsed = now - then;
    let hours = elapsed.num_milliseconds() as f64 / 3_600_000.0;
    if hours <= 0.0 {
        return;
    }
    let fast = (-hours / settings.valence_tau_hours.max(0.01)).exp() as f32;
    let slow = (-hours / settings.irritation_tau_hours.max(0.01)).exp() as f32;
    state.valence = baseline.valence + (state.valence - baseline.valence) * fast;
    state.arousal = baseline.arousal + (state.arousal - baseline.arousal) * fast;
    state.dominance = baseline.dominance + (state.dominance - baseline.dominance) * fast;
    state.irritation = baseline.irritation + (state.irritation - baseline.irritation) * slow;
    state.curiosity = baseline.curiosity + (state.curiosity - baseline.curiosity) * slow;
    state.fatigue = baseline.fatigue + (state.fatigue - baseline.fatigue) * slow;
    state.clamp();
    nearest_label(state.valence, state.arousal, state.dominance).clone_into(&mut state.mood_label);
    state.last_report_ts = now.to_rfc3339();
}

/// Deterministic user-utterance deltas plus optional classifier blend.
pub fn apply_turn_signals(
    state: &mut AffectState,
    user_text: &str,
    proposal: Option<&AffectProposal>,
    settings: &AffectSettings,
) {
    let lower = user_text.to_ascii_lowercase();
    if looks_positive(&lower) {
        state.valence = (state.valence + 0.1).clamp(-1.0, 1.0);
        state.affinity = (state.affinity + 0.002).clamp(-1.0, 1.0);
    }
    if looks_negative(&lower) {
        state.valence = (state.valence - 0.12).clamp(-1.0, 1.0);
        state.irritation = (state.irritation + 0.1).clamp(0.0, 1.0);
        state.trust = (state.trust - 0.005).clamp(-1.0, 1.0);
    }
    if let Some(proposal) = proposal
        && proposal.confidence >= settings.classifier_min_confidence
    {
        let w = (proposal.confidence - settings.classifier_min_confidence)
            / (1.0 - settings.classifier_min_confidence).max(0.01);
        state.valence += (proposal.valence - state.valence) * w * 0.4;
        state.arousal += (proposal.arousal - state.arousal) * w * 0.4;
        state.irritation += (proposal.irritation - state.irritation) * w * 0.3;
        state.affinity += (proposal.affinity - state.affinity) * w * 0.2;
    }
    apply_conversation_fatigue(state, user_text);
    state.clamp();
    nearest_label(state.valence, state.arousal, state.dominance).clone_into(&mut state.mood_label);
}

/// Raise fatigue from conversational activity. Empty text is a no-op.
pub fn apply_conversation_fatigue(state: &mut AffectState, user_text: &str) {
    let trimmed = user_text.trim();
    if trimmed.is_empty() {
        return;
    }
    let chars = u16::try_from(trimmed.chars().count()).unwrap_or(u16::MAX);
    let extra = (f32::from(chars) / 160.0) * 0.01;
    state.fatigue = (state.fatigue + 0.02 + extra.min(0.04)).clamp(0.0, 1.0);
}

/// Apply a model self-report as an input event, then return the presentation.
pub fn apply_self_report(
    state: &mut AffectState,
    report: &EmotionReport,
    now: DateTime<Utc>,
) -> AffectPresentation {
    let (label, intensity) = normalize_label(&report.label, report.intensity);
    let centroid = centroid_for(&label);
    let w = intensity.clamp(0.0, 1.0);
    state.valence += (centroid.0 - state.valence) * w * 0.5;
    state.arousal += (centroid.1 - state.arousal) * w * 0.5;
    state.dominance += (centroid.2 - state.dominance) * w * 0.3;
    state.clamp();
    state.mood_label.clone_from(&label);
    state.last_report_ts = now.to_rfc3339();
    AffectPresentation { label, intensity }
}

/// Output arbiter: self-report wins; hysteresis + rate limit.
#[derive(Debug, Clone)]
pub struct ExpressionArbiter {
    last_label: String,
    last_change: DateTime<Utc>,
    window: Vec<DateTime<Utc>>,
}

impl Default for ExpressionArbiter {
    fn default() -> Self {
        Self {
            last_label: "calm".to_owned(),
            last_change: DateTime::<Utc>::UNIX_EPOCH,
            window: Vec::new(),
        }
    }
}

impl ExpressionArbiter {
    pub fn decide(
        &mut self,
        presentation: AffectPresentation,
        settings: &AffectSettings,
        now: DateTime<Utc>,
        mapped: bool,
    ) -> Option<AffectPresentation> {
        let mut out = presentation;
        if !mapped {
            let (label, intensity) = normalize_label(&out.label, out.intensity * 0.7);
            out.label = label;
            out.intensity = intensity;
        }
        self.window
            .retain(|ts| now.signed_duration_since(*ts) < chrono::Duration::minutes(1));
        if u32::try_from(self.window.len()).unwrap_or(u32::MAX) >= settings.max_per_minute {
            return None;
        }
        let same = out.label == self.last_label;
        let elapsed = now.signed_duration_since(self.last_change);
        let min = Duration::from_millis(settings.min_interval_ms);
        if same && elapsed.to_std().unwrap_or(Duration::ZERO) < min {
            return None;
        }
        if !same && elapsed.to_std().unwrap_or(Duration::ZERO) < min {
            return None;
        }
        self.last_label.clone_from(&out.label);
        self.last_change = now;
        self.window.push(now);
        Some(out)
    }
}

/// Optional LLM affect proposal (advisory).
#[derive(Debug, Clone, PartialEq)]
pub struct AffectProposal {
    pub valence: f32,
    pub arousal: f32,
    pub irritation: f32,
    pub affinity: f32,
    pub confidence: f32,
}

/// Parse a classifier JSON object. Fail-closed: malformed input is `None`.
#[must_use]
pub fn parse_affect_json(raw: &str) -> Option<AffectProposal> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let text = extract_json_object(trimmed).unwrap_or(trimmed);
    let value = serde_json::from_str::<serde_json::Value>(text).ok()?;
    let obj = value.as_object()?;
    let f = |key: &str| {
        obj.get(key)
            .and_then(serde_json::Value::as_f64)
            .map(|n| n as f32)
    };
    Some(AffectProposal {
        valence: f("valence").unwrap_or(0.0).clamp(-1.0, 1.0),
        arousal: f("arousal").unwrap_or(0.0).clamp(-1.0, 1.0),
        irritation: f("irritation").unwrap_or(0.0).clamp(0.0, 1.0),
        affinity: f("affinity").unwrap_or(0.0).clamp(-1.0, 1.0),
        confidence: f("confidence").unwrap_or(0.0).clamp(0.0, 1.0),
    })
}

fn extract_json_object(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    (end >= start).then(|| &raw[start..=end])
}

fn looks_positive(text: &str) -> bool {
    [
        "thank",
        "thanks",
        "great job",
        "love you",
        "appreciate",
        "すごい",
        "ありがとう",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn looks_negative(text: &str) -> bool {
    ["shut up", "stupid", "hate you", "idiot", "うるさい", "ばか"]
        .iter()
        .any(|needle| text.contains(needle))
}

fn centroid_for(label: &str) -> (f32, f32, f32) {
    match label {
        "happy" | "joyful" | "amused" => (0.7, 0.4, 0.2),
        "excited" => (0.6, 0.8, 0.3),
        "content" | "calm" | "relaxed" => (0.3, -0.2, 0.1),
        "sleepy" | "bored" => (-0.1, -0.6, -0.1),
        "curious" | "interested" => (0.3, 0.4, 0.2),
        "surprised" => (0.2, 0.7, 0.0),
        "confused" | "worried" | "anxious" => (-0.3, 0.5, -0.3),
        "sad" | "lonely" | "disappointed" => (-0.6, -0.2, -0.4),
        "embarrassed" | "shy" => (-0.2, 0.3, -0.5),
        "angry" | "annoyed" | "jealous" => (-0.6, 0.6, 0.4),
        "determined" => (0.2, 0.5, 0.6),
        _ => (0.0, 0.0, 0.0),
    }
}

fn nearest_label(valence: f32, arousal: f32, dominance: f32) -> &'static str {
    VOCABULARY
        .iter()
        .copied()
        .min_by(|a, b| {
            let ca = centroid_for(a);
            let cb = centroid_for(b);
            dist(ca, valence, arousal, dominance)
                .partial_cmp(&dist(cb, valence, arousal, dominance))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or("calm")
}

fn dist(c: (f32, f32, f32), v: f32, a: f32, d: f32) -> f32 {
    (c.0 - v).hypot(c.1 - a).hypot(c.2 - d)
}

fn normalize_label(raw: &str, intensity: f32) -> (String, f32) {
    let needle = raw.trim().to_ascii_lowercase();
    if VOCABULARY.contains(&needle.as_str()) {
        return (needle, intensity.clamp(0.0, 1.0));
    }
    let nearest = nearest_by_name(&needle);
    (nearest.to_owned(), (intensity * 0.7).clamp(0.0, 1.0))
}

fn nearest_by_name(raw: &str) -> &'static str {
    VOCABULARY
        .iter()
        .copied()
        .min_by_key(|label| strsim_dist(raw, label))
        .unwrap_or("calm")
}

fn strsim_dist(a: &str, b: &str) -> usize {
    let mut score = a.len().abs_diff(b.len()) * 4;
    for (ca, cb) in a.chars().zip(b.chars()) {
        if ca != cb {
            score += 1;
        }
    }
    score
}
