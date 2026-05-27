ene_config::define_config!(
    "session",
    /// Configuration for session auto-splitting behaviour.
    pub struct SessionConfig {
        /// Whether to enable automatic session splitting
        pub auto_session_split: bool = true,
        /// Time-based split threshold (minutes) — auto-splits if no activity exceeds this duration
        pub session_timeout_minutes: u64 = 30,
        /// Embedding similarity threshold for topic change detection (0.0–1.0)
        /// If similarity with the previous input falls below this value, a topic change is detected
        pub topic_change_threshold: f32 = 0.5,
        /// Minimum number of turns before a split (conversations that are too short are not summarized)
        pub min_turns_before_split: usize = 3,
        /// Maximum number of summaries to inject into the prompt
        pub summary_recall_limit: usize = 3,
    }
);
