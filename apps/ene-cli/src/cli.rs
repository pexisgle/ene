use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Interactive CLI REPL for chatting with AI characters, testing tools,
/// and managing memory/sessions.
///
/// With no subcommand, starts the interactive REPL. With a subcommand, runs a
/// single non-interactive operation and exits — suitable for CI pipelines and
/// shell scripts.
#[derive(Debug, Parser)]
#[command(name = "ene", version, about, long_about = None)]
pub struct Cli {
    /// Path to a settings.json file to load instead of the default location.
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Character card name or path to use instead of the configured default.
    #[arg(long, global = true, value_name = "NAME")]
    pub character: Option<String>,

    /// UI language override (en or ja). Defaults to system locale.
    #[arg(long, global = true, value_name = "LANG")]
    pub lang: Option<String>,

    /// Non-interactive subcommand. Omit to start the interactive REPL.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Non-interactive subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run a single prompt and stream the response, then exit.
    Run {
        /// Prompt text. If omitted, the prompt is read from stdin.
        prompt: Vec<String>,

        /// Emit one JSON object per line (streaming events) on stdout.
        #[arg(long)]
        jsonl: bool,

        /// Emit a single JSON summary object on stdout instead of streaming.
        #[arg(long, conflicts_with = "jsonl")]
        json: bool,

        /// Abort if the turn does not complete within this many seconds.
        #[arg(long, value_name = "SECONDS")]
        timeout: Option<u64>,

        /// Automatically approve side-effecting tool operations. Without this,
        /// a permission gate fails the run instead of prompting.
        #[arg(long)]
        yes: bool,
    },
    /// Manage and call tools.
    Tool {
        /// Tool subcommand.
        #[command(subcommand)]
        action: ToolAction,

        /// Emit machine-readable JSON on stdout.
        #[arg(long, global = true)]
        json: bool,
    },
    /// Manage conversation sessions.
    Session {
        /// Session subcommand.
        #[command(subcommand)]
        action: SessionAction,

        /// Emit machine-readable JSON on stdout.
        #[arg(long, global = true)]
        json: bool,
    },
    /// List discovered characters.
    Characters {
        /// Character subcommand.
        #[command(subcommand)]
        action: CharactersAction,

        /// Emit machine-readable JSON on stdout.
        #[arg(long, global = true)]
        json: bool,
    },
    /// Inspect and manage cognitive memories.
    Memory {
        /// Memory subcommand.
        #[command(subcommand)]
        action: MemoryAction,

        /// Emit machine-readable JSON on stdout.
        #[arg(long, global = true)]
        json: bool,
    },
    /// Run environment health checks.
    Doctor {
        /// Emit machine-readable JSON on stdout.
        #[arg(long)]
        json: bool,
    },
    /// Backup, restore, and integrity-check the memory database.
    Store {
        /// Store subcommand.
        #[command(subcommand)]
        action: StoreAction,

        /// Emit machine-readable JSON on stdout.
        #[arg(long, global = true)]
        json: bool,
    },
    /// Build, sign, and verify signed artifact catalogs (publisher side).
    Catalog {
        /// Catalog subcommand.
        #[command(subcommand)]
        action: CatalogAction,

        /// Emit machine-readable JSON on stdout.
        #[arg(long, global = true)]
        json: bool,
    },
}

/// `tool` subcommands.
#[derive(Debug, Subcommand)]
pub enum ToolAction {
    /// List all registered tools.
    List,
    /// Search registered tools.
    Search {
        /// Search query.
        query: String,
    },
    /// Show detailed help for a tool.
    Help {
        /// Tool name.
        name: String,
    },
    /// Call a tool directly with JSON arguments.
    Call {
        /// Tool name.
        name: String,
        /// JSON-encoded arguments (defaults to `{}`).
        #[arg(default_value = "{}")]
        arguments: String,
    },
}

/// `session` subcommands.
#[derive(Debug, Subcommand)]
pub enum SessionAction {
    /// List stored sessions (newest first).
    List {
        /// Include archived sessions.
        #[arg(long)]
        archived: bool,
    },
    /// Export a session to a versioned, redacted JSON bundle.
    Export {
        /// Session id.
        id: String,
    },
    /// Import a session from a JSON export file.
    Import {
        /// Path to the export file.
        path: PathBuf,
    },
    /// Full-text search over stored conversation messages.
    Search {
        /// Search query.
        query: String,
    },
    /// Archive a session.
    Archive {
        /// Session id.
        id: String,
    },
    /// Unarchive a session.
    Unarchive {
        /// Session id.
        id: String,
    },
}

/// `characters` subcommands.
#[derive(Debug, Subcommand)]
pub enum CharactersAction {
    /// List discovered characters.
    List,
    /// Import a character card (PNG or CHARX) into the characters directory.
    Import {
        /// Path to the card file.
        path: PathBuf,
    },
}

/// `memory` subcommands.
#[derive(Debug, Subcommand)]
pub enum MemoryAction {
    /// List typed memories.
    List {
        /// Filter by memory kind.
        #[arg(long, value_name = "KIND")]
        kind: Option<String>,
    },
    /// Show full typed-memory details.
    Inspect {
        /// Memory id.
        id: i64,
    },
    /// Search typed memories (hybrid score + breakdown).
    Search {
        /// Search query.
        query: String,
    },
}

/// `store` subcommands.
#[derive(Debug, Subcommand)]
pub enum StoreAction {
    /// Create a timestamped file backup of the memory database.
    Backup,
    /// List existing `{db}.bak.*` backups (newest first).
    ListBackups,
    /// Restore the memory database from a backup file (exits afterward).
    Restore {
        /// Path to the backup file.
        path: PathBuf,
        /// Confirm the destructive restore.
        #[arg(long)]
        yes: bool,
    },
    /// Run `PRAGMA integrity_check` on the open database.
    Integrity,
}

/// `catalog` subcommands.
#[derive(Debug, Subcommand)]
pub enum CatalogAction {
    /// Generate a fresh Ed25519 key pair for signing catalogs.
    Keygen {
        /// Directory to write `key-id.txt`, `private.hex`, and `public.hex`.
        #[arg(long, value_name = "DIR")]
        out_dir: std::path::PathBuf,
    },
    /// Build and sign a catalog from a spec file.
    Build {
        /// Catalog spec JSON (see `scripts/publish-catalog.sh` for the shape).
        #[arg(long, value_name = "SPEC")]
        spec: std::path::PathBuf,
        /// Key id written into the signed catalog.
        #[arg(long, value_name = "ID")]
        key_id: String,
        /// Hex-encoded Ed25519 private key.
        #[arg(long, value_name = "HEX")]
        key_hex: String,
        /// Override the catalog version (defaults to the spec's `version`).
        #[arg(long, value_name = "N")]
        version: Option<u64>,
        /// Validity window in hours from now (used when the spec has no
        /// `expires_at_ms`).
        #[arg(long, default_value_t = 24 * 30, value_name = "HOURS")]
        expires_hours: u64,
        /// Output catalog JSON path.
        #[arg(long, value_name = "OUT")]
        out: std::path::PathBuf,
    },
    /// Bump an existing catalog's version (re-signing it).
    Update {
        /// Existing signed catalog JSON.
        #[arg(long, value_name = "CATALOG")]
        catalog: std::path::PathBuf,
        /// Key id written into the signed catalog.
        #[arg(long, value_name = "ID")]
        key_id: String,
        /// Hex-encoded Ed25519 private key.
        #[arg(long, value_name = "HEX")]
        key_hex: String,
        /// Explicit version instead of the previous version + 1.
        #[arg(long, value_name = "N")]
        version: Option<u64>,
        /// Extend the validity window to this many hours from now.
        #[arg(long, value_name = "HOURS")]
        expires_hours: Option<u64>,
        /// Output catalog JSON path (defaults to the input path).
        #[arg(long, value_name = "OUT")]
        out: Option<std::path::PathBuf>,
    },
    /// Verify a catalog's signature, expiry, and rollback safety.
    Verify {
        /// Signed catalog JSON to verify.
        #[arg(long, value_name = "CATALOG")]
        catalog: std::path::PathBuf,
        /// Key id the catalog claims to be signed with.
        #[arg(long, value_name = "ID")]
        key_id: String,
        /// Hex-encoded Ed25519 public key.
        #[arg(long, value_name = "HEX")]
        key_hex: String,
        /// Optional installer `state.json` used for rollback checks.
        #[arg(long, value_name = "STATE")]
        state: Option<std::path::PathBuf>,
    },
}
