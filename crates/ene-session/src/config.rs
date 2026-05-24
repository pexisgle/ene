ene_config::define_config!(
    "session",
    pub struct SessionConfig {
        /// セッション自動分割を有効にするか
        pub auto_session_split: bool = true,
        /// 時間ベースの分割閾値（分）— この時間以上発言がなければ自動分割
        pub session_timeout_minutes: u64 = 30,
        /// トピック変更の embedding 類似度閾値 (0.0–1.0)
        /// 前回入力との類似度がこの値を下回ったらトピック変更と判定
        pub topic_change_threshold: f32 = 0.5,
        /// 分割前の最小ターン数（短すぎる会話は要約しない）
        pub min_turns_before_split: usize = 3,
        /// 要約をプロンプトに注入する最大数
        pub summary_recall_limit: usize = 3,
    }
);
