//! Setup intents and Host-filtered management views.
//!
//! Management crosses the wire as intents and filtered views only. The Client
//! proposes with [`crate::management::ManagementIntent`] and reads with
//! [`crate::management::ManagementView`]; the
//! Host decides with [`crate::management::ManagementOutcome`]. Secrets never
//! appear in views,
//! and view content never authorizes anything by itself.
//!
//! Setup gating is Host-enforced: the Host refuses out-of-order setup steps
//! regardless of what the Client sends, so a forged or replayed intent gains
//! nothing.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Setup step a management intent asks the Host to take.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SetupIntentKind {
    /// Adjust general settings.
    ConfigureGeneralSettings,
    /// Choose which provider serves an assignment.
    SelectProvider,
    /// Start registering a credential (values stay on the Host surface).
    RegisterCredentialIntent,
    /// Mark setup complete and enter normal operation.
    CompleteSetup,
}

/// One proposed management action.
///
/// `intent_id` is the idempotency key: resending the same intent replays the
/// prior outcome instead of applying twice. `target` and `base_view` are
/// opaque Host-minted references the Client echoes; it never parses,
/// synthesizes, or stores them as keys.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagementIntent {
    /// Sender-minted idempotency key for this intent.
    pub intent_id: Uuid,
    /// Setup step the Client asks the Host to take.
    pub kind: SetupIntentKind,
    /// Opaque target reference; echo-only.
    pub target: String,
    /// Opaque view mark the intent builds on, if any; echo-only.
    pub base_view: Option<String>,
    /// Human-readable explanation shown alongside the intent, if any.
    pub rationale: Option<String>,
}

/// Host-local decision on one [`ManagementIntent`].
///
/// Like round intake, the non-applied variants are `Ok`-side domain outcomes,
/// never errors and never implicit retries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ManagementOutcome {
    /// Applied exactly once under this `intent_id`.
    AppliedAsOneTime,
    /// The Host needs more information before deciding.
    NeedsClarification {
        /// Human-readable explanation for display only.
        detail: String,
    },
    /// Refused at a safety or authority boundary.
    DeniedByBoundary {
        /// Human-readable refusal reason for display only.
        reason: String,
    },
    /// `base_view` is older than the current mark; resend against `current`.
    StaleBaseView {
        /// Opaque current view mark to base the next intent on; echo-only.
        current: String,
    },
    /// An ongoing operation holds management; the Client waits, it does not
    /// resend.
    HeldByOperation,
}

/// Request for a filtered management view.
///
/// Section names are opaque Host-defined strings the Client echoes; asking
/// for an unknown section yields no section, never an error-shaped guess.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagementViewRequest {
    /// Opaque section names wanted; echo-only.
    pub sections: Vec<String>,
}

/// One filtered display section of a [`ManagementView`].
///
/// Display facts only: `body` never contains secrets, and section content
/// never authorizes management actions by itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewSection {
    /// Opaque section discriminator; echo-only, never parsed for authority.
    pub kind: String,
    /// Display title.
    pub title: String,
    /// Display body; never contains secrets.
    pub body: String,
}

/// Host-filtered management view answering a [`ManagementViewRequest`].
///
/// `mark` is the opaque view mark the next [`ManagementIntent`] echoes as
/// `base_view`; the Client never parses, synthesizes, or stores it as a key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagementView {
    /// Opaque view mark for the next intent's `base_view`; echo-only.
    pub mark: String,
    /// Sections the Host chose to show.
    pub sections: Vec<ViewSection>,
}
