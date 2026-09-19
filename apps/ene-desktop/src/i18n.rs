//! Japanese / English UI labels and deny-reason text.
//!
//! Switching locale rewrites labels only. Conversation bodies, history, and
//! Host domain state are not rewritten.

use ene_api::v1::management::ManagementOutcome;
use ene_local_control::FromHost;

/// UI locale. Persisted as a GUI preference, never as setup-complete consent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locale {
    Ja,
    En,
}

impl Locale {
    #[must_use]
    pub fn as_tag(self) -> &'static str {
        match self {
            Self::Ja => "ja",
            Self::En => "en",
        }
    }

    #[must_use]
    pub fn parse(tag: &str) -> Self {
        if tag.eq_ignore_ascii_case("en") || tag.eq_ignore_ascii_case("en-us") {
            Self::En
        } else {
            Self::Ja
        }
    }
}

/// Stable label keys so tests can switch locale without depending on prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Label {
    Chat,
    History,
    Memory,
    Settings,
    About,
    WizardLanguage,
    WizardBundledEne,
    WizardCloudCost,
    WizardCredential,
    WizardAssignment,
    Send,
    Confirm,
    Cancel,
    Next,
    Back,
    CompanionStopped,
    ProviderDown,
    AvatarAbsent,
    Connecting,
    Connected,
    Disconnected,
}

#[must_use]
pub fn label(locale: Locale, key: Label) -> &'static str {
    match (locale, key) {
        (Locale::Ja, Label::Chat) => "会話",
        (Locale::En, Label::Chat) => "Chat",
        (Locale::Ja, Label::History) => "履歴",
        (Locale::En, Label::History) => "History",
        (Locale::Ja, Label::Memory) => "記憶",
        (Locale::En, Label::Memory) => "Memory",
        (Locale::Ja, Label::Settings) => "設定",
        (Locale::En, Label::Settings) => "Settings",
        (Locale::Ja, Label::About) => "情報",
        (Locale::En, Label::About) => "About",
        (Locale::Ja, Label::WizardLanguage) => "UI言語を選んでください",
        (Locale::En, Label::WizardLanguage) => "Choose a UI language",
        (Locale::Ja, Label::WizardBundledEne) => {
            "同梱キャラクターは公式の ene です。編集基盤は使いません。"
        }
        (Locale::En, Label::WizardBundledEne) => {
            "The bundled character is official ene. This is not a character editor."
        }
        (Locale::Ja, Label::WizardCloudCost) => {
            "会話はクラウドへ送られ、トークン課金が発生します。キー登録だけでは送信しません。"
        }
        (Locale::En, Label::WizardCloudCost) => {
            "Chat is sent to the cloud and incurs token cost. Registering a key does not send."
        }
        (Locale::Ja, Label::WizardCredential) => {
            "OpenAI API キーを登録します（この欄は会話ではありません）"
        }
        (Locale::En, Label::WizardCredential) => {
            "Register an OpenAI API key (this field is not chat)"
        }
        (Locale::Ja, Label::WizardAssignment) => "使用モデルを割り当てるとセットアップが完了します",
        (Locale::En, Label::WizardAssignment) => {
            "Assign a model to finish setup. This is the consent step."
        }
        (Locale::Ja, Label::Send) => "送信",
        (Locale::En, Label::Send) => "Send",
        (Locale::Ja, Label::Confirm) => "確認する",
        (Locale::En, Label::Confirm) => "Confirm",
        (Locale::Ja, Label::Cancel) => "キャンセル",
        (Locale::En, Label::Cancel) => "Cancel",
        (Locale::Ja, Label::Next) => "次へ",
        (Locale::En, Label::Next) => "Next",
        (Locale::Ja, Label::Back) => "戻る",
        (Locale::En, Label::Back) => "Back",
        (Locale::Ja, Label::CompanionStopped) => "パートナーは停止中です。管理は利用できます。",
        (Locale::En, Label::CompanionStopped) => {
            "Companion is stopped. Management remains available."
        }
        (Locale::Ja, Label::ProviderDown) => "プロバイダーに到達できません。管理は利用できます。",
        (Locale::En, Label::ProviderDown) => {
            "Provider is unreachable. Management remains available."
        }
        (Locale::Ja, Label::AvatarAbsent) => "アバターはありません。会話と設定は利用できます。",
        (Locale::En, Label::AvatarAbsent) => {
            "Avatar is absent. Chat and settings remain available."
        }
        (Locale::Ja, Label::Connecting) => "接続中",
        (Locale::En, Label::Connecting) => "Connecting",
        (Locale::Ja, Label::Connected) => "接続済み",
        (Locale::En, Label::Connected) => "Connected",
        (Locale::Ja, Label::Disconnected) => "未接続",
        (Locale::En, Label::Disconnected) => "Disconnected",
    }
}

#[must_use]
pub fn management_deny(locale: Locale, outcome: &ManagementOutcome) -> String {
    match (locale, outcome) {
        (Locale::Ja, ManagementOutcome::DeniedByBoundary) => {
            String::from("境界により拒否されました。Client の confirmed=true では完了しません。")
        }
        (Locale::En, ManagementOutcome::DeniedByBoundary) => String::from(
            "Denied by the control boundary. Client confirmed=true cannot complete this.",
        ),
        (Locale::Ja, ManagementOutcome::NeedsClarification) => {
            String::from("内容を確認できません。条件を見直してください。")
        }
        (Locale::En, ManagementOutcome::NeedsClarification) => {
            String::from("Needs clarification; refine the request.")
        }
        (Locale::Ja, ManagementOutcome::HeldByOperation) => {
            String::from("別の操作が進行中です。後で再試行してください。")
        }
        (Locale::En, ManagementOutcome::HeldByOperation) => {
            String::from("Held by a concurrent operation; retry later.")
        }
        (Locale::Ja, ManagementOutcome::StaleBaseView { .. }) => {
            String::from("表示が古いため拒否されました。最新の画面からやり直してください。")
        }
        (Locale::En, ManagementOutcome::StaleBaseView { .. }) => {
            String::from("Stale view; reload and retry.")
        }
        (Locale::Ja, ManagementOutcome::AppliedAsOneTime) => String::from("適用しました。"),
        (Locale::En, ManagementOutcome::AppliedAsOneTime) => String::from("Applied."),
        (Locale::Ja, ManagementOutcome::StoredAsRuleView { .. }) => {
            String::from("規則として保存しました。")
        }
        (Locale::En, ManagementOutcome::StoredAsRuleView { .. }) => {
            String::from("Stored as a rule.")
        }
    }
}

#[must_use]
pub fn control_deny(locale: Locale, from: &FromHost) -> String {
    match (locale, from) {
        (Locale::Ja, FromHost::DeniedByBoundary) => {
            String::from("確認は座席とセッションに束縛されます。空席の先着は真正性ではありません。")
        }
        (Locale::En, FromHost::DeniedByBoundary) => String::from(
            "Confirmation is bound to the seat and session. Empty-seat occupancy is not authenticity.",
        ),
        (Locale::Ja, FromHost::SeatOccupied) => String::from("確認面は既に使用中です。"),
        (Locale::En, FromHost::SeatOccupied) => String::from("The confirmation seat is occupied."),
        (Locale::Ja, FromHost::Unavailable) => String::from("Host の確認面を利用できません。"),
        (Locale::En, FromHost::Unavailable) => String::from("Confirmation is unavailable."),
        (Locale::Ja, _) => String::from("制御の応答を処理できません。"),
        (Locale::En, _) => String::from("The control channel answered unexpectedly."),
    }
}

#[cfg(test)]
mod tests {
    use super::{Label, Locale, label};

    #[test]
    fn locale_switch_changes_labels_only() {
        let ja = label(Locale::Ja, Label::Chat);
        let en = label(Locale::En, Label::Chat);
        assert_ne!(ja, en);
        assert_ne!(
            label(Locale::Ja, Label::Memory),
            label(Locale::En, Label::Memory)
        );
        assert_eq!(Locale::parse("en").as_tag(), "en");
        assert_eq!(Locale::parse("ja").as_tag(), "ja");
    }
}
