use rusqlite::{Connection, TransactionBehavior};

pub(crate) const CURRENT_VERSION: i64 = 42;

const SCHEMA: &str = "
CREATE TABLE action_attempt (
attempt_id TEXT PRIMARY KEY,
task_id TEXT NOT NULL,
task_revision INTEGER NOT NULL,
delegation_id TEXT NOT NULL,
workspace_assoc_id TEXT NOT NULL,
real_target TEXT NOT NULL,
operation TEXT NOT NULL,
relied_evaluation TEXT NOT NULL UNIQUE,
certainty TEXT NOT NULL,
grounds TEXT NULL,
started_at TEXT NOT NULL
);
CREATE TABLE activity_record (
activity_id TEXT PRIMARY KEY,
companion_id TEXT NOT NULL,
kind TEXT NOT NULL,
task_id TEXT NULL,
task_revision INTEGER NULL,
purpose_adopted_revision INTEGER NULL,
body TEXT NOT NULL,
created_at TEXT NOT NULL,
command_id TEXT NULL UNIQUE
);
CREATE TABLE companion (
companion_id TEXT PRIMARY KEY,
lifecycle TEXT NOT NULL,
created_at TEXT NOT NULL
);
CREATE TABLE consent_record (
capability TEXT PRIMARY KEY,
id TEXT NOT NULL,
rev INTEGER NOT NULL,
provider TEXT NOT NULL,
model TEXT NOT NULL,
credential_id TEXT NOT NULL
);
CREATE TABLE credential_pending (
provider TEXT NOT NULL,
label TEXT NOT NULL,
requested_at TEXT NOT NULL,
PRIMARY KEY (provider, label)
);
CREATE TABLE credential_ref (
id TEXT PRIMARY KEY,
provider TEXT NOT NULL,
label TEXT NOT NULL,
UNIQUE (provider, label)
);
CREATE TABLE credential_set (
id INTEGER PRIMARY KEY CHECK (id = 1),
rev INTEGER NOT NULL
);
-- Credential publication (Stage 7 A1c): one row per registration attempt and
-- one row per credential's active version. No secret value, hash, or
-- encryption body is stored here: the OS item holds the value, and this table
-- holds only the non-secret references the recovery path reconciles.
CREATE TABLE credential_mutation (
mutation_id TEXT PRIMARY KEY,
op TEXT NOT NULL,
provider TEXT NOT NULL,
label TEXT NOT NULL,
expected_revision INTEGER NULL,
candidate_version INTEGER NULL,
phase TEXT NOT NULL,
decided_outcome TEXT NULL,
created_at TEXT NOT NULL
);
CREATE TABLE credential_active (
provider TEXT NOT NULL,
label TEXT NOT NULL,
active_version INTEGER NULL,
PRIMARY KEY (provider, label)
);
-- Retired credential versions whose OS item removal is not yet confirmed.
-- One row per retired version, written in the same transaction that moves the
-- active pointer (activation/rotation) or clears it (revocation), so a later
-- update can never overwrite a pending retirement: the set accumulates and a
-- bounded cleanup pass drains it. The row is the durable `pending` cleanup
-- state; it is deleted only in the transaction that records the confirmed
-- removal and completes the retiring mutation, so a crash between the OS
-- erase and the state write leaves the row and the erase is re-attempted
-- (idempotently). Non-secret references only: the OS item holds the value.
CREATE TABLE credential_retired (
provider TEXT NOT NULL,
label TEXT NOT NULL,
version INTEGER NOT NULL,
mutation_id TEXT NOT NULL,
retired_at TEXT NOT NULL,
PRIMARY KEY (provider, label, version)
);
CREATE INDEX idx_credential_retired_mutation ON credential_retired (mutation_id);
CREATE TABLE delegation (
delegation_id TEXT PRIMARY KEY,
task_id TEXT NOT NULL,
task_revision INTEGER NOT NULL,
delegator TEXT NOT NULL,
agent TEXT NOT NULL,
scope_assoc TEXT NULL,
scope_folder TEXT NULL,
scope_save_target TEXT NULL
);
CREATE TABLE deletion_operation (
operation_id TEXT PRIMARY KEY,
request_id TEXT NULL UNIQUE,
sweep INTEGER NOT NULL CHECK (sweep > 0),
phase TEXT NOT NULL CHECK (phase IN ('active', 'held', 'finalizing', 'completed')),
purpose TEXT NOT NULL CHECK (purpose IN ('privacy', 'security')),
started_at TEXT NOT NULL,
hold_reason TEXT NULL CHECK (hold_reason IN ('unavailable', 'generation_exhausted')),
erased_total INTEGER NOT NULL DEFAULT 0 CHECK (erased_total >= 0),
CHECK ((phase = 'held') = (hold_reason IS NOT NULL))
);
-- Staged Targeted Deletion requests awaiting the Host-local trusted
-- confirmation (IPC §18.1). Staging publishes no erasure condition and no
-- operation: the row is inert until the confirmation inlet records the
-- matching deletion_confirmation row and the canonical admission commits.
-- `exact_text` is protected operation-lifetime material: it is destroyed (set
-- NULL) by the A5 completion commit that admits the operation, and it is
-- never copied into audit rows or views.
CREATE TABLE deletion_request (
request_id TEXT PRIMARY KEY,
purpose TEXT NOT NULL CHECK (purpose IN ('privacy', 'security')),
exact_text TEXT NULL CHECK (exact_text IS NULL OR length(exact_text) > 0),
requested_at TEXT NOT NULL
);
-- Durable Owner confirmation facts, written only by the Host-local trusted
-- first-party inlet. A confirmation names its request identity: it can never
-- be replayed onto another target or purpose, and a request starts at most
-- one operation (deletion_operation.request_id is UNIQUE).
CREATE TABLE deletion_confirmation (
request_id TEXT PRIMARY KEY,
confirmed_at TEXT NOT NULL
);
CREATE TABLE deletion_search_material (
operation_id TEXT PRIMARY KEY,
exact_text TEXT NOT NULL CHECK (length(exact_text) > 0)
);
-- Required participant snapshot plus current progress (lifecycle §8-§10).
-- Every operation carries a non-empty set from admission; every row tracks the
-- operation's current sweep (NextSweep resets progress to pending in the same
-- transaction that advances the generation); a completed operation has every
-- row verified for the final sweep. Progress is keyed by the stable owner name
-- (a semantic owner class or `client_incarnation:<uuid>`), so a replacement
-- Client incarnation never inherits another incarnation's status.
CREATE TABLE deletion_participant (
operation_id TEXT NOT NULL,
participant_owner TEXT NOT NULL,
state TEXT NOT NULL CHECK (state IN ('pending', 'running', 'local_complete', 'verified', 'held')),
sweep INTEGER NOT NULL CHECK (sweep > 0),
hold_class TEXT NULL CHECK (hold_class IN ('unavailable', 'unsupported', 'failed')),
erased_count INTEGER NOT NULL CHECK (erased_count >= 0),
remainder_count INTEGER NOT NULL CHECK (remainder_count >= 0),
reported_at TEXT NULL,
PRIMARY KEY (operation_id, participant_owner),
CHECK ((state = 'held') = (hold_class IS NOT NULL)),
CHECK (state != 'verified' OR remainder_count = 0),
CHECK ((state = 'pending') = (reported_at IS NULL))
);
CREATE TABLE deletion_semantic_hint (
operation_id TEXT NOT NULL,
ordinal INTEGER NOT NULL,
material TEXT NOT NULL,
PRIMARY KEY (operation_id, ordinal)
);
CREATE TABLE erasure_condition (
operation_id TEXT NOT NULL,
sweep INTEGER NOT NULL CHECK (sweep > 0),
opened_at TEXT NOT NULL,
closed_at TEXT NULL,
PRIMARY KEY (operation_id, sweep)
);
-- Current-sweep canonical source correlation for the erasure-currentness hot
-- path (AU14 / Task resume). Unfinished operations keep source rows only in
-- their current sweep: NextSweep copies the current sweep forward and then
-- deletes the old sweep in the same transaction. Historical erasure_condition
-- rows remain as lifecycle/history but carry no source rows. Completed
-- operations keep zero source rows: the completion boundary (A5; A1 has no
-- completion authority) must delete them, and any remaining row fails closed.
-- No second copy is kept for audit/history.
CREATE TABLE erasure_condition_source (
operation_id TEXT NOT NULL,
sweep INTEGER NOT NULL CHECK (sweep > 0),
source TEXT NOT NULL,
PRIMARY KEY (operation_id, sweep, source)
);
-- Durable correspondence between an already-claimed use and the deletion
-- operation whose condition committed after the claim (lifecycle §11 R2).
-- Written inside the admission transaction for every in-flight inference
-- attempt / unsealed task delegation / in-flight Learning formation whose
-- durable provenance intersects the operation's covered sources; the work
-- kinds are the closed set of claims that can produce a delayed
-- target-bearing body. One use may correspond to several operations: two
-- unfinished Targeted Deletion operations on different exact targets can
-- share one delegation or one claim, and each association must survive.
-- Unlike the operation-lifetime search material, a hold deliberately
-- outlives completion: a delayed result from a use that started before the
-- condition must still be refused after the operation completed and
-- `closed_at` is set. A hold is objective metadata only -- the claim
-- identity, the operation identity, and the hold time -- never a target
-- body, a reversible encoding, a target hash/fingerprint, or a search
-- token, so it can never become a keyword ban, a reusable matcher, or a
-- work item's permanent text blacklist; a claim is single-use, so the row
-- loses its force once that claim settles.

CREATE TABLE erasure_use_hold (
use_kind TEXT NOT NULL CHECK (use_kind IN ('inference_attempt', 'task_delegation', 'learning_formation')),
use_id TEXT NOT NULL,
operation_id TEXT NOT NULL,
held_at TEXT NOT NULL,
PRIMARY KEY (use_kind, use_id, operation_id)
);
-- Body-free in-flight Learning formation identity. Published when a
-- Host-transient ExperienceCandidate is taken off the formation queue,
-- before the Learning inference claim exists. Never stores transcript
-- text, a target body, a hash, or a fingerprint. Source rows name the
-- same History identities the later claim's data_use will carry, so
-- admission can associate this execution with a deletion interval. The
-- identity is settled when the pass converts to a claim or is dropped;
-- correspondence rows in erasure_use_hold outlive that settle.
CREATE TABLE learning_formation (
formation_id TEXT PRIMARY KEY,
companion_id TEXT NOT NULL,
started_at TEXT NOT NULL
);
CREATE TABLE learning_formation_source (
formation_id TEXT NOT NULL,
source TEXT NOT NULL,
PRIMARY KEY (formation_id, source)
);
CREATE INDEX idx_learning_formation_source_source ON learning_formation_source (source);
-- Body-free completion audit (lifecycle §13). Written exactly once, in the
-- same transaction that destroys the operation's protected material and closes
-- the current condition; the rows carry only objective metadata -- the
-- operation identity, purpose class, times, sweep count, and per-participant
-- final status/counts -- and never the target body, a reversible encoding, a
-- target hash/fingerprint, a search token, a credential value, or a
-- prompt/output body. A completed operation has exactly one audit row; an
-- unfinished operation has none (the completion commit is atomic).
CREATE TABLE deletion_completion_audit (
operation_id TEXT PRIMARY KEY,
purpose TEXT NOT NULL CHECK (purpose IN ('privacy', 'security')),
started_at TEXT NOT NULL,
completed_at TEXT NOT NULL,
sweep_count INTEGER NOT NULL CHECK (sweep_count > 0),
participant_count INTEGER NOT NULL CHECK (participant_count > 0),
verified_count INTEGER NOT NULL CHECK (verified_count >= 0),
erased_count INTEGER NOT NULL CHECK (erased_count >= 0)
);
-- One audit entry per required participant of the durable snapshot at
-- completion. `final_state` is `verified` only: a completed operation has no
-- held or pending participant, so a hold can never be audited as success. A
-- hold class is unfinished-status metadata and lives only in
-- `deletion_participant`, where a completed operation keeps no row with one.
CREATE TABLE deletion_audit_participant (
operation_id TEXT NOT NULL,
participant_owner TEXT NOT NULL,
final_state TEXT NOT NULL CHECK (final_state = 'verified'),
erased_count INTEGER NOT NULL CHECK (erased_count >= 0),
PRIMARY KEY (operation_id, participant_owner)
);
CREATE TABLE history_message (
message_id TEXT PRIMARY KEY,
companion_id TEXT NOT NULL,
round_id TEXT NOT NULL,
role TEXT NOT NULL,
body TEXT NOT NULL,
lang TEXT NOT NULL,
at TEXT NOT NULL,
presence_generation INTEGER NOT NULL,
local_id TEXT NULL,
command_id TEXT NULL,
round_wire TEXT NULL,
client_counter INTEGER NULL,
client_random INTEGER NULL,
round_intent TEXT NULL,
round_intent_ref TEXT NULL,
at_utc TEXT NULL
);
CREATE TABLE inference_attempt (
ticket TEXT PRIMARY KEY,
consent_id TEXT NOT NULL,
consent_rev INTEGER NOT NULL,
provider TEXT NOT NULL,
model TEXT NOT NULL,
started_at TEXT NOT NULL,
capability TEXT NOT NULL,
consumer TEXT NULL,
purpose TEXT NULL,
credential_set_rev INTEGER NULL,
delegation_id TEXT NULL,
task_id TEXT NULL,
task_revision INTEGER NULL,
data_use_count INTEGER NULL,
pricing_snapshot TEXT NULL
);
CREATE TABLE inference_attempt_data_use (
ticket TEXT NOT NULL,
ordinal INTEGER NOT NULL,
source TEXT NOT NULL,
PRIMARY KEY (ticket, ordinal)
);
CREATE TABLE learning_memory (
memory_id TEXT PRIMARY KEY,
companion_id TEXT NOT NULL,
revision INTEGER NOT NULL,
content TEXT NOT NULL,
importance INTEGER NOT NULL,
temporal TEXT NOT NULL,
recall_suppressed INTEGER NOT NULL,
updated_at TEXT NOT NULL
);
CREATE TABLE learning_memory_revision (
memory_id TEXT NOT NULL,
revision INTEGER NOT NULL,
companion_id TEXT NOT NULL,
content TEXT NOT NULL,
importance INTEGER NOT NULL,
temporal TEXT NOT NULL,
recall_suppressed INTEGER NOT NULL,
change_kind TEXT NOT NULL,
summary_id TEXT,
at TEXT NOT NULL,
PRIMARY KEY (memory_id, revision)
);
CREATE TABLE learning_memory_term (
term TEXT NOT NULL,
memory_id TEXT NOT NULL,
companion_id TEXT NOT NULL,
PRIMARY KEY (companion_id, term, memory_id)
);
CREATE TABLE learning_summary (
summary_id TEXT PRIMARY KEY,
companion_id TEXT NOT NULL,
content TEXT NOT NULL,
source_kind TEXT NOT NULL,
source_start TEXT NOT NULL,
source_end TEXT NOT NULL,
formed_at TEXT NOT NULL
);
CREATE TABLE management_intent (
intent_id TEXT PRIMARY KEY,
kind TEXT NOT NULL,
target TEXT NOT NULL,
base TEXT NOT NULL,
rationale_origin TEXT NOT NULL,
rationale_quote TEXT NULL,
outcome TEXT NOT NULL,
mark TEXT NULL
);
CREATE TABLE paired_device (
device_id TEXT PRIMARY KEY,
descriptor TEXT NOT NULL,
paired_at TEXT NOT NULL,
wire TEXT NULL
);
CREATE TABLE pairing_pending (
pending_id TEXT PRIMARY KEY,
descriptor TEXT NOT NULL,
requested_at TEXT NOT NULL,
origin_connection TEXT NOT NULL
);
-- Reviewed provider rates, immutable per (provider, model, revision). The id
-- is derived from the reviewed content, so the same revision resolves to the
-- same durable identity in every process; a row that disagrees with the
-- revision it was published under fails closed instead of repricing history.
-- Rates are exact micro-currency units per 1,000,000 tokens.
CREATE TABLE pricing_snapshot (
id TEXT PRIMARY KEY,
provider TEXT NOT NULL,
model TEXT NOT NULL,
currency TEXT NOT NULL,
input_rate INTEGER NOT NULL CHECK (input_rate >= 0),
cached_input_rate INTEGER NOT NULL CHECK (cached_input_rate >= 0),
output_rate INTEGER NOT NULL CHECK (output_rate >= 0),
effective_at TEXT NOT NULL,
source_revision INTEGER NOT NULL CHECK (source_revision >= 0),
UNIQUE (provider, model, source_revision)
);
CREATE TABLE presence_attribution (
companion_id TEXT PRIMARY KEY,
state TEXT NOT NULL,
active_client TEXT,
generation INTEGER NOT NULL
);
CREATE TABLE presence_transition_log (
transition_seq INTEGER PRIMARY KEY AUTOINCREMENT,
companion_id TEXT NOT NULL,
old_state TEXT NOT NULL,
new_state TEXT NOT NULL,
old_gen INTEGER NOT NULL,
new_gen INTEGER NOT NULL,
reason TEXT NOT NULL,
at TEXT NOT NULL
);
CREATE TABLE relocation_hint (
companion_id TEXT PRIMARY KEY,
last_client TEXT,
recovery_destination TEXT
);
CREATE TABLE task (
task_id TEXT PRIMARY KEY,
revision INTEGER NOT NULL,
purpose_adopted_revision INTEGER NOT NULL,
purpose_text TEXT NOT NULL,
assignee TEXT NOT NULL,
progress TEXT NULL
);
CREATE TABLE task_context_entry (
entry_id TEXT PRIMARY KEY,
task_id TEXT NOT NULL,
revision INTEGER NOT NULL,
item_kind TEXT NOT NULL,
purpose_adopted_revision INTEGER NULL,
origin_kind TEXT NOT NULL,
origin_source TEXT NOT NULL,
acquired_at TEXT NOT NULL
);
CREATE TABLE task_result (
result_id TEXT PRIMARY KEY,
task_id TEXT NOT NULL,
task_revision INTEGER NOT NULL,
delegation_id TEXT NOT NULL UNIQUE,
body TEXT NOT NULL,
adopted_revision INTEGER NULL,
recorded_at TEXT NOT NULL
);
CREATE TABLE task_result_attempt (
result_id TEXT NOT NULL,
attempt_id TEXT NOT NULL,
PRIMARY KEY (result_id, attempt_id)
);
CREATE TABLE task_revision (
task_id TEXT NOT NULL,
revision INTEGER NOT NULL,
purpose_adopted_revision INTEGER NOT NULL,
purpose_text TEXT NOT NULL,
assignee TEXT NOT NULL,
PRIMARY KEY (task_id, revision)
);
CREATE TABLE undelivered (
row_seq INTEGER PRIMARY KEY AUTOINCREMENT,
undelivered_id TEXT NOT NULL UNIQUE,
companion_id TEXT NOT NULL,
source_kind TEXT NOT NULL,
source_id TEXT NOT NULL,
source_phase TEXT NOT NULL,
status TEXT NOT NULL,
round_id TEXT NULL,
presence_generation INTEGER NULL,
created_at TEXT NOT NULL,
UNIQUE (companion_id, source_kind, source_id, source_phase)
);
CREATE TABLE usage_fact (
ticket TEXT PRIMARY KEY,
provider TEXT NOT NULL,
model TEXT NOT NULL,
input_tokens INTEGER,
cached_input_tokens INTEGER,
output_tokens INTEGER,
source TEXT NOT NULL,
pricing_snapshot TEXT NULL,
CHECK (
    (source = 'unknown' AND input_tokens IS NULL AND cached_input_tokens IS NULL AND output_tokens IS NULL)
    OR
    (source = 'reported' AND input_tokens IS NOT NULL AND cached_input_tokens IS NOT NULL AND output_tokens IS NOT NULL
        AND input_tokens >= 0 AND cached_input_tokens >= 0 AND output_tokens >= 0
        AND cached_input_tokens <= input_tokens)
)
);
-- Provider / system cost caps (usage-cost-cap §6/§13). One row per
-- (scope, window): `provider` is '' for the system scope and the exact
-- provider name for a provider scope. `revision` is the currentness premise
-- the compare-and-set update and the send admission serialize on; the limit
-- is an exact micro-currency amount in `currency`.
CREATE TABLE usage_cap (
scope TEXT NOT NULL CHECK (scope IN ('system', 'provider')),
provider TEXT NOT NULL,
window TEXT NOT NULL CHECK (window IN ('daily_utc', 'monthly_utc')),
revision INTEGER NOT NULL CHECK (revision >= 0),
currency TEXT NOT NULL,
limit_micros INTEGER NOT NULL CHECK (limit_micros > 0),
PRIMARY KEY (scope, provider, window),
CHECK ((scope = 'provider') = (length(provider) > 0))
);
-- One usage reservation per claimed provider call (usage-cost-cap §7/§8).
-- The row is written inside the same Immediate transaction as the attempt
-- claim and its cap compare, before any provider byte. `currency` +
-- `upper_bound_micros` is the reserved cap amount: a `committed_reported` row
-- replaces it with the actual cost, `committed_unknown` keeps the upper bound
-- counted, and `released` counts nothing. `opened_at` is canonical UTC text;
-- a cap window is the UTC period containing it, and settlement never moves
-- the row between windows.
CREATE TABLE usage_reservation (
reservation_id TEXT PRIMARY KEY,
ticket TEXT NOT NULL UNIQUE,
provider TEXT NOT NULL CHECK (length(provider) > 0),
model TEXT NOT NULL,
pricing_snapshot TEXT NOT NULL,
currency TEXT NOT NULL,
upper_bound_micros INTEGER NOT NULL CHECK (upper_bound_micros >= 0),
state TEXT NOT NULL CHECK (state IN ('reserved', 'committed_reported', 'committed_unknown', 'released')),
committed_currency TEXT NULL,
committed_micros INTEGER NULL CHECK (committed_micros IS NULL OR committed_micros >= 0),
opened_at TEXT NOT NULL,
CHECK ((state = 'committed_reported') = (committed_micros IS NOT NULL)),
CHECK (committed_micros IS NULL OR committed_currency IS NOT NULL)
);
CREATE TABLE workspace_assoc (
assoc_id TEXT PRIMARY KEY,
task_id TEXT NOT NULL,
folder TEXT NOT NULL,
save_target TEXT
);
CREATE INDEX idx_action_attempt_delegation ON action_attempt (delegation_id);
CREATE INDEX idx_action_attempt_task ON action_attempt (task_id);
CREATE INDEX idx_erasure_condition_source_source ON erasure_condition_source (source);
-- The per-read unfinished-operation probe and the keyset walk read only the
-- unfinished set. Completed operations are retained forever (lifecycle §13),
-- so without this partial index the probe scans every completed row.
CREATE INDEX idx_deletion_operation_unfinished ON deletion_operation (operation_id)
 WHERE phase != 'completed';
-- Admission associates already-claimed uses by joining their ordered source
-- correlation against the operation's covered sources; this index serves that
-- probe from the (bounded) covered set instead of scanning every attempt's
-- correlation rows.
CREATE INDEX idx_inference_attempt_data_use_source ON inference_attempt_data_use (source);
CREATE INDEX idx_history_message_companion ON history_message (companion_id);
-- The round lookup reads one (companion, wire) row; the composite index
-- keeps that a bounded seek instead of a companion-history scan.
CREATE INDEX idx_history_message_companion_wire ON history_message (companion_id, round_wire);
CREATE UNIQUE INDEX idx_history_message_companion_command ON history_message (companion_id, command_id);
CREATE INDEX idx_history_message_companion_role ON history_message (companion_id, role);
CREATE INDEX idx_inference_attempt_delegation ON inference_attempt (delegation_id);
-- Bounded first-party usage summary (usage-cost-cap §16): the newest-first
-- keyset page reads this index, so the SQL LIMIT bounds the rows read and no
-- full scan or sort is needed for an unfiltered range.
CREATE INDEX idx_inference_attempt_started ON inference_attempt (started_at, ticket);
CREATE INDEX idx_learning_memory_companion ON learning_memory (companion_id);
CREATE INDEX idx_learning_memory_recall_importance ON learning_memory (companion_id, importance DESC) WHERE recall_suppressed = 0;
CREATE INDEX idx_learning_memory_recall_newest ON learning_memory (companion_id) WHERE recall_suppressed = 0;
CREATE INDEX idx_learning_memory_term_memory ON learning_memory_term (memory_id);
-- The system-wide remainder probe tests `term = ?1` inside the completion
-- transaction; the composite primary key cannot seek on `term` alone, so this
-- index keeps that probe a bounded lookup instead of a whole-table walk.
CREATE INDEX idx_learning_memory_term_term ON learning_memory_term (term);
CREATE UNIQUE INDEX idx_paired_device_wire ON paired_device (wire);
CREATE INDEX idx_task_context_entry_task ON task_context_entry (task_id, revision);
CREATE INDEX idx_task_result_task ON task_result (task_id, result_id);
CREATE INDEX idx_task_result_unadopted ON task_result (recorded_at, result_id) WHERE adopted_revision IS NULL;
CREATE INDEX idx_undelivered_companion_status ON undelivered (companion_id, status, row_seq);
-- The system-wide remainder probe tests `NOT EXISTS ... (source_kind,
-- source_id)` inside the completion transaction; no other index leads with
-- those columns, so this keeps the probe bounded.
CREATE INDEX idx_undelivered_source ON undelivered (source_kind, source_id);
CREATE INDEX idx_usage_reservation_opened ON usage_reservation (opened_at);
CREATE INDEX idx_usage_reservation_provider_opened ON usage_reservation (provider, opened_at);
CREATE INDEX idx_workspace_assoc_task ON workspace_assoc (task_id);
-- Body-free occurrence ledger of Task Agent execution-local observations
-- (Stage 6 A4). The execution mints one occurrence identity at observation
-- time and the row carries the delegation/execution correlation, the
-- producing AU5 action attempt where one exists, and the workspace/path
-- correlation. `body_observed` means the observation reproduced workspace
-- content (read bytes or a list listing). The ledger stores no body and no
-- content-version identity, so a later clean read of the mutable path cannot
-- prove the discarded body was unrelated to a deletion target: admission
-- treats such an occurrence as covered (fail closed). The table deliberately
-- stores no observation body, no body hash or fingerprint, no reversible
-- encoding, and no presentation copy: the observation text stays
-- execution-local and only this correlation ledger is durable. `path` is the
-- resolved AU5 target and is a mechanical-erasure column of the Task owner,
-- exactly like `action_attempt.real_target`.
CREATE TABLE task_agent_observation (
observation_id TEXT PRIMARY KEY,
delegation_id TEXT NOT NULL,
task_id TEXT NOT NULL,
task_revision INTEGER NOT NULL,
workspace_assoc_id TEXT NULL,
action_attempt_id TEXT NULL,
path TEXT NULL,
body_observed INTEGER NOT NULL CHECK (body_observed IN (0, 1)),
observed_at TEXT NOT NULL,
CHECK ((path IS NULL) = (workspace_assoc_id IS NULL)),
CHECK ((workspace_assoc_id IS NULL) = (action_attempt_id IS NULL)),
CHECK (body_observed = 0 OR action_attempt_id IS NOT NULL)
);
CREATE INDEX idx_task_agent_observation_delegation ON task_agent_observation (delegation_id);
INSERT INTO credential_set (id, rev) VALUES (1, 0);
-- Durable, body-free Client body-delivery evidence (lifecycle §8.1). One row
-- per Host-minted Client incarnation the Host actually handed body-bearing
-- material to. `delivery_seq` advances on every such delivery and is the CAS
-- premise a verified local-erasure result clears on: a delivery that raced a
-- wipe leaves a higher sequence and the row survives. The row keeps only the
-- incarnation identity, the delivery sequence, and the delivery times --
-- never a target body, a reversible encoding, a body hash/fingerprint, a
-- deletion matcher/search token, a presentation copy, or a device secret.
-- A row is created only from an authenticated connection's pinned
-- incarnation, so the Host never invents an owner it cannot name. A Host
-- restart never removes a row; only a verified full-class local-erasure
-- result (clearing the exact observed sequence) may clear it.
CREATE TABLE client_delivery_evidence (
incarnation_id TEXT PRIMARY KEY,
delivery_seq INTEGER NOT NULL CHECK (delivery_seq > 0),
first_delivered_at TEXT NOT NULL,
last_delivered_at TEXT NOT NULL
);
-- Exhaustive covered-source reconciliation state (lifecycle §4.1 point 4,
-- §12 step 1). One durable keyset cursor per (operation, current sweep,
-- known identity table): `cursor` is the last canonical identity published
-- from that table, and `complete=1` means the ordered identity scan reached
-- its end for this sweep. Admission writes the rows and publishes a first
-- bounded page; bounded reconciliation steps continue from the cursor, so no
-- page bound can drop a covered identity. A new sweep deletes the old rows and
-- inserts fresh incomplete ones (a generation is never reused); the completion
-- commit deletes them together with the rest of the operation-lifetime
-- protected state. A completed operation keeps zero rows.
CREATE TABLE deletion_reconciliation (
 operation_id TEXT NOT NULL,
 sweep INTEGER NOT NULL CHECK (sweep > 0),
 identity_table TEXT NOT NULL,
 cursor TEXT NOT NULL,
 complete INTEGER NOT NULL CHECK (complete IN (0, 1)),
 PRIMARY KEY (operation_id, sweep, identity_table)
);
-- Fair scheduling position of each bounded walk over the unfinished deletion
-- operations (lifecycle §14). One row per walk owner; `after_id` is the last
-- operation id the previous pass examined, so a pass starts after it instead
-- of at the smallest id and the tail of a set larger than one pass bound is
-- reached on later passes (the walk wraps to the head at the end). The row is
-- a scheduling position only: it is never phase, participant, verification,
-- condition, or completion truth, and a stale position changes nothing
-- because each visit re-derives those from `deletion_operation` /
-- `deletion_participant` / `erasure_condition`. The row holds an identity
-- only: no target body, matcher material, or count.
CREATE TABLE deletion_walk_cursor (
 walk TEXT PRIMARY KEY CHECK (walk IN ('fan_out', 'retryable_hold')),
 after_id TEXT NOT NULL
);
-- A reconciliation page associates already-claimed uses whose Task context
-- origin names one of the page's covered identities; this index drives that
-- probe from the (bounded) page instead of scanning every context entry.
CREATE INDEX idx_task_context_entry_origin_source ON task_context_entry (origin_source);
";

pub(super) fn run(conn: &mut Connection) -> Result<(), String> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    let stored: i64 = tx
        .query_row("PRAGMA user_version", (), |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if stored == CURRENT_VERSION {
        return Ok(());
    }
    if stored != 0 {
        return Err(String::from("unsupported schema version"));
    }
    let populated: bool = tx
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%')",
            (),
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if populated {
        return Err(String::from("unversioned database is not empty"));
    }
    tx.execute_batch(SCHEMA)
        .map_err(|error| error.to_string())?;
    tx.pragma_update(None, "user_version", CURRENT_VERSION)
        .map_err(|error| error.to_string())?;
    tx.commit().map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialization_is_idempotent_and_rejects_representative_unsupported_versions() {
        let mut current = Connection::open_in_memory().unwrap();
        run(&mut current).unwrap();
        current
            .execute("UPDATE credential_set SET rev = 7", ())
            .unwrap();
        let current_changes = current.total_changes();
        run(&mut current).unwrap();
        assert_eq!(current.total_changes(), current_changes);
        assert_eq!(
            current
                .query_row("SELECT rev FROM credential_set", (), |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            7
        );

        for version in [-1, 1, CURRENT_VERSION - 1] {
            let mut conn = Connection::open_in_memory().unwrap();
            conn.pragma_update(None, "user_version", version).unwrap();
            let changes = conn.total_changes();
            assert_eq!(
                run(&mut conn),
                Err(String::from("unsupported schema version"))
            );
            assert_eq!(
                conn.query_row("PRAGMA user_version", (), |row| row.get::<_, i64>(0))
                    .unwrap(),
                version
            );
            assert_eq!(conn.total_changes(), changes);
        }

        let mut unversioned = Connection::open_in_memory().unwrap();
        unversioned
            .execute_batch("CREATE TABLE preexisting(value TEXT)")
            .unwrap();
        let changes = unversioned.total_changes();
        assert_eq!(
            run(&mut unversioned),
            Err(String::from("unversioned database is not empty"))
        );
        assert_eq!(
            unversioned
                .query_row("PRAGMA user_version", (), |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(unversioned.total_changes(), changes);
    }

    #[test]
    fn failed_atomic_initialization_can_be_retried() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("atomic.db");
        let reader = Connection::open(&path).unwrap();
        reader
            .execute_batch("BEGIN; SELECT * FROM sqlite_schema;")
            .unwrap();
        let mut writer = Connection::open(&path).unwrap();
        writer.busy_timeout(std::time::Duration::ZERO).unwrap();
        assert!(run(&mut writer).is_err());
        assert_eq!(
            writer
                .query_row("SELECT COUNT(*) FROM sqlite_schema", (), |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );
        assert_eq!(
            writer
                .query_row("PRAGMA user_version", (), |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
        reader.execute_batch("ROLLBACK").unwrap();
        run(&mut writer).unwrap();
        assert_eq!(
            writer
                .query_row("PRAGMA user_version", (), |row| row.get::<_, i64>(0))
                .unwrap(),
            CURRENT_VERSION
        );
    }
}
