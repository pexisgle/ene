# `Commitment` / Active Commitments Ledger Specifications

The `Commitment` module tracks, persists, and monitors promises, scheduled tasks, and follow-ups made by the companion during conversation. These are integrated into prompt packing to prevent the companion from forgetting its obligations.

---

## 1. Data Structures

### `CommitmentStatus` (Public / Enum)
Tracks the state of a promise:
*   `Active`: Actionable promise. Surfaced in prompt sections and search pools.
*   `Done`: Fulfilled.
*   `Cancelled`: Withdrawn or cancelled.
*   `Stale`: Expired or superseded.

### `Commitment` (Public / Struct)
The persistent database commitment record:
```rust
pub struct Commitment {
    pub id: Option<i64>,
    pub character_id: String,
    pub user_id: String,
    pub title: String,
    pub description: String,
    pub status: CommitmentStatus,
    pub due_at: Option<DateTime<Utc>>,
    pub due_label: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}
```
*   `due_at`: Absolute datetime for deadline evaluation.
*   `due_label`: Raw temporal string parsed by the LLM (e.g. "tomorrow", "next Monday").

---

## 2. Low-Level Database Operations

`MemoryStore` (`store/mod.rs`) provides the following CRUD operations:

### `insert_commitment`
*   **Signature**: `pub async fn insert_commitment(&self, row: &NewCommitment) -> Result<i64, MemoryError>`
*   **Description**: Inserts a new commitment row and returns the primary key ID.

### `list_active_commitments`
*   **Signature**: `pub async fn list_active_commitments(&self, character_id: &str, user_id: Option<&str>, limit: usize) -> Result<Vec<Commitment>, MemoryError>`
*   **Description**: Loads up to `limit` active commitments matching the character and user scope.

### `update_commitment_status`
*   **Signature**: `pub async fn update_commitment_status(&self, id: i64, status: CommitmentStatus, fail_reason: Option<&str>) -> Result<bool, MemoryError>`
*   **Description**: Transitions the status of a commitment. If set to `Done`, it automatically stamps the current UTC timestamp onto `completed_at`.

---

## 3. Prompt Representation (`ActiveCommitmentPrompt`)

To ensure active commitments are always surfaced regardless of vector similarity matching, they are converted into light DTOs:

*   **Attributes**:
    -   `id`: Primary key.
    -   `title`: Short label.
    -   `description`: Description of the obligation.
    -   `due_label`: Optional deadline label.
*   **Mapping**:
    `CommitmentLedger::active_prompt_candidates` maps database row lists into simplified `ActiveCommitmentPrompt` strings for system prompt injection.
