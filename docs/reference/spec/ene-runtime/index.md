# `ene-runtime` Crate Role & Modules Overview

The `ene-runtime` crate provides the top-level public interface (facade) for consumer applications (`ene-cli` and `ene-desktop`). It encapsulates the actor-based concurrent execution task `EneActor` into a thread-safe, cloneable `EneHandle`, orchestrating turn lifecycles, memory storage operations, and external tool processes.

---

## 1. Dependencies and Boundaries

### Physical Dependencies (`Cargo.toml`)
- **External Dependencies**: `tokio`, `tokio-stream`, `tokio-util`, `serde`, `serde_json`, `chrono`, `tracing`, `async-trait`
- **Workspace Dependencies**: `ene-ai`, `ene-config`, `ene-mind`, `ene-store`, `ene-plugin-host`, `ene-plugin-proto`, `ene-tool-rag`
- **Architectural Rules**: No other crate can depend on `ene-runtime`. To avoid circular references, lower-level domain crates (e.g. `ene-mind` and `ene-store`) must not import `ene-runtime`.

### Logical Isolation
`ene-runtime` contains no business logic regarding cognitive state transitions, emotional decay, factual memory pruning, or SQLite schemas. It delegates those tasks to `ene-mind` and `ene-store`, serving strictly as an event router, state keeper, and IPC server socket host.

---

## 2. Module Directory

The crate is partitioned into the following modules:

```text
ene-runtime/src/
├── lib.rs              # Crate root. Re-exports key APIs
├── bootstrap.rs        # Host bootstrapper functions
├── db_server.rs        # Tool IPC DB proxy socket server
├── diagnostics.rs      # Diagnostic inspection facade
├── error.rs            # Crate-wide error enum (EneRuntimeError)
├── handle.rs           # Thread-safe EneHandle & EneActor state loop
├── message_builder.rs  # LLM system prompt & message assembly
├── proactive.rs        # Proactive companion behavior loop
├── streaming.rs        # Conversation streaming loop & tool execution
├── streaming_cognitive.rs # Cognitive streaming pipeline
└── types.rs            # TurnId, RequestId, RunError, CancelError types
```

---

## 3. Host Bootstrapping Helper (Bootstrap)

The `bootstrap.rs` module provides helpers to read the active configuration and character card from the disk, preparing ready-to-run handles.

### Function Specifications

#### `open_from_disk`
*   **Signature**:
    ```rust
    pub async fn open_from_disk() -> Result<(EneHandle, EneConfig), EneRuntimeError>
    ```
*   **Description**: A fail-hard bootstrap utility designed for CLI apps. It loads `config.json` via `ConfigStore`, loads character cards via `load_character_card`, and initializes the handle.
*   **Connections**: `ene_config::ConfigStore`, `ene_config::load_character_card`, `EneHandle::open`.

#### `open_with_config`
*   **Signature**:
    ```rust
    pub async fn open_with_config(config: EneConfig) -> Result<EneHandle, EneRuntimeError>
    ```
*   **Description**: Warm starts the actor using an already-loaded configuration object. Preferred by desktop applications.
*   **Connections**: `ene_config::load_character_card`, `EneHandle::open`.

#### `open_ready`
*   **Signature**:
    ```rust
    pub async fn open_ready(
        config: EneConfig,
        card: CharacterCardV3,
    ) -> Result<EneHandle, EneRuntimeError>
    ```
*   **Description**: Instantiates a handle using raw struct values directly. Avoids disk and file system operations. Used in unit and integration testing.
*   **Connections**: `EneHandle::open`.
