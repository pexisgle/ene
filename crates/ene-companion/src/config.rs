ene_config::define_config!(
    settings,
    "mind",
    /// Companion cognition: inner channel, affect, memory, proactive speech.
    pub struct MindSettings {
        pub inner: InnerSettings,
        pub affect: AffectSettings,
        pub recall: RecallSettings,
        pub memory_approval: MemoryApprovalSettings,
        pub forgetting: ForgettingSettings,
        pub proactive: ProactiveSettings,
    }
);

ene_config::define_config!(
    settings,
    "characters",
    /// Character package install layout and import.
    pub struct CharacterSettings {
        pub home_dir: String,
        pub import_v3: bool = true,
        pub install_max_total_bytes: u64 = 536_870_912,
        pub redistribute_check: bool = true,
    }
);

/// `mind.inner.*`
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct InnerSettings {
    pub self_reference_window: u32,
    pub auto_emotion_events: bool,
    pub derive_from_thinking: bool,
}

impl Default for InnerSettings {
    fn default() -> Self {
        Self {
            self_reference_window: 24,
            auto_emotion_events: true,
            derive_from_thinking: true,
        }
    }
}

/// `mind.affect.*`
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct AffectSettings {
    pub valence_tau_hours: f64,
    pub irritation_tau_hours: f64,
    pub min_interval_ms: u64,
    pub max_per_minute: u32,
    pub classifier_min_confidence: f32,
}

impl Default for AffectSettings {
    fn default() -> Self {
        Self {
            valence_tau_hours: 6.0,
            irritation_tau_hours: 3.0,
            min_interval_ms: 1500,
            max_per_minute: 12,
            classifier_min_confidence: 0.4,
        }
    }
}

/// `mind.recall.*`
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct RecallSettings {
    pub budget: usize,
    pub mmr_lambda: f32,
    pub weight_lexical: f32,
    pub weight_recency: f32,
    pub weight_salience: f32,
    pub weight_embedding: f32,
}

impl Default for RecallSettings {
    fn default() -> Self {
        Self {
            budget: 8,
            mmr_lambda: 0.7,
            weight_lexical: 0.5,
            weight_recency: 0.25,
            weight_salience: 0.25,
            weight_embedding: 0.35,
        }
    }
}

/// `mind.memory_approval.*`
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct MemoryApprovalSettings {
    pub require_approval: bool,
    pub confidence_threshold: f32,
    pub shared_confidence_threshold: f32,
}

impl Default for MemoryApprovalSettings {
    fn default() -> Self {
        Self {
            require_approval: true,
            confidence_threshold: 0.7,
            shared_confidence_threshold: 0.85,
        }
    }
}

/// `mind.forgetting.*`
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct ForgettingSettings {
    pub mode: ForgettingMode,
    pub salience_threshold: f32,
}

impl Default for ForgettingSettings {
    fn default() -> Self {
        Self {
            mode: ForgettingMode::Confirm,
            salience_threshold: 0.15,
        }
    }
}

/// Forget confirmation vs immediate delete.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    ene_config::schemars::JsonSchema,
)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case")]
#[schemars(crate = "::ene_config::schemars")]
pub enum ForgettingMode {
    #[default]
    Confirm,
    Immediate,
}

/// `mind.proactive.*`
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct ProactiveSettings {
    pub enabled: bool,
    pub paused: bool,
    pub observation_interval_seconds: u64,
    pub min_idle_seconds: u64,
    pub cooldown_seconds: u64,
    pub max_turns_per_session: usize,
    pub decision_timeout_seconds: u64,
    pub min_confidence: f64,
    pub confirmation_enabled: bool,
    pub fatigue_suppression_threshold: f32,
    pub max_conversation_chars: usize,
    pub max_activity_chars: usize,
    pub max_screen_summary_chars: usize,
    pub max_memory_notes: usize,
    pub sources: ProactiveSourcesSettings,
    pub quiet_hours: QuietHoursSettings,
    pub world_state: WorldStateSettings,
    pub pending_confirmation: PendingConfirmationSettings,
}

impl Default for ProactiveSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            paused: false,
            observation_interval_seconds: 60,
            min_idle_seconds: 120,
            cooldown_seconds: 300,
            max_turns_per_session: 6,
            decision_timeout_seconds: 15,
            min_confidence: 0.55,
            confirmation_enabled: false,
            fatigue_suppression_threshold: 0.7,
            max_conversation_chars: 4_000,
            max_activity_chars: 500,
            max_screen_summary_chars: 800,
            max_memory_notes: 12,
            sources: ProactiveSourcesSettings::default(),
            quiet_hours: QuietHoursSettings::default(),
            world_state: WorldStateSettings::default(),
            pending_confirmation: PendingConfirmationSettings::default(),
        }
    }
}

impl ProactiveSettings {
    /// Borderline confidence is forwarded to the main model when confirmation
    /// is on (current-code-true offset 0.15).
    #[must_use]
    pub fn effective_decision_min_confidence(&self) -> f64 {
        if self.confirmation_enabled {
            (self.min_confidence - 0.15).max(0.0)
        } else {
            self.min_confidence
        }
    }

    /// Character `proactive.tendency` shifts the decision threshold only.
    /// Cooldown / quiet hours / min-idle are never pierced.
    #[must_use]
    pub fn with_tendency(&self, tendency: &str) -> Self {
        let mut next = self.clone();
        match tendency {
            "quiet" | "rarely" => next.min_confidence = (self.min_confidence + 0.15).min(0.95),
            "chatty" | "talkative" => next.min_confidence = (self.min_confidence - 0.1).max(0.2),
            _ => {}
        }
        next
    }
}

/// Proactive input sources.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct ProactiveSourcesSettings {
    pub conversation: bool,
    pub activity: bool,
    pub screen_summary: bool,
    pub memory: bool,
}

impl Default for ProactiveSourcesSettings {
    fn default() -> Self {
        Self {
            conversation: true,
            activity: true,
            screen_summary: false,
            memory: true,
        }
    }
}

impl ProactiveSourcesSettings {
    #[must_use]
    pub const fn any_enabled(&self) -> bool {
        self.conversation || self.activity || self.screen_summary || self.memory
    }
}

/// Quiet-hours window.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct QuietHoursSettings {
    pub enabled: bool,
    pub timezone: String,
    pub days: QuietHoursDays,
    pub start: QuietHoursTime,
    pub end: QuietHoursTime,
    pub suppress_decisions: bool,
}

impl Default for QuietHoursSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            timezone: String::new(),
            days: QuietHoursDays::default(),
            start: QuietHoursTime {
                hour: 22,
                minute: 0,
            },
            end: QuietHoursTime { hour: 7, minute: 0 },
            suppress_decisions: true,
        }
    }
}

/// Weekdays the quiet-hours window applies to.
#[derive(
    Debug,
    Clone,
    serde::Serialize,
    serde::Deserialize,
    ene_config::schemars::JsonSchema,
    PartialEq,
    Eq,
)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct QuietHoursDays {
    pub monday: bool,
    pub tuesday: bool,
    pub wednesday: bool,
    pub thursday: bool,
    pub friday: bool,
    pub saturday: bool,
    pub sunday: bool,
}

impl Default for QuietHoursDays {
    fn default() -> Self {
        Self {
            monday: true,
            tuesday: true,
            wednesday: true,
            thursday: true,
            friday: true,
            saturday: true,
            sunday: true,
        }
    }
}

impl QuietHoursDays {
    #[must_use]
    pub fn contains(&self, weekday: chrono::Weekday) -> bool {
        match weekday {
            chrono::Weekday::Mon => self.monday,
            chrono::Weekday::Tue => self.tuesday,
            chrono::Weekday::Wed => self.wednesday,
            chrono::Weekday::Thu => self.thursday,
            chrono::Weekday::Fri => self.friday,
            chrono::Weekday::Sat => self.saturday,
            chrono::Weekday::Sun => self.sunday,
        }
    }
}

/// Local wall clock.
#[derive(
    Debug,
    Clone,
    Copy,
    serde::Serialize,
    serde::Deserialize,
    ene_config::schemars::JsonSchema,
    PartialEq,
    Eq,
)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct QuietHoursTime {
    pub hour: u8,
    pub minute: u8,
}

impl Default for QuietHoursTime {
    fn default() -> Self {
        Self {
            hour: 22,
            minute: 0,
        }
    }
}

impl QuietHoursTime {
    #[must_use]
    pub fn minutes_since_midnight(self) -> Option<u32> {
        (self.hour <= 23 && self.minute <= 59)
            .then(|| u32::from(self.hour) * 60 + u32::from(self.minute))
    }
}

/// How much window-title text observation may send to the model.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    ene_config::schemars::JsonSchema,
)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case")]
#[schemars(crate = "::ene_config::schemars")]
pub enum ObservationTitleMode {
    /// App name when it can be parsed; never the document title.
    #[default]
    AppOnly,
    /// Title with path, URL, email, and digit-heavy tokens removed.
    RedactedTitle,
    /// Unredacted title (still truncated by proactive char caps).
    FullTitle,
}

/// Non-persistent world-state ring.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct WorldStateSettings {
    pub enabled: bool,
    pub max_snapshots: usize,
    pub min_snapshots_for_trend: usize,
    pub engaged_idle_seconds: u64,
    pub change_window: usize,
    pub title_mode: ObservationTitleMode,
    /// Local OCR hint slot. No bundled backend; enabling it sends nothing extra yet.
    pub ocr_hint: bool,
}

impl Default for WorldStateSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            max_snapshots: 64,
            min_snapshots_for_trend: 3,
            engaged_idle_seconds: 60,
            change_window: 3,
            title_mode: ObservationTitleMode::AppOnly,
            ocr_hint: false,
        }
    }
}

impl WorldStateSettings {
    /// Contract the product GUI shows as the current observation send scope.
    #[must_use]
    pub fn send_scope(&self) -> ObservationSendScope {
        ObservationSendScope {
            title_mode: self.title_mode,
            ocr_hint: self.ocr_hint,
            screen: "overview_and_roi",
            persist: "digest_and_summary",
        }
    }
}

/// What observation may send now (settings, not a live frame).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case")]
pub struct ObservationSendScope {
    pub title_mode: ObservationTitleMode,
    pub ocr_hint: bool,
    pub screen: &'static str,
    pub persist: &'static str,
}

/// Proactive confirmation of parked memory candidates.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct PendingConfirmationSettings {
    pub enabled: bool,
    pub min_age_days: u32,
    pub min_confidence: f32,
}

impl Default for PendingConfirmationSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            min_age_days: 3,
            min_confidence: 0.7,
        }
    }
}
