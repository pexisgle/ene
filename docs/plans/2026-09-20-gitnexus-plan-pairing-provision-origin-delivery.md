# GitNexus Engineering Plan

> Task: Implement IPC §9.2 origin-bound pairing provision delivery and remove polling/bootstrap secret paths.
> Evidence verified at commit `8bf8445d7e3d2a40c2871b512a48a4bf01a0e356`; GitNexus index fresh at that exact commit with PDG data (runner `node .gitnexus/run.cjs`, GitNexus 1.6.12, analyzer identity verified current).
> Evidence provenance schema 2; global dirty digest `119a8063e9b06a6e0564b41628fafd3c0d516e7fcf80ac917efa597719f99e81`; cited-path manifest 46 sorted entries; exact generated plan path excluded.
> Worktree note: the manifest pins unstaged user changes in `docs/implementation/PROGRESS.md` and `docs/implementation/stages/stage-7.md`; the repository-wide digest also covers every other dirty path. Those edits are evidence to preserve, not work to replace.

## 1. Objective

[verified] Implement the remaining Stage 7 A1 gap described at `docs/implementation/stages/stage-7.md:222-226`: first-pairing authentication material must travel in an authentication-specific `PairingProvision` frame to the same live Client connection that opened the pending request. It must never be returned through requester/confirmation outcomes, stdout, ordinary business DTOs, environment bootstrap, durable pending polling, or a replacement connection after disconnect (`docs/design/concrete/host-client-ipc.md:305-323`, `docs/design/concrete/first-party-desktop.md:207-209`).

The completed slice keeps secret custody bounded and zeroizing, serializes phase mutation and socket output in the connection owner, preserves post-auth client persistence, and reports durable approval separately from transport receipt. Backward compatibility with the polling/bootstrap implementation is explicitly out of scope.

## 2. Current Behaviour

- [verified] `PairingRequest` carries an optional polling ID and `PairingResult::Paired` returns the device ID as an ordinary request response (`crates/ene-api/src/v1/handshake.rs:12-41`); both are normal `WirePayload` variants (`crates/ene-api/src/v1/payload.rs:42-50`).
- [verified] `Client::connect_with_bootstrap` loads `client-pending.json`, sends a poll, returns and drops the first socket on `PendingOwnerConfirmation`, then later accepts an environment/in-process secret and persists it only after successful authentication (`crates/ene-client/src/transport.rs:110-299`; `crates/ene-client/src/device.rs:45-54,121-134`).
- [verified] Host pairing resolves an approved pending from any later connection, calls `note_paired`, and emits `PairingResult::Paired`; a new request only returns the pending ID (`apps/ene-core/src/serve/handshake.rs:35-115`).
- [verified] Store polling resolves `paired_device.pending_id` from any connection, while reapproval of a consumed pending mints a fresh secret for the same device (`crates/ene-store/src/credential.rs:335-440,444-519`; schema at `crates/ene-store/src/migrate.rs:340-352`).
- [verified] `HostHandle::approve_device` has an absent-pending fallback for rotation, saves the returned secret in the Host auth store, and returns `(DeviceRecord, String)` (`apps/ene-core/src/serve.rs:2050-2104`). `execute_pending` wraps that string in `ControlOutcome::DeviceApproved`, and the desktop clones it to reconnect (`apps/ene-core/src/host_control.rs:663-689`; `apps/ene-desktop/src/ui/runtime.rs:353-441`).
- [verified] `RequesterOutcome::DeviceApproved` is already non-secret, but `ControlOutcome::DeviceApproved` still carries `RedactedSecret` (`crates/ene-local-control/src/lib.rs:138-168,217-246`).
- [verified] `serve_connection` registers wakeups before reads, owns the split socket's write half, uses bounded channels, and funnels final cleanup through `HostHandle::close_connection` (`apps/ene-core/src/conn.rs:1315-1600`). Encoded and decoded `Vec<u8>` frame bodies are not presently zeroizing (`apps/ene-core/src/conn.rs:1149-1204`; `crates/ene-client/src/transport.rs:594-629`).

## 3. Relevant Architecture

- [verified] IPC §9.2 requires pairing material to use an authentication-only frame, and §9.3 requires the one-way `Accepted → Paired → Challenged → Authenticated` phase order (`docs/design/concrete/host-client-ipc.md:291-323`). `ConnectionTable::note_paired` is therefore the phase authority; delivery code must not invent a parallel state machine (`apps/ene-core/src/conn.rs:266-311`).
- [verified] The first-party desktop contract separates the public requester listener, the private Owner-confirmation channel, and the Client channel. Pairing material belongs only to the original Client channel; requester/control completion may carry non-secret facts (`docs/design/concrete/first-party-desktop.md:140-151,182-209`).
- [verified] `serve_connection` is the established single-writer boundary. Approval runs through `HostHandle` on the control path, so the safe bridge is a bounded connection-keyed queue, not a shared write half (`apps/ene-core/src/conn.rs:1328-1385`).
- [verified] `drop_connection_transient_state` is the single Host-memory cleanup boundary shared by close and supersession (`apps/ene-core/src/serve.rs:2415-2456`). The delivery slot belongs there; durable unapproved pending cleanup is an explicit repository operation from close, not a side effect of a read.
- [verified] Client device persistence already occurs after `AuthResult::Accepted`; the new flow should change the secret source while retaining that ordering (`crates/ene-client/src/transport.rs:223-290`).

## 4. GitNexus Findings

- [graph] `gitnexus.impact {target:"Client::connect_with_bootstrap", direction:"upstream", maxDepth:3}` reported **HIGH**, 27 impacted symbols. Its two depth-1 dependents are desktop `session::connect` and `Client::connect`; both are included in the cutover. Key output: `risk HIGH`.
- [graph] `gitnexus.impact {target:"HostHandle::approve_device", direction:"upstream", maxDepth:3}` reported **HIGH**, 46 impacted symbols. Its seven depth-1 callers are `host_control::execute_pending`, one `serve` test caller, and helper/callers in the five core E2E suites; every group is listed in §§8–10. Key output: `risk HIGH`.
- [graph] `gitnexus.impact {target:"DevicePairingRepository::approve_pending", direction:"upstream", maxDepth:3}` reported **CRITICAL**, 22 impacted symbols. Its five depth-1 dependents are `HostHandle::approve_device` and four store-test callers; the repository/API and those tests move together. Key output: `risk CRITICAL`.
- [graph] `gitnexus.impact {target:"serve_connection", direction:"upstream", maxDepth:3}` reported **HIGH** with five direct test callers in `apps/ene-core/src/conn/tests.rs`; all fixtures must register the new receiver before reads. Key output: `5 impacted at depth 1`.
- [graph] `gitnexus.context` located `connect_with_bootstrap` at `crates/ene-client/src/transport.rs:131-299`, `approve_device` at `apps/ene-core/src/serve.rs:2071-2104`, `approve_pending` at `crates/ene-credential/src/pairing.rs:139-143`, and `serve_connection` at `apps/ene-core/src/conn.rs:1315-1600`. Targeted source reads confirmed each current branch and caller contract.

## 5. Statement-Level PDG Findings

- [graph] `gitnexus.pdg_query {mode:"controls", target:"serve_connection"}` and `{mode:"flows", target:"serve_connection", variable:"connection"}` show the loop at `apps/ene-core/src/conn.rs:1348` controlling all select branches, with `connection` flowing through currentness checks and the close at line 1588. [inferred] Pairing delivery must be one more bounded receiver branch in this loop; moving writes or phase mutation into approval would create concurrent writers and split lifetime authority.
- [graph] `gitnexus.pdg_query` on `HostHandle::approve_device` shows the absent-origin branch at `apps/ene-core/src/serve.rs:2081-2093` controlling reapproval/rotation, and the approved value flowing to `auth_store.save_secret` and the secret-bearing return. [inferred] Removing that branch and consuming the secret into the delivery queue closes both alternate release paths.
- [graph] `gitnexus.pdg_query` on `Client::connect_with_bootstrap` shows the missing-device/secret branch entering pairing, the Pending arm storing a poll ID and returning, and resolved secret data flowing into `SessionState` and post-auth durable storage (`crates/ene-client/src/transport.rs:189-290`). [inferred] An owning `PendingPairingClient` must replace the return/drop boundary while retaining the accepted-authentication persistence guard.
- [graph] `gitnexus.explain` reported no persisted taint findings for `connect_with_bootstrap`, `approve_device`, `serve_connection`, or `host_control.rs`. This is not evidence of safety: closure/callback, property, and implicit flows are not completely modeled, so source-level custody and negative tests remain mandatory.

## 6. Proposed Changes

### 6.1 Protocol and secret representation

1. **`crates/ene-api/src/v1/handshake.rs` — `PairingRequest`, `PairingResult`, new `PairingProvision`/secret wrapper.** [verified] Remove `PairingRequest.pending_id` and `PairingResult::Paired`; keep `PendingOwnerConfirmation { pending_id }` and `Denied`. Add `PairingProvision { device_id, pairing_secret }` as authentication material. Its secret newtype must implement serde intentionally, custom redacted `Debug`, explicit zeroization, and zeroize-on-drop; derived `Debug` must never expose the inner bytes. Add `zeroize.workspace = true` in `crates/ene-api/Cargo.toml`.
2. **`crates/ene-api/src/v1/payload.rs` — `WirePayload`.** [verified] Add `PairingProvision` beside pairing/auth variants and return its canonical message type. It remains an auth-specific wire variant, never a management/business outcome.
3. **Framing custody.** [verified] Add `zeroize.workspace = true` to `crates/ene-client/Cargo.toml` and `apps/ene-core/Cargo.toml`; wrap secret-bearing encoded/decoded frame bodies in `Zeroizing<Vec<u8>>` (or an equivalent drop-zeroing owner). Move the provision secret between wrappers rather than cloning it; unavoidable `WirePayload::Clone` copies must retain redaction and zeroize-on-drop.

### 6.2 One-shot durable pairing contract

4. **`crates/ene-credential/src/pairing.rs` and re-exports — `DevicePairingRepository`.** [verified] Replace the polling-shaped `request_pairing(..., pending_id)` result with a fresh-request-only `PendingPairing`; remove `DevicePairingStatus` if it has no remaining role. Make `approve_pending(pending_id, origin)` a one-shot CAS returning an approved record plus a redacted/zeroizing domain secret. Add `abandon_pending_by_origin(origin)` for disconnect cleanup. Delete the documented rotation and “wire never carries secret” claims that conflict with IPC §9.2.
5. **`crates/ene-store/src/migrate.rs`, `credential.rs`, and erasure metadata.** [verified] Remove `paired_device.pending_id`, `SQL_SELECT_PAIRED_BY_PENDING`, poll branches, and the already-approved rotation branch. Approval must delete exactly the pending row matching `(pending_id, origin_connection)`, insert one paired device, and mint one secret in the same SQLite transaction; another approval returns `None`. `abandon_pending_by_origin` deletes only still-unapproved rows for that connection. This is a direct fresh-schema edit—no migration or legacy reader.

### 6.3 Live-origin delivery ownership

6. **New `apps/ene-core/src/pairing_delivery.rs` — `PairingDeliveryRegistry`.** [inferred] Add a Host-owned map keyed by `ConnectionWireId`. `register` creates a capacity-one Tokio channel before socket reads; `bind_pending` associates the fresh pending ID with that same slot; `claim` is a single-winner transition returning the recorded origin/sender; `queue` accepts exactly one zeroizing provision; `remove` drops sender/state on close or supersession. Keep lock hold times synchronous and short; never await, access SQLite, or write a socket while holding the registry lock.
7. **`apps/ene-core/src/serve/handshake.rs` — `HostHandle::pair`.** [verified] Validate peer/descriptor, create one fresh pending, and bind it to the pre-registered connection slot before returning `PendingOwnerConfirmation`. If registration/binding fails, delete/abandon that pending and return an operational denial; never emit `Paired` or mutate connection phase from this handler.
8. **`apps/ene-core/src/serve.rs` — `HostHandle::approve_device` and lifecycle cleanup.** [verified] Claim the pending's live slot first, pass that exact origin to the repository CAS, save the secret in `auth_store`, then enqueue `PairingProvision`. Return only the non-secret record/facts after queue acceptance. Delete the absent-origin rotation fallback. If store commit succeeds but auth-store save or queueing fails, return a technical unavailable result, zeroize local material, do not claim receipt, do not rotate/replay, and require a fresh pairing. Register delivery cleanup in `drop_connection_transient_state`; from `close_connection`, invoke origin-scoped pending abandonment without touching paired records.
9. **`apps/ene-core/src/conn.rs` — `serve_connection`.** [verified] Register the delivery receiver before spawning/starting reads and retain the original admitted `PairingRequest` frame/live template after writing the pending response. Add a biased select branch that dequeues only this connection's provision, verifies the retained template and current `Accepted` phase, calls `ConnectionTable::note_paired`, builds `PairingProvision` under the original correlation, and immediately writes it through the loop-owned `write_half`. Any phase mismatch, shutdown, encode failure, or write failure closes the connection and drops the material; it never puts the item back. Authentication—not a new delivery ACK—is the existing proof the client received and used the secret.

### 6.4 Remove control-channel and client bootstrap paths

10. **`crates/ene-local-control/src/lib.rs` and `apps/ene-core/src/host_control.rs`.** [verified] Remove `pairing_secret` from `ControlOutcome::DeviceApproved`. Both confirmation and requester outcomes retain `pending_id` and `device_id` as approval facts only. Document that `DeviceApproved` means the durable approval was committed and a provision was accepted for the origin queue; it does not assert socket receipt. A pre-queue technical failure is `Unavailable`, not fake success.
11. **`crates/ene-client` — connection progress API.** [verified] Replace `connect_with_bootstrap` with `Client::begin_connect(...) -> ConnectProgress`, where `Connected(Client)` is the stored-device path and `Pending(PendingPairingClient)` owns the original stream, incarnation, data-dir context, and non-secret pending ID. `PendingPairingClient::complete(self)` waits for `PairingProvision`, continues capability/challenge/proof on that same stream, and persists only after `AuthResult::Accepted`. `Client::connect` may delegate and await completion for callers that do not need progress. Remove `ENE_PAIRING_SECRET`, `client-pending.json`, pending-file helpers, bootstrap resolution, rotation guidance, and all poll frame fields.
12. **`apps/ene-ctl/src/main.rs`.** [verified] Use the progress API so a first run can display the non-secret pending ID/instructions and remain alive awaiting approval on its retained socket. Never print or accept a pairing secret.
13. **Desktop session/runtime.** [verified] Change `DesktopConnect::PendingOwnerConfirmation` from a bare string to an owner of `PendingPairingClient`. Store it in `DesktopRuntime` while the confirmation request runs; after the non-secret `DeviceApproved`, consume it via `complete` and adopt that client. Rejection or any error drops it. Remove the secret clone, `connect_with_bootstrap`, and reconnect. If approval committed but provision write/read fails, surface an explicit client transport/protocol error and instruct a fresh pairing without implying rollback or redelivery.

### 6.5 Documentation and test fixtures

14. **Rustdoc, fixtures, and implementation status.** [verified] Rewrite polling/bootstrap comments and helpers throughout the cited files, migrate all direct callers, and remove obsolete rotation/wrong-bootstrap scenarios. After tests pass, update only the pairing-gap lines in `docs/implementation/stages/stage-7.md` and `docs/implementation/PROGRESS.md`; preserve the user's current unstaged VRM edits exactly.

## 7. Implementation Sequence

1. **Introduce inert protocol/custody primitives.** Add `PairingProvision`, its redacted/zeroizing secret type, payload mapping, zeroizing framing helpers, and focused DTO tests without yet removing old variants. The tree remains compiling and no product path changes.
2. **Add the inert live-delivery mechanism.** Add `PairingDeliveryRegistry`, Host ownership, registration/removal hooks, and isolated registry tests. Do not route approval through it yet; the tree remains behaviorally unchanged.
3. **Perform one coherent vertical cutover.** In one implementation unit, simplify the repository/schema, bind fresh pending requests, route approval through the claimed origin queue, add the `serve_connection` delivery branch, remove the control secret, add client progress/pending ownership, and migrate CLI/desktop production callers. Remove polling/bootstrap/rotation symbols in that same unit so no intermediate production path can emit fake success or depend on two delivery mechanisms.
4. **Migrate unit tests and race fixtures.** Update API/store/client/core/desktop tests, including every depth-1 caller from §4. Use barriers/notifications around claim, commit, queue, phase transition, and write; do not use sleep to assert race order.
5. **Migrate Unix/Windows E2E helpers.** Keep the first client process/socket alive, obtain its pending ID from Host/control facts, approve concurrently, and await successful authentication. Delete all `ENE_PAIRING_SECRET`, pending-file, reapproval, and out-of-band provisioning setup.
6. **Run focused then workspace verification.** Fix formatting/lints/docs and run the §8 commands. The final ripgrep command must return no Rust-code matches for removed paths.
7. **Update implementation status last.** Remove only the completed IPC §9.2 gap text from the two dirty Stage 7 documents, preserving unrelated VRM content byte-for-byte, then rerun format-independent status/diff review.

## 8. Test Strategy

### Protocol and secret hygiene

- [verified] Extend `crates/ene-api/src/v1/handshake.rs` tests and `crates/ene-api/tests/dto.rs`: round-trip `PairingProvision`, assert canonical message type, assert `Debug` is redacted, and assert `PairingRequest` has no polling field.
- [inferred] Exercise explicit zeroization methods on owned wrappers/buffers without relying on allocator-after-free inspection. Assert every error message and outcome contains IDs/reasons only, never the known secret marker.

### Store and authority

- [verified] Replace polling/rotation coverage in `crates/ene-store/src/tests.rs` with fresh unique pending creation, wrong-origin refusal, one-shot approval, consumed-ID refusal, and origin-scoped abandonment. Update erasure fixtures/column accounting for the direct schema change.
- [inferred] Add deterministic approval-versus-abandon tests for both orderings. Exactly one transaction may consume the pending; neither ordering can produce two devices or two secrets.

### Connection and concurrency

- [verified] Extend `apps/ene-core/src/conn/tests.rs` around its existing `serve_connection` fixtures: register-before-read, pending response before provision, same socket/correlation, `Accepted → Paired` immediately before write, queue capacity one, duplicate approval, phase mismatch, shutdown, EOF, and write failure.
- [inferred] Add barrier-controlled disconnect races at (a) before claim, (b) after claim/before store commit, (c) after commit/before queue, and (d) after dequeue/before write. Expected outcomes are either no approval or one durable approval with unavailable delivery; never replay, another connection's provision, secret-bearing control output, or a second external effect.

### Client, desktop, and platform integration

- [verified] Update `crates/ene-client/src/tests.rs` for the progress API and persist-after-accepted ordering. A pending client must reject any pre-provision frame other than its correlated `PairingProvision`; denial/EOF/auth rejection leaves no new device file.
- [verified] Update `apps/ene-desktop/tests/stage7_b.rs`: the runtime retains the pending transport across confirmation, `ControlOutcome::DeviceApproved` has no secret, and the resulting authenticated client comes from that retained transport.
- [verified] Rewrite the five core E2E helpers so Unix sockets and Windows named pipes provision the still-running origin process. Replace wrong-bootstrap/reapproval cases with lost-origin/no-redelivery and fresh-pairing cases.

### Commands

```text
cargo fmt --all -- --check
cargo test -p ene-api
cargo test -p ene-store
cargo test -p ene-client
cargo test -p ene-core
cargo test -p ene-desktop
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps
rg -n 'ENE_PAIRING_SECRET|client-pending\.json|connect_with_bootstrap|PairingResult::Paired' crates apps --glob '*.rs'
```

The final `rg` is expected to return no matches. Windows-specific tests still require Windows CI/host execution; do not report the named-pipe slice complete from Linux-only results.

## 9. Risk and Impact Analysis

- **Cross-boundary secret exposure — high.** [verified] The current secret crosses repository → Host string → confirmation DTO → desktop clone. The cutover must cover API `Debug`, queued values, `WirePayload` clones, session state, serialized buffers, errors, and test diagnostics. Redaction alone is insufficient without zeroization.
- **Approval/delivery partial failure — high.** [inferred] SQLite approval, auth-store save, queue acceptance, and socket write cannot be atomic. The contract is: no success before store/auth/queue acceptance; after that, `DeviceApproved` is only an approval fact, while authentication proves use. Any later failure is explicit, never auto-retried or replayed. The existing repository says revocation is deferred (`crates/ene-credential/src/pairing.rs:70-73`), so an approved-but-undelivered record may remain inert until separate revocation work exists.
- **Disconnect/approval race — high.** [verified] `serve_connection` closes at `apps/ene-core/src/conn.rs:1588`, while approval runs on the control path. A registry claim plus store origin CAS serializes ownership; bounded channels and the sole writer serialize bytes. Tests must force both orders.
- **Public API blast radius — high/critical.** [graph] All direct dependents from §4 are assigned: desktop session and `Client::connect`; `execute_pending`, one serve-test group, and five E2E groups; four store-test callers; five `serve_connection` fixtures. Additional source-derived constructors in API DTO, shutdown, targeted-deletion, erasure-owner, and host-lock tests are listed in §10.
- **Schema/compatibility — intentional break.** [verified] Removing `paired_device.pending_id` makes old polling state unreadable by design. Repository guidance forbids migration layers and compatibility shims; tests use fresh databases.
- **Performance/backpressure — low if bounded.** [inferred] One capacity-one slot per live connection is O(live connections), performs no scan, and reuses the existing select loop. Never replace it with an unbounded queue or store-wide polling pass.
- **Observability.** [inferred] Errors may name pending/device/connection IDs and phase, but never the secret or encoded frame. Distinguish unknown pending, origin unavailable, approval committed but provision unavailable, phase mismatch, and authentication rejection; do not collapse domain outcomes into success.
- **Concurrent user work.** [verified] The provenance manifest marks both implementation-status documents unstaged, and the global digest covers additional unrelated desktop/asset work. Implementation must preserve those changes and avoid unrelated Stage 7/VRM scope.

## 10. Files Expected to Change

| File | Symbols | Reason |
| ---- | ------- | ------ |
| `crates/ene-api/Cargo.toml`, `crates/ene-api/src/v1/handshake.rs`, `crates/ene-api/src/v1/payload.rs`, `crates/ene-api/tests/dto.rs` | pairing/auth DTOs and tests | Add auth-only provision with redaction/zeroization; remove polling/Paired response. |
| `crates/ene-client/Cargo.toml`, `crates/ene-client/src/{lib.rs,transport.rs,device.rs,frames.rs,session.rs,tests.rs}` | connection progress, pending owner, device custody | Keep the original socket, consume provision, remove environment/pending-file/bootstrap paths. |
| `crates/ene-credential/src/{lib.rs,pairing.rs}` | repository contract and secret material | Fresh requests, one-shot origin approval, disconnect abandonment, no rotation. |
| `crates/ene-store/src/{migrate.rs,credential.rs,erasure/remainder.rs,tests.rs,tests/erasure_owners.rs}` | schema, SQL, repository tests/fixtures | Remove paired pending correlation/polling; add origin cleanup. |
| `crates/ene-local-control/src/lib.rs` | `ControlOutcome::DeviceApproved` | Remove the confirmation-channel secret and receipt implication. |
| `apps/ene-core/Cargo.toml`, `apps/ene-core/src/{lib.rs,pairing_delivery.rs,serve.rs,serve/handshake.rs,conn.rs,host_control.rs}` | registry, approval, pairing, socket loop | Route one bounded provision to the originating writer and clean it on lifecycle end. |
| `apps/ene-core/src/{conn/tests.rs,conn/shutdown_tests.rs,serve/tests.rs,host_lock.rs,targeted_deletion.rs}` | unit/race fixtures | Cover ordering/failure semantics and migrate constructors. |
| `apps/ene-core/tests/{stage5_e2e.rs,stage5_windows_pipe_e2e.rs,stage6_e2e.rs,stage7_a1.rs,vertical_slice.rs}` | platform/E2E pairing helpers | Replace injected secret/polling with a retained live origin. |
| `apps/ene-desktop/src/session.rs`, `apps/ene-desktop/src/ui/runtime.rs`, `apps/ene-desktop/tests/stage7_b.rs` | pending transport and confirmation flow | Retain and complete the original Client connection; no control secret/reconnect. |
| `apps/ene-ctl/src/main.rs` | Client startup | Show pending ID while the original process waits; never accept/print a secret. |
| `docs/implementation/stages/stage-7.md`, `docs/implementation/PROGRESS.md` | A1/current milestone status | Remove the completed gap only; preserve current unrelated VRM edits. |

## 11. Reusable Implementation Context

```json
{
  "implementation_context": {
    "task_summary": "Deliver first-pairing device authentication material only in an auth-specific frame on the live connection that originated the request; remove polling, bootstrap-secret, reapproval-rotation, and secret-bearing control paths.",
    "acceptance_criteria": [
      "PairingProvision is written only by serve_connection for the registered originating connection.",
      "A lost origin cannot have its provision replayed or redirected to another connection or requester.",
      "Requester/control outcomes, stdout, ordinary DTOs, logs, and Debug contain no pairing secret.",
      "Client and desktop retain the original stream through confirmation and persist the secret only after accepted authentication.",
      "Delivery, disconnect, duplicate approval, and write-failure races are bounded and deterministic."
    ],
    "evidence_provenance": {
      "schema_version": 2,
      "head_commit": "8bf8445d7e3d2a40c2871b512a48a4bf01a0e356",
      "generated_plan_path": "docs/plans/2026-09-20-gitnexus-plan-pairing-provision-origin-delivery.md",
      "global_dirty_digest": {
        "algorithm": "sha256",
        "canonicalization": "gitnexus-evidence-provenance-v2 NUL-framed UTF-8 records",
        "value": "119a8063e9b06a6e0564b41628fafd3c0d516e7fcf80ac917efa597719f99e81"
      },
      "cited_path_manifest": [
        {
          "path": "Cargo.toml",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:fd91b6c50df86498d5cb70fd7801fb3084c0e53f408ca545a6e2dc6dad144c36",
          "index_digest": "sha256:fd91b6c50df86498d5cb70fd7801fb3084c0e53f408ca545a6e2dc6dad144c36",
          "worktree_digest": "sha256:fd91b6c50df86498d5cb70fd7801fb3084c0e53f408ca545a6e2dc6dad144c36",
          "untracked_digest": "absent"
        },
        {
          "path": "apps/ene-core/Cargo.toml",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:4874390a752eab84e236da0ff1bb351a0e7184fca9ca96b7d8f486dd5f9b9c16",
          "index_digest": "sha256:4874390a752eab84e236da0ff1bb351a0e7184fca9ca96b7d8f486dd5f9b9c16",
          "worktree_digest": "sha256:4874390a752eab84e236da0ff1bb351a0e7184fca9ca96b7d8f486dd5f9b9c16",
          "untracked_digest": "absent"
        },
        {
          "path": "apps/ene-core/src/conn.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:ed97f7eaf2162280248ed50b75bc43077ca50597e86f43bd505d0891b5304bae",
          "index_digest": "sha256:ed97f7eaf2162280248ed50b75bc43077ca50597e86f43bd505d0891b5304bae",
          "worktree_digest": "sha256:ed97f7eaf2162280248ed50b75bc43077ca50597e86f43bd505d0891b5304bae",
          "untracked_digest": "absent"
        },
        {
          "path": "apps/ene-core/src/conn/shutdown_tests.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:64077c054f59f97257fb4d44a9ee5fe3a11a7c1e1e2f4bca7e79a336caf11b0f",
          "index_digest": "sha256:64077c054f59f97257fb4d44a9ee5fe3a11a7c1e1e2f4bca7e79a336caf11b0f",
          "worktree_digest": "sha256:64077c054f59f97257fb4d44a9ee5fe3a11a7c1e1e2f4bca7e79a336caf11b0f",
          "untracked_digest": "absent"
        },
        {
          "path": "apps/ene-core/src/conn/tests.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:5dfc33c37cda0ccf1a35656483df9caede6f8da8be597490e462b74a26e27d51",
          "index_digest": "sha256:5dfc33c37cda0ccf1a35656483df9caede6f8da8be597490e462b74a26e27d51",
          "worktree_digest": "sha256:5dfc33c37cda0ccf1a35656483df9caede6f8da8be597490e462b74a26e27d51",
          "untracked_digest": "absent"
        },
        {
          "path": "apps/ene-core/src/host_control.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:d2e69837b6dd035f41db945a05889192dfcfc9b6bf5fad9da621db3c58b46e61",
          "index_digest": "sha256:d2e69837b6dd035f41db945a05889192dfcfc9b6bf5fad9da621db3c58b46e61",
          "worktree_digest": "sha256:d2e69837b6dd035f41db945a05889192dfcfc9b6bf5fad9da621db3c58b46e61",
          "untracked_digest": "absent"
        },
        {
          "path": "apps/ene-core/src/host_lock.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:799ca28bce6e5b5d183dc1cafe0a8fcf8cd19642e42f45a8e096bab80a2469ce",
          "index_digest": "sha256:799ca28bce6e5b5d183dc1cafe0a8fcf8cd19642e42f45a8e096bab80a2469ce",
          "worktree_digest": "sha256:799ca28bce6e5b5d183dc1cafe0a8fcf8cd19642e42f45a8e096bab80a2469ce",
          "untracked_digest": "absent"
        },
        {
          "path": "apps/ene-core/src/lib.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:094d6419b2ddae98dc91db5826073af7102787bc0d196829d592f077cf6c0f34",
          "index_digest": "sha256:094d6419b2ddae98dc91db5826073af7102787bc0d196829d592f077cf6c0f34",
          "worktree_digest": "sha256:094d6419b2ddae98dc91db5826073af7102787bc0d196829d592f077cf6c0f34",
          "untracked_digest": "absent"
        },
        {
          "path": "apps/ene-core/src/pairing_delivery.rs",
          "object_kind": {
            "head": "absent",
            "index": "absent",
            "worktree": "absent",
            "untracked": "absent"
          },
          "state": "absent",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "absent",
          "index_digest": "absent",
          "worktree_digest": "absent",
          "untracked_digest": "absent"
        },
        {
          "path": "apps/ene-core/src/serve.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:dd65121d3fbb5453eb8c6918650511c58c90538cf3c9fe248a154128c97643fc",
          "index_digest": "sha256:dd65121d3fbb5453eb8c6918650511c58c90538cf3c9fe248a154128c97643fc",
          "worktree_digest": "sha256:dd65121d3fbb5453eb8c6918650511c58c90538cf3c9fe248a154128c97643fc",
          "untracked_digest": "absent"
        },
        {
          "path": "apps/ene-core/src/serve/handshake.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:ca61d2342c489582d8f764806bd136a2833154d031dcfc747d51a4ca80c6b04d",
          "index_digest": "sha256:ca61d2342c489582d8f764806bd136a2833154d031dcfc747d51a4ca80c6b04d",
          "worktree_digest": "sha256:ca61d2342c489582d8f764806bd136a2833154d031dcfc747d51a4ca80c6b04d",
          "untracked_digest": "absent"
        },
        {
          "path": "apps/ene-core/src/serve/tests.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:fb66165be6aebb6a1232a5b049eb43995c7d2393dc205a586ea9ac4464ee07b9",
          "index_digest": "sha256:fb66165be6aebb6a1232a5b049eb43995c7d2393dc205a586ea9ac4464ee07b9",
          "worktree_digest": "sha256:fb66165be6aebb6a1232a5b049eb43995c7d2393dc205a586ea9ac4464ee07b9",
          "untracked_digest": "absent"
        },
        {
          "path": "apps/ene-core/src/targeted_deletion.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:a85e8dda6b735fee766c0d1c1b713a05bf8d48d328c05c69d074316583289d90",
          "index_digest": "sha256:a85e8dda6b735fee766c0d1c1b713a05bf8d48d328c05c69d074316583289d90",
          "worktree_digest": "sha256:a85e8dda6b735fee766c0d1c1b713a05bf8d48d328c05c69d074316583289d90",
          "untracked_digest": "absent"
        },
        {
          "path": "apps/ene-core/tests/stage5_e2e.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:1031cb71dc9e927e225af3581f3e5ec76aa392e1e5856288702095e09ec64e46",
          "index_digest": "sha256:1031cb71dc9e927e225af3581f3e5ec76aa392e1e5856288702095e09ec64e46",
          "worktree_digest": "sha256:1031cb71dc9e927e225af3581f3e5ec76aa392e1e5856288702095e09ec64e46",
          "untracked_digest": "absent"
        },
        {
          "path": "apps/ene-core/tests/stage5_windows_pipe_e2e.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:fc8babcb6f015cf80add8650dc526f5529c9547544b20e773ae28cfd774b8d4d",
          "index_digest": "sha256:fc8babcb6f015cf80add8650dc526f5529c9547544b20e773ae28cfd774b8d4d",
          "worktree_digest": "sha256:fc8babcb6f015cf80add8650dc526f5529c9547544b20e773ae28cfd774b8d4d",
          "untracked_digest": "absent"
        },
        {
          "path": "apps/ene-core/tests/stage6_e2e.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:5be7e145c44f176c291575ec181ce327a05a036b5ce7b491b9f45245078269c9",
          "index_digest": "sha256:5be7e145c44f176c291575ec181ce327a05a036b5ce7b491b9f45245078269c9",
          "worktree_digest": "sha256:5be7e145c44f176c291575ec181ce327a05a036b5ce7b491b9f45245078269c9",
          "untracked_digest": "absent"
        },
        {
          "path": "apps/ene-core/tests/stage7_a1.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:fed05547b91ea6924adb612b2db56cc7edf16bd25dbc771c2396aa423166956e",
          "index_digest": "sha256:fed05547b91ea6924adb612b2db56cc7edf16bd25dbc771c2396aa423166956e",
          "worktree_digest": "sha256:fed05547b91ea6924adb612b2db56cc7edf16bd25dbc771c2396aa423166956e",
          "untracked_digest": "absent"
        },
        {
          "path": "apps/ene-core/tests/vertical_slice.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:6d1b3a0d4c8ed61289031d72e275ceffd6b1c86be5dad15556fa745fbdbc29e8",
          "index_digest": "sha256:6d1b3a0d4c8ed61289031d72e275ceffd6b1c86be5dad15556fa745fbdbc29e8",
          "worktree_digest": "sha256:6d1b3a0d4c8ed61289031d72e275ceffd6b1c86be5dad15556fa745fbdbc29e8",
          "untracked_digest": "absent"
        },
        {
          "path": "apps/ene-ctl/src/main.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:ca454684ae02f97e7e7179963962b3c89da1430c1e46d31b24436c0d03d2bc02",
          "index_digest": "sha256:ca454684ae02f97e7e7179963962b3c89da1430c1e46d31b24436c0d03d2bc02",
          "worktree_digest": "sha256:ca454684ae02f97e7e7179963962b3c89da1430c1e46d31b24436c0d03d2bc02",
          "untracked_digest": "absent"
        },
        {
          "path": "apps/ene-desktop/src/session.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:ecdca5f09ff9e10eb037800bccd852bb38d57d37e20c5a15cda4b9abb3004986",
          "index_digest": "sha256:ecdca5f09ff9e10eb037800bccd852bb38d57d37e20c5a15cda4b9abb3004986",
          "worktree_digest": "sha256:ecdca5f09ff9e10eb037800bccd852bb38d57d37e20c5a15cda4b9abb3004986",
          "untracked_digest": "absent"
        },
        {
          "path": "apps/ene-desktop/src/ui/runtime.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:c60bef4d83faffd4899f379dc7db3a6f2282c63e72c6872939f919f0ab7adaf6",
          "index_digest": "sha256:c60bef4d83faffd4899f379dc7db3a6f2282c63e72c6872939f919f0ab7adaf6",
          "worktree_digest": "sha256:c60bef4d83faffd4899f379dc7db3a6f2282c63e72c6872939f919f0ab7adaf6",
          "untracked_digest": "absent"
        },
        {
          "path": "apps/ene-desktop/tests/stage7_b.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:e3bbeab4c89f91be1118c8d3af4ec9763947a3fcce5eb49fb67b5bbbb7fe18d4",
          "index_digest": "sha256:e3bbeab4c89f91be1118c8d3af4ec9763947a3fcce5eb49fb67b5bbbb7fe18d4",
          "worktree_digest": "sha256:e3bbeab4c89f91be1118c8d3af4ec9763947a3fcce5eb49fb67b5bbbb7fe18d4",
          "untracked_digest": "absent"
        },
        {
          "path": "crates/ene-api/Cargo.toml",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:05e89a8203da1c5e6c65356d78b76177a8ddee3679667eaba82204a9a2198a33",
          "index_digest": "sha256:05e89a8203da1c5e6c65356d78b76177a8ddee3679667eaba82204a9a2198a33",
          "worktree_digest": "sha256:05e89a8203da1c5e6c65356d78b76177a8ddee3679667eaba82204a9a2198a33",
          "untracked_digest": "absent"
        },
        {
          "path": "crates/ene-api/src/v1/handshake.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:1aebeeb88c99705b1d364a03c6edd814eecc2a85fbfe8f3808cf353bee55e223",
          "index_digest": "sha256:1aebeeb88c99705b1d364a03c6edd814eecc2a85fbfe8f3808cf353bee55e223",
          "worktree_digest": "sha256:1aebeeb88c99705b1d364a03c6edd814eecc2a85fbfe8f3808cf353bee55e223",
          "untracked_digest": "absent"
        },
        {
          "path": "crates/ene-api/src/v1/payload.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:4234cc52f5cc53842717495c431fda7b772c50eb6d794b8c5db3b8f4e2df34c0",
          "index_digest": "sha256:4234cc52f5cc53842717495c431fda7b772c50eb6d794b8c5db3b8f4e2df34c0",
          "worktree_digest": "sha256:4234cc52f5cc53842717495c431fda7b772c50eb6d794b8c5db3b8f4e2df34c0",
          "untracked_digest": "absent"
        },
        {
          "path": "crates/ene-api/tests/dto.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:05eb60dad59adfea23526a694f75a4406ff511e100db4695aa9d34f8327e557f",
          "index_digest": "sha256:05eb60dad59adfea23526a694f75a4406ff511e100db4695aa9d34f8327e557f",
          "worktree_digest": "sha256:05eb60dad59adfea23526a694f75a4406ff511e100db4695aa9d34f8327e557f",
          "untracked_digest": "absent"
        },
        {
          "path": "crates/ene-client/Cargo.toml",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:a1181b352b85a5e657f6cec2dc5ac0143aab2d682c1f210fcde1389660aba5a2",
          "index_digest": "sha256:a1181b352b85a5e657f6cec2dc5ac0143aab2d682c1f210fcde1389660aba5a2",
          "worktree_digest": "sha256:a1181b352b85a5e657f6cec2dc5ac0143aab2d682c1f210fcde1389660aba5a2",
          "untracked_digest": "absent"
        },
        {
          "path": "crates/ene-client/src/device.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:24ceb17021cbcaa904a98970400ba4b18773bec821339df0c0f6c9283798474a",
          "index_digest": "sha256:24ceb17021cbcaa904a98970400ba4b18773bec821339df0c0f6c9283798474a",
          "worktree_digest": "sha256:24ceb17021cbcaa904a98970400ba4b18773bec821339df0c0f6c9283798474a",
          "untracked_digest": "absent"
        },
        {
          "path": "crates/ene-client/src/frames.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:de4fe7118865a072cfcc0fe5a92b228657c8323a84e35c0a552ad8c4a04dcfb2",
          "index_digest": "sha256:de4fe7118865a072cfcc0fe5a92b228657c8323a84e35c0a552ad8c4a04dcfb2",
          "worktree_digest": "sha256:de4fe7118865a072cfcc0fe5a92b228657c8323a84e35c0a552ad8c4a04dcfb2",
          "untracked_digest": "absent"
        },
        {
          "path": "crates/ene-client/src/lib.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:0445f804b24fee15a3d9fbdf305b59a1c5b923ffd1e89fb1c615d6fda75ac730",
          "index_digest": "sha256:0445f804b24fee15a3d9fbdf305b59a1c5b923ffd1e89fb1c615d6fda75ac730",
          "worktree_digest": "sha256:0445f804b24fee15a3d9fbdf305b59a1c5b923ffd1e89fb1c615d6fda75ac730",
          "untracked_digest": "absent"
        },
        {
          "path": "crates/ene-client/src/session.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:f98485bb22c6f6673a72373929879dd07e7570c776f15068fce7f2a73fd81de1",
          "index_digest": "sha256:f98485bb22c6f6673a72373929879dd07e7570c776f15068fce7f2a73fd81de1",
          "worktree_digest": "sha256:f98485bb22c6f6673a72373929879dd07e7570c776f15068fce7f2a73fd81de1",
          "untracked_digest": "absent"
        },
        {
          "path": "crates/ene-client/src/tests.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:6e98933d19f9b62fb5731345e69062e72b40f7cd8435520c86b258c33338acb6",
          "index_digest": "sha256:6e98933d19f9b62fb5731345e69062e72b40f7cd8435520c86b258c33338acb6",
          "worktree_digest": "sha256:6e98933d19f9b62fb5731345e69062e72b40f7cd8435520c86b258c33338acb6",
          "untracked_digest": "absent"
        },
        {
          "path": "crates/ene-client/src/transport.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:df8314781bd52a9a944a07f34038197892ac72fd4b87ba74a154b94fa361379a",
          "index_digest": "sha256:df8314781bd52a9a944a07f34038197892ac72fd4b87ba74a154b94fa361379a",
          "worktree_digest": "sha256:df8314781bd52a9a944a07f34038197892ac72fd4b87ba74a154b94fa361379a",
          "untracked_digest": "absent"
        },
        {
          "path": "crates/ene-credential/src/lib.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:da984f0a73734aa6c83e389b4984bf707d2b44a96fde72f756736fa964a5ab68",
          "index_digest": "sha256:da984f0a73734aa6c83e389b4984bf707d2b44a96fde72f756736fa964a5ab68",
          "worktree_digest": "sha256:da984f0a73734aa6c83e389b4984bf707d2b44a96fde72f756736fa964a5ab68",
          "untracked_digest": "absent"
        },
        {
          "path": "crates/ene-credential/src/pairing.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:1ab7d87ec004309f469328d14eee037b619886b306f63ac67f589573847adc19",
          "index_digest": "sha256:1ab7d87ec004309f469328d14eee037b619886b306f63ac67f589573847adc19",
          "worktree_digest": "sha256:1ab7d87ec004309f469328d14eee037b619886b306f63ac67f589573847adc19",
          "untracked_digest": "absent"
        },
        {
          "path": "crates/ene-local-control/src/lib.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:785a98ed737a976c96881dcf28a213b76cfde886316640b57cd71d721eae1d5f",
          "index_digest": "sha256:785a98ed737a976c96881dcf28a213b76cfde886316640b57cd71d721eae1d5f",
          "worktree_digest": "sha256:785a98ed737a976c96881dcf28a213b76cfde886316640b57cd71d721eae1d5f",
          "untracked_digest": "absent"
        },
        {
          "path": "crates/ene-store/src/credential.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:d713062e01217edfaef6aa6cadf9fc3dc11de3074cb18581829b56a68792e8dd",
          "index_digest": "sha256:d713062e01217edfaef6aa6cadf9fc3dc11de3074cb18581829b56a68792e8dd",
          "worktree_digest": "sha256:d713062e01217edfaef6aa6cadf9fc3dc11de3074cb18581829b56a68792e8dd",
          "untracked_digest": "absent"
        },
        {
          "path": "crates/ene-store/src/erasure/remainder.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:b3b8c9fbfa684e18f6fcf1bc47c849816da5ae6ff54ba00506945470a98a8b48",
          "index_digest": "sha256:b3b8c9fbfa684e18f6fcf1bc47c849816da5ae6ff54ba00506945470a98a8b48",
          "worktree_digest": "sha256:b3b8c9fbfa684e18f6fcf1bc47c849816da5ae6ff54ba00506945470a98a8b48",
          "untracked_digest": "absent"
        },
        {
          "path": "crates/ene-store/src/migrate.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:fb4b4d46fddcab590da2138767c09e7dc233d4ca7946d6373fb24069108ce481",
          "index_digest": "sha256:fb4b4d46fddcab590da2138767c09e7dc233d4ca7946d6373fb24069108ce481",
          "worktree_digest": "sha256:fb4b4d46fddcab590da2138767c09e7dc233d4ca7946d6373fb24069108ce481",
          "untracked_digest": "absent"
        },
        {
          "path": "crates/ene-store/src/tests.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:5342641a38c1578e536fca4ac6e7a71ba6a85e39036462f11f22af0608165c5d",
          "index_digest": "sha256:5342641a38c1578e536fca4ac6e7a71ba6a85e39036462f11f22af0608165c5d",
          "worktree_digest": "sha256:5342641a38c1578e536fca4ac6e7a71ba6a85e39036462f11f22af0608165c5d",
          "untracked_digest": "absent"
        },
        {
          "path": "crates/ene-store/src/tests/erasure_owners.rs",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:7eac8f1ae83c0c2b8b095ff42bb0c8001e999ed9db51fd14415433693d312f40",
          "index_digest": "sha256:7eac8f1ae83c0c2b8b095ff42bb0c8001e999ed9db51fd14415433693d312f40",
          "worktree_digest": "sha256:7eac8f1ae83c0c2b8b095ff42bb0c8001e999ed9db51fd14415433693d312f40",
          "untracked_digest": "absent"
        },
        {
          "path": "docs/design/concrete/first-party-desktop.md",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:0e6923fe291a7c1fbc37e59460afb0e99caff0d092e4b6723374567026454021",
          "index_digest": "sha256:0e6923fe291a7c1fbc37e59460afb0e99caff0d092e4b6723374567026454021",
          "worktree_digest": "sha256:0e6923fe291a7c1fbc37e59460afb0e99caff0d092e4b6723374567026454021",
          "untracked_digest": "absent"
        },
        {
          "path": "docs/design/concrete/host-client-ipc.md",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:63ad2b0e4075a3c69e9b7173b0f646521c6d15912e19c95af9fc83bc524985f7",
          "index_digest": "sha256:63ad2b0e4075a3c69e9b7173b0f646521c6d15912e19c95af9fc83bc524985f7",
          "worktree_digest": "sha256:63ad2b0e4075a3c69e9b7173b0f646521c6d15912e19c95af9fc83bc524985f7",
          "untracked_digest": "absent"
        },
        {
          "path": "docs/implementation/PROGRESS.md",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "unstaged",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:7800adff7021b3eccf60b50ba3e276bd794efc600a4855f10cfd4c19e2fb2080",
          "index_digest": "sha256:7800adff7021b3eccf60b50ba3e276bd794efc600a4855f10cfd4c19e2fb2080",
          "worktree_digest": "sha256:a8f331ce3f39d121854e15425a5aa5398f39a3540c3e9ef82ab9789aaab2d58d",
          "untracked_digest": "absent"
        },
        {
          "path": "docs/implementation/stages/stage-7.md",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "unstaged",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:8eac7fdcf4192859910dc1d9f0dc6af6df81b16392d966f3caddf2150b5c3c09",
          "index_digest": "sha256:8eac7fdcf4192859910dc1d9f0dc6af6df81b16392d966f3caddf2150b5c3c09",
          "worktree_digest": "sha256:d65e7a51fc3d03000caf8683029e16882c43a80bf8b80c53ccfd72b58a9cdf73",
          "untracked_digest": "absent"
        },
        {
          "path": "docs/requirements/requirements.md",
          "object_kind": {
            "head": "regular",
            "index": "regular",
            "worktree": "regular",
            "untracked": "absent"
          },
          "state": "clean",
          "rename_from": null,
          "rename_to": null,
          "head_digest": "sha256:eecc3d917728663536c1f2833bef1e1dd9973d8d00598223b1131f5db1477d8a",
          "index_digest": "sha256:eecc3d917728663536c1f2833bef1e1dd9973d8d00598223b1131f5db1477d8a",
          "worktree_digest": "sha256:eecc3d917728663536c1f2833bef1e1dd9973d8d00598223b1131f5db1477d8a",
          "untracked_digest": "absent"
        }
      ]
    },
    "primary_symbols": [
      {
        "symbol": "Client::connect_with_bootstrap (to replace)",
        "file": "crates/ene-client/src/transport.rs",
        "lines": "110-299",
        "role": "current polling/bootstrap client handshake and persistence path"
      },
      {
        "symbol": "HostHandle::pair",
        "file": "apps/ene-core/src/serve/handshake.rs",
        "lines": "49-115",
        "role": "current pending creation, approved-ID polling, and premature phase transition"
      },
      {
        "symbol": "HostHandle::approve_device",
        "file": "apps/ene-core/src/serve.rs",
        "lines": "2050-2104",
        "role": "current durable approval, rotation fallback, auth-store save, and secret return"
      },
      {
        "symbol": "DevicePairingRepository::approve_pending",
        "file": "crates/ene-credential/src/pairing.rs",
        "lines": "115-143",
        "role": "origin CAS and current reapproval/rotation contract"
      },
      {
        "symbol": "serve_connection",
        "file": "apps/ene-core/src/conn.rs",
        "lines": "1315-1600",
        "role": "sole socket writer and connection-lifetime cleanup owner"
      }
    ],
    "related_symbols": [
      {
        "symbol": "WirePayload",
        "relationship": "CONTAINS auth and pairing variants",
        "relevance": "adds PairingProvision and canonical message type"
      },
      {
        "symbol": "ConnectionTable::note_paired",
        "relationship": "STATE MUTATION called by delivery",
        "relevance": "enforces Accepted-to-Paired immediately before provision write"
      },
      {
        "symbol": "host_control::execute_pending",
        "relationship": "CALLS approve_device",
        "relevance": "must emit non-secret approval facts only"
      },
      {
        "symbol": "desktop session::connect_or_pending",
        "relationship": "CALLS client transport",
        "relevance": "must retain an owning pending transport"
      },
      {
        "symbol": "DesktopRuntime::confirm_owner",
        "relationship": "CONSUMES ControlOutcome",
        "relevance": "must complete the retained socket instead of reconnecting with a secret"
      },
      {
        "symbol": "DevicePairingRepository::request_pairing",
        "relationship": "CALLED BY HostHandle::pair",
        "relevance": "becomes fresh-request-only"
      },
      {
        "symbol": "HostHandle::drop_connection_transient_state",
        "relationship": "LIFECYCLE OWNER",
        "relevance": "removes the delivery slot and drops queued material"
      }
    ],
    "execution_path": [
      "serve_connection registers a capacity-one delivery receiver for its ConnectionWireId before starting socket reads.",
      "The client sends one PairingRequest with a display descriptor and no pending poll ID.",
      "HostHandle::pair creates a fresh durable pending, binds its pending ID to the registered connection slot, and returns PendingOwnerConfirmation.",
      "The caller keeps PendingPairingClient and the same stream alive while the pending ID is approved through the Owner confirmation surface.",
      "HostHandle::approve_device atomically claims the live pending slot, approves the store row with its recorded origin, saves Host authentication material, and enqueues one PairingProvision; it returns non-secret facts only.",
      "serve_connection dequeues the provision, rechecks/advances the same connection Accepted-to-Paired, builds an auth-only response from the original pairing template, and writes it through its sole write half.",
      "PendingPairingClient receives the provision, continues capability negotiation and proof authentication on that stream, and persists the device secret only after AuthResult::Accepted.",
      "EOF, close, supersession, phase failure, queue failure, or write failure drops/zeroizes material and never redirects or replays it; a later attempt opens a fresh pending."
    ],
    "pdg_constraints": [
      {
        "description": "serve_connection's loop/select owns every unsolicited write and final close",
        "affected_statements": [
          "apps/ene-core/src/conn.rs:1348-1576",
          "apps/ene-core/src/conn.rs:1588"
        ],
        "implementation_consequence": "delivery must be another bounded select branch; approval must never write the socket or mutate connection phase"
      },
      {
        "description": "approve_device's absent-origin branch currently enables reapproval and secret rotation",
        "affected_statements": [
          "apps/ene-core/src/serve.rs:2081-2093"
        ],
        "implementation_consequence": "delete the fallback; an ID without a live claimed origin is unknown/unavailable"
      },
      {
        "description": "the client Pending branch currently persists a poll ID and returns, while resolved secret flows into SessionState and post-auth persistence",
        "affected_statements": [
          "crates/ene-client/src/transport.rs:189-231",
          "crates/ene-client/src/transport.rs:253-290"
        ],
        "implementation_consequence": "replace the return/drop boundary with an owning pending state, then preserve the existing persist-after-Accepted ordering"
      }
    ],
    "architectural_patterns": [
      {
        "pattern": "single socket writer",
        "example_location": "apps/ene-core/src/conn.rs:1315-1600 serve_connection",
        "usage_guidance": "queue data to this loop; never share write_half with approval code"
      },
      {
        "pattern": "bounded Tokio channels",
        "example_location": "apps/ene-core/src/conn.rs:1329 and 1384-1385",
        "usage_guidance": "use capacity one for the one-shot provision slot and fail closed on a full/closed sender"
      },
      {
        "pattern": "single connection-transient cleanup boundary",
        "example_location": "apps/ene-core/src/serve.rs:2415-2456",
        "usage_guidance": "drop registry state from the existing close/supersede sweep, not ad hoc handlers"
      },
      {
        "pattern": "persist client identity after accepted proof",
        "example_location": "crates/ene-client/src/transport.rs:223-290",
        "usage_guidance": "retain this order when the secret source becomes PairingProvision"
      },
      {
        "pattern": "non-secret requester facts",
        "example_location": "crates/ene-local-control/src/lib.rs:138-168",
        "usage_guidance": "keep pending/device IDs but no secret or delivery-receipt claim"
      }
    ],
    "files_to_modify": [
      {
        "file": "crates/ene-api/Cargo.toml",
        "symbols": [
          "dependencies"
        ],
        "intended_change": "inherit zeroize for auth-secret DTO custody"
      },
      {
        "file": "crates/ene-api/src/v1/handshake.rs",
        "symbols": [
          "PairingRequest",
          "PairingResult",
          "PairingProvision",
          "PairingProvisionSecret"
        ],
        "intended_change": "remove polling/Paired response and add the redacted, zeroizing auth-only provision frame"
      },
      {
        "file": "crates/ene-api/src/v1/payload.rs",
        "symbols": [
          "WirePayload",
          "WirePayload::message_type"
        ],
        "intended_change": "register PairingProvision as an authentication frame"
      },
      {
        "file": "crates/ene-api/tests/dto.rs",
        "symbols": [
          "pairing DTO round trips"
        ],
        "intended_change": "cover the new DTO and removed request field"
      },
      {
        "file": "crates/ene-client/Cargo.toml",
        "symbols": [
          "dependencies"
        ],
        "intended_change": "inherit zeroize for framing buffers and in-memory provision custody"
      },
      {
        "file": "crates/ene-client/src/lib.rs",
        "symbols": [
          "crate-level handshake contract"
        ],
        "intended_change": "document live-origin provisioning instead of polling/bootstrap"
      },
      {
        "file": "crates/ene-client/src/transport.rs",
        "symbols": [
          "Client::connect",
          "Client::begin_connect (new)",
          "PendingPairingClient (new)",
          "read_frame",
          "write_frame"
        ],
        "intended_change": "retain the original stream, receive PairingProvision, continue capability/authentication, and zeroize frame buffers"
      },
      {
        "file": "crates/ene-client/src/device.rs",
        "symbols": [
          "StoredDevice",
          "pending/bootstrap helpers"
        ],
        "intended_change": "remove ENE_PAIRING_SECRET and client-pending.json; keep persistence only after accepted authentication"
      },
      {
        "file": "crates/ene-client/src/frames.rs",
        "symbols": [
          "pairing_frame",
          "pending_guidance",
          "missing_secret_guidance"
        ],
        "intended_change": "send only a fresh PairingRequest and remove bootstrap-era guidance"
      },
      {
        "file": "crates/ene-client/src/session.rs",
        "symbols": [
          "SessionState pairing secret"
        ],
        "intended_change": "hold provisioned material in a redacted, zeroizing type"
      },
      {
        "file": "crates/ene-client/src/tests.rs",
        "symbols": [
          "pairing/auth tests"
        ],
        "intended_change": "replace polling/environment expectations with live provision and redaction expectations"
      },
      {
        "file": "crates/ene-credential/src/lib.rs",
        "symbols": [
          "pairing re-exports"
        ],
        "intended_change": "export the simplified pairing repository result and zeroizing secret type"
      },
      {
        "file": "crates/ene-credential/src/pairing.rs",
        "symbols": [
          "DevicePairingRepository",
          "DevicePairingStatus",
          "PairingSecretMaterial (new)"
        ],
        "intended_change": "make requests always fresh, make approval one-shot, add abandon-by-origin, and remove rotation/poll contracts"
      },
      {
        "file": "crates/ene-store/src/migrate.rs",
        "symbols": [
          "paired_device schema",
          "pairing_pending schema"
        ],
        "intended_change": "drop paired_device.pending_id in the fresh schema"
      },
      {
        "file": "crates/ene-store/src/credential.rs",
        "symbols": [
          "request_pairing",
          "approve_pending",
          "abandon_pending_by_origin (new)"
        ],
        "intended_change": "remove approved-ID polling and rotation; retain origin-CAS approval and add disconnect cleanup"
      },
      {
        "file": "crates/ene-store/src/erasure/remainder.rs",
        "symbols": [
          "paired_device remainder columns"
        ],
        "intended_change": "remove the deleted pending_id column from erasure accounting"
      },
      {
        "file": "crates/ene-store/src/tests.rs",
        "symbols": [
          "device pairing repository tests"
        ],
        "intended_change": "replace poll/rotation tests with one-shot origin-bound approval and abandon tests"
      },
      {
        "file": "crates/ene-store/src/tests/erasure_owners.rs",
        "symbols": [
          "pairing fixtures"
        ],
        "intended_change": "adapt fixtures to the simplified request result"
      },
      {
        "file": "crates/ene-local-control/src/lib.rs",
        "symbols": [
          "ControlOutcome::DeviceApproved",
          "RequesterOutcome::DeviceApproved"
        ],
        "intended_change": "make both outcomes non-secret approval facts and document that they do not acknowledge socket receipt"
      },
      {
        "file": "apps/ene-core/Cargo.toml",
        "symbols": [
          "dependencies"
        ],
        "intended_change": "inherit zeroize for queued and encoded secret material"
      },
      {
        "file": "apps/ene-core/src/lib.rs",
        "symbols": [
          "module declarations"
        ],
        "intended_change": "register the new pairing-delivery module"
      },
      {
        "file": "apps/ene-core/src/pairing_delivery.rs",
        "symbols": [
          "PairingDeliveryRegistry (new)",
          "PairingDeliveryReceiver (new)"
        ],
        "intended_change": "provide a capacity-one connection slot, pending binding, single-winner claim, queue, and zeroizing cleanup"
      },
      {
        "file": "apps/ene-core/src/serve.rs",
        "symbols": [
          "HostHandle",
          "HostHandle::approve_device",
          "HostHandle::close_connection",
          "drop_connection_transient_state"
        ],
        "intended_change": "own the registry, publish only to the claimed live origin, abandon disconnected pendings, and return no secret"
      },
      {
        "file": "apps/ene-core/src/serve/handshake.rs",
        "symbols": [
          "HostHandle::pair"
        ],
        "intended_change": "open one fresh pending and bind it to the registered origin; never poll or mark Paired here"
      },
      {
        "file": "apps/ene-core/src/conn.rs",
        "symbols": [
          "ConnectionTable::note_paired",
          "serve_connection",
          "write_response",
          "read_frames"
        ],
        "intended_change": "register before reads, serialize provision delivery on the socket owner, transition Accepted to Paired immediately before write, and zeroize framing buffers"
      },
      {
        "file": "apps/ene-core/src/conn/tests.rs",
        "symbols": [
          "serve_connection fixtures",
          "connection phase tests"
        ],
        "intended_change": "cover successful delivery, bounded queueing, phase order, disconnect/write races, and no replay"
      },
      {
        "file": "apps/ene-core/src/conn/shutdown_tests.rs",
        "symbols": [
          "PairingRequest fixture"
        ],
        "intended_change": "adapt the request shape and ensure shutdown drops queued material"
      },
      {
        "file": "apps/ene-core/src/serve/tests.rs",
        "symbols": [
          "pairing fixtures",
          "approve_device callers"
        ],
        "intended_change": "remove poll/reapproval tests and exercise registry-bound approval facts"
      },
      {
        "file": "apps/ene-core/src/host_control.rs",
        "symbols": [
          "execute_pending"
        ],
        "intended_change": "remove pairing_secret from confirmation output and distinguish approval facts from unavailable delivery"
      },
      {
        "file": "apps/ene-core/src/host_lock.rs",
        "symbols": [
          "pairing test fixtures"
        ],
        "intended_change": "adapt request/approval fixtures without restoring offline mutation"
      },
      {
        "file": "apps/ene-core/src/targeted_deletion.rs",
        "symbols": [
          "pairing fixtures"
        ],
        "intended_change": "adapt test-only setup to the simplified repository API"
      },
      {
        "file": "apps/ene-core/tests/stage5_e2e.rs",
        "symbols": [
          "approve_and_provision"
        ],
        "intended_change": "keep the original client process/socket alive while approval occurs"
      },
      {
        "file": "apps/ene-core/tests/stage5_windows_pipe_e2e.rs",
        "symbols": [
          "approve_and_provision"
        ],
        "intended_change": "exercise origin-only provision over the named pipe without a pending file"
      },
      {
        "file": "apps/ene-core/tests/stage6_e2e.rs",
        "symbols": [
          "approve_and_provision"
        ],
        "intended_change": "replace out-of-band secret injection with live provision"
      },
      {
        "file": "apps/ene-core/tests/stage7_a1.rs",
        "symbols": [
          "approve_and_provision",
          "control outcome assertions"
        ],
        "intended_change": "prove neither requester nor confirmation outcomes carry the pairing secret"
      },
      {
        "file": "apps/ene-core/tests/vertical_slice.rs",
        "symbols": [
          "approve_and_provision",
          "pairing bootstrap scenarios"
        ],
        "intended_change": "remove ENE_PAIRING_SECRET, polling, and rotation scenarios; add same-socket and failure semantics"
      },
      {
        "file": "apps/ene-desktop/src/session.rs",
        "symbols": [
          "connect",
          "DesktopConnect",
          "connect_or_pending"
        ],
        "intended_change": "return an owning pending transport rather than a bare pending ID or bootstrap secret"
      },
      {
        "file": "apps/ene-desktop/src/ui/runtime.rs",
        "symbols": [
          "connect_or_begin_pairing",
          "confirm_owner"
        ],
        "intended_change": "retain the pending client across Owner confirmation, then complete that same connection"
      },
      {
        "file": "apps/ene-desktop/tests/stage7_b.rs",
        "symbols": [
          "first-run pairing tests"
        ],
        "intended_change": "assert same-connection adoption and a non-secret control outcome"
      },
      {
        "file": "apps/ene-ctl/src/main.rs",
        "symbols": [
          "run_command client connection"
        ],
        "intended_change": "display the non-secret pending ID while awaiting the retained original connection"
      },
      {
        "file": "docs/implementation/stages/stage-7.md",
        "symbols": [
          "A1 trust-boundary status"
        ],
        "intended_change": "mark IPC 9.2 provision delivery complete while preserving concurrent VRM edits"
      },
      {
        "file": "docs/implementation/PROGRESS.md",
        "symbols": [
          "Stage 7 remaining work"
        ],
        "intended_change": "remove only the completed pairing-provision gap while preserving concurrent VRM edits"
      }
    ],
    "tests": [
      {
        "file": "crates/ene-api/src/v1/handshake.rs and crates/ene-api/tests/dto.rs",
        "scenarios": [
          "PairingProvision serialize/deserialize -> same device ID and secret",
          "Debug on provision/secret -> redacted text only",
          "PairingRequest -> no polling field"
        ]
      },
      {
        "file": "crates/ene-store/src/tests.rs",
        "scenarios": [
          "fresh request -> unique pending bound to origin",
          "wrong origin approval -> None and pending remains",
          "first approval -> one device/secret and pending consumed",
          "second approval of consumed ID -> None, never secret rotation",
          "abandon origin -> only that origin's unapproved pendings removed"
        ]
      },
      {
        "file": "apps/ene-core/src/conn/tests.rs",
        "scenarios": [
          "pending response -> approval -> PairingProvision on the same socket -> Accepted-to-Paired before capability",
          "disconnect wins before claim -> no approval/no delivery and a new connection cannot receive the old provision",
          "approval commits and queues, then write fails -> cleanup/no replay and no receipt claim",
          "two approvals race -> one store mutation and at most one queued frame",
          "shutdown with queued provision -> secret-bearing queue is dropped"
        ]
      },
      {
        "file": "apps/ene-core/src/serve/tests.rs and apps/ene-core/src/host_control.rs tests",
        "scenarios": [
          "confirmation and requester DeviceApproved -> pending/device facts only",
          "unknown or unregistered pending -> DeviceUnknown/Unavailable, never a secret",
          "direct handler tests cannot bypass the live-origin registry"
        ]
      },
      {
        "file": "crates/ene-client/src/tests.rs",
        "scenarios": [
          "PendingPairingClient exposes only pending ID before provision",
          "provision -> capability/auth -> persist only after AuthResult::Accepted",
          "EOF, wrong frame, denial, or rejected auth -> no device-file write and secret dropped",
          "encoded/decoded secret buffers are explicitly zeroized"
        ]
      },
      {
        "file": "apps/ene-desktop/tests/stage7_b.rs",
        "scenarios": [
          "first-run GUI keeps pending transport while confirmation runs",
          "DeviceApproved contains no secret and the retained connection becomes the adopted client",
          "delivery failure after approval -> explicit client error, no reconnect/bootstrap"
        ]
      },
      {
        "file": "apps/ene-core/tests/stage5_e2e.rs, apps/ene-core/tests/stage5_windows_pipe_e2e.rs, apps/ene-core/tests/stage6_e2e.rs, apps/ene-core/tests/stage7_a1.rs, apps/ene-core/tests/vertical_slice.rs",
        "scenarios": [
          "Unix and Windows first run -> process remains live, approval provisions that socket, authentication succeeds",
          "no ENE_PAIRING_SECRET or client-pending.json setup",
          "lost origin -> fresh pairing required and another requester/connection never receives old material"
        ]
      }
    ],
    "verification_commands": [
      "cargo fmt --all -- --check",
      "cargo test -p ene-api",
      "cargo test -p ene-store",
      "cargo test -p ene-client",
      "cargo test -p ene-core",
      "cargo test -p ene-desktop",
      "cargo clippy --workspace --all-targets -- -D warnings",
      "cargo test --workspace",
      "cargo doc --workspace --no-deps",
      "rg -n 'ENE_PAIRING_SECRET|client-pending\\.json|connect_with_bootstrap|PairingResult::Paired' crates apps --glob '*.rs'"
    ],
    "risks": [
      "Approval/auth-store persistence and socket delivery cannot be one atomic transaction; post-commit delivery failure must remain explicit and non-replayable.",
      "A disconnect can race slot claim, store commit, queueing, phase transition, and write; barrier-controlled tests must cover both orderings at each ownership handoff.",
      "WirePayload is Clone, so PairingProvision can acquire transient copies; every secret wrapper and framing buffer must redact Debug and zeroize on drop.",
      "Public pairing APIs and the fresh schema intentionally break the polling/bootstrap implementation; all direct dependents and fixtures must move in one cutover.",
      "Windows named-pipe behavior must remain equivalent to Unix despite local development usually exercising Unix first."
    ],
    "assumptions": [
      "Reverify that the existing UUID-text secret generation and HMAC-SHA256 proof remain the selected cryptographic format before editing; this task changes custody/delivery, not the proof algorithm.",
      "Reverify no product timeout is specified; pending waits for the original connection lifetime rather than inventing a timeout.",
      "Apply schema changes directly to the fresh schema and do not add migrations or compatibility shims, per repository guidance.",
      "Before editing docs/implementation/PROGRESS.md or docs/implementation/stages/stage-7.md, preserve their pinned unstaged VRM changes and remove only the pairing gap text."
    ],
    "open_questions": [],
    "avoid": [
      "Do not repeat full repository discovery.",
      "Do not write a pairing secret to requester/confirmation outcomes, stdout/stderr, logs, Debug, business DTOs, environment variables, or client-pending.json.",
      "Do not reconnect, poll an approved pending ID, rotate by reapproval, or deliver to a replacement connection after origin loss.",
      "Do not let approval code write the socket or directly mutate ConnectionTable phase.",
      "Do not automatically retry an auth-store save or socket delivery whose outcome is unknown.",
      "Do not add migration, legacy, or bootstrap compatibility paths.",
      "Do not overwrite the user's concurrent VRM asset/document changes or modify unrelated Stage 7 product-character work."
    ]
  }
}
```

## 12. Assumptions and Open Questions

### Assumptions to reverify before implementation

1. [assumed] Preserve the current UUID-text pairing secret generation and HMAC-SHA256 proof algorithm; this slice changes custody and delivery, not cryptography. Recheck `crates/ene-store/src/credential.rs:60` and `crates/ene-credential/src/pairing.rs:164+` before editing.
2. [assumed] No product timeout is specified for first pairing. The pending wait therefore lasts for the original connection lifetime; do not invent a timer. Recheck the design documents and current acceptance requirements before implementation.
3. [assumed] Fresh schema changes are acceptable without migration or compatibility behavior, as required by repository guidance. Do not preserve old databases, pending files, environment variables, or poll frames.
4. [assumed] The two cited implementation documents' unstaged VRM edits are user work. Recheck their worktree digests before touching them and preserve every unrelated line.

### Open questions

No blocking product/design question remains. The implementation must name internal registry/error types consistently with surrounding code, but may not change the settled behavior above.

### Explicitly deferred

- Device revocation/garbage collection for a durable approved record whose provision was not received remains separate; do not add an implicit rollback, secret replay, or rotation path here.
- Stage 7 overlay/VRM/runtime acceptance and platform probes are unrelated to pairing provision delivery and must not be folded into this change.
- A separate delivery ACK is not introduced; accepted authentication remains the proof of receipt/use.

## 13. Definition of Done

1. `PairingProvision` is an auth-specific frame with device ID plus redacted/zeroizing secret, and all secret-bearing buffers/types redact `Debug` and zeroize on release.
2. A fresh pending is bound to one registered live `ConnectionWireId`; approval can enqueue exactly once only for that origin.
3. Only `serve_connection` advances `Accepted → Paired` and writes the provision, immediately before delivery on the originating socket.
4. Disconnect, supersession, shutdown, queue/phase/encode/write failure, duplicate approval, and unknown/wrong-origin approval never redirect, replay, rotate, or expose the secret.
5. `PairingRequest.pending_id`, `PairingResult::Paired`, `paired_device.pending_id`, approved-ID polling, reapproval rotation, `client-pending.json`, `ENE_PAIRING_SECRET`, and `connect_with_bootstrap` are absent from production Rust code.
6. Requester and confirmation `DeviceApproved` outcomes contain only non-secret approval facts; CLI output/logs/errors contain no pairing secret and do not claim client receipt.
7. CLI and desktop keep the original pending connection alive; capability negotiation, proof authentication, and persistence continue on that connection, with persistence only after `AuthResult::Accepted`.
8. Deterministic unit/race tests and Unix/Windows E2E fixtures cover success and all listed failure orderings without sleep-based race assertions or out-of-band secret injection.
9. Focused tests, workspace fmt, Clippy with `-D warnings`, full tests, and rustdoc pass; Windows named-pipe CI passes before completion is reported.
10. Stage 7 status removes the IPC §9.2 gap while preserving all unrelated user VRM/asset changes; no production placeholder, migration shim, or unrelated VRM work is added.
