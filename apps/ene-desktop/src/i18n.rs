//! Japanese / English locale and deny-reason text.
//!
//! Switching locale rewrites labels only. Conversation bodies, history, and
//! Host domain state are not rewritten.

use ene_api::v1::management::ManagementOutcome;
use ene_local_control::{ControlOutcome, FromConfirmation};

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
pub fn control_deny(locale: Locale, from: &FromConfirmation) -> String {
    match (locale, from) {
        (Locale::Ja, FromConfirmation::DeniedByBoundary) => {
            String::from("確認は Host が起動した GUI の専用チャネルとセッションに束縛されます。")
        }
        (Locale::En, FromConfirmation::DeniedByBoundary) => String::from(
            "Confirmation is bound to the private channel and session the Host issued to its own GUI.",
        ),
        (Locale::Ja, FromConfirmation::Unavailable) => {
            String::from("Host の確認面を利用できません。")
        }
        (Locale::En, FromConfirmation::Unavailable) => String::from("Confirmation is unavailable."),
        (Locale::Ja, FromConfirmation::Outcome(ControlOutcome::CredentialRefused { .. })) => {
            String::from(
                "資格情報は保存されませんでした。OS の保護ストアが使えないか、登録が拒否されました。",
            )
        }
        (Locale::En, FromConfirmation::Outcome(ControlOutcome::CredentialRefused { .. })) => {
            String::from(
                "The credential was not stored. The OS protected store is unavailable or refused the put.",
            )
        }
        (Locale::Ja, FromConfirmation::Outcome(ControlOutcome::CredentialUncommitted { .. })) => {
            String::from(
                "値は保護ストアに届きましたが、登録の確定が完了していません。再試行する前に保留状態を確認してください。",
            )
        }
        (Locale::En, FromConfirmation::Outcome(ControlOutcome::CredentialUncommitted { .. })) => {
            String::from(
                "The value reached the protected store, but registration did not commit. Check the pending state before retrying.",
            )
        }
        (Locale::Ja, FromConfirmation::Outcome(ControlOutcome::Rejected { .. })) => {
            String::from("確認は拒否されました。変更は適用されていません。")
        }
        (Locale::En, FromConfirmation::Outcome(ControlOutcome::Rejected { .. })) => {
            String::from("The confirmation was declined. No change was applied.")
        }
        (Locale::Ja, FromConfirmation::Outcome(ControlOutcome::DeviceUnknown { .. })) => {
            String::from("このペアリング要求は Host にありません。")
        }
        (Locale::En, FromConfirmation::Outcome(ControlOutcome::DeviceUnknown { .. })) => {
            String::from("This pairing request is unknown to Host.")
        }
        (Locale::Ja, _) => String::from("制御の応答を処理できません。"),
        (Locale::En, _) => String::from("The control channel answered unexpectedly."),
    }
}

/// Renders the requester-listener hold. The Host refused admission because
/// its pending queue is saturated; nothing was accepted. The Owner retries by
/// an explicit action, so no path may resend the held request on its own.
#[must_use]
pub fn backpressure_hold(locale: Locale) -> &'static str {
    match locale {
        Locale::Ja => "混雑のため保留中です。しばらくしてから再試行してください。",
        Locale::En => "Held due to load; retry shortly.",
    }
}

#[cfg(test)]
mod tests {
    use super::{Locale, backpressure_hold, control_deny};
    use ene_local_control::{ControlOutcome, FromConfirmation};

    #[test]
    fn credential_refused_is_not_an_unexpected_control_answer() {
        let refused = FromConfirmation::Outcome(ControlOutcome::CredentialRefused {
            provider: String::from("openai"),
            label: String::from("main"),
        });
        let ja = control_deny(Locale::Ja, &refused);
        let en = control_deny(Locale::En, &refused);
        assert!(!ja.contains("処理できません"));
        assert!(!en.contains("unexpected"));
        assert!(ja.contains("保護ストア") || ja.contains("拒否"));
    }

    #[test]
    fn a_requester_hold_has_its_own_notice_in_both_locales() {
        let ja = backpressure_hold(Locale::Ja);
        let en = backpressure_hold(Locale::En);
        assert!(ja.contains("保留"));
        assert_eq!(en, "Held due to load; retry shortly.");
        // The hold is not the generic unexpected-answer fallback.
        assert!(!ja.contains("処理できません"));
        assert!(!en.contains("unexpected"));
    }

    #[test]
    fn locale_tags_round_trip() {
        assert_eq!(Locale::parse("en").as_tag(), "en");
        assert_eq!(Locale::parse("ja").as_tag(), "ja");
        assert_eq!(Locale::parse("EN-US").as_tag(), "en");
        assert_eq!(Locale::parse("fr").as_tag(), "ja");
    }
}
