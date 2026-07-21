//! Doctor diagnostic command (#179).
//!
//! Checks environment health across runtime, config, AI provider,
//! embedding, store, tool registry, and assets categories. Secrets
//! (API keys, private paths) are masked in all output.

use crate::commands::{CliCommand, CliError};
use crate::context::AppContext;
use crate::style;
use async_trait::async_trait;
use ene_runtime::{AiConfig, EneDiagnostics, EneStateSnapshot, StoreConfig};
use serde::Serialize;

/// Diagnostic category grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CheckCategory {
    /// Actor / runtime health.
    Runtime,
    /// Configuration validity.
    Config,
    /// AI provider connectivity.
    AiProvider,
    /// Embedding backend.
    Embedding,
    /// Memory store.
    Store,
    /// Tool registry.
    ToolRegistry,
    /// Assets directory.
    Assets,
}

impl std::fmt::Display for CheckCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::Runtime => "Runtime",
            Self::Config => "Config",
            Self::AiProvider => "AI Provider",
            Self::Embedding => "Embedding",
            Self::Store => "Store",
            Self::ToolRegistry => "Tool Registry",
            Self::Assets => "Assets",
        };
        write!(f, "{label}")
    }
}

/// Status of a single diagnostic check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CheckStatus {
    /// Check passed.
    Ok,
    /// Non-fatal issue detected.
    Warning,
    /// Fatal issue detected.
    Error,
    /// Check was not applicable.
    Skipped,
}

/// A single diagnostic check result.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct CheckResult {
    /// Category this check belongs to.
    pub(crate) category: CheckCategory,
    /// Short check name.
    pub(crate) name: String,
    /// Outcome.
    pub(crate) status: CheckStatus,
    /// Human-readable detail.
    pub(crate) message: String,
    /// Suggested fix, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) remediation: Option<String>,
}

impl CheckResult {
    fn ok(category: CheckCategory, name: &str, message: impl Into<String>) -> Self {
        Self {
            category,
            name: name.to_string(),
            status: CheckStatus::Ok,
            message: message.into(),
            remediation: None,
        }
    }

    fn warning(
        category: CheckCategory,
        name: &str,
        message: impl Into<String>,
        remediation: impl Into<String>,
    ) -> Self {
        Self {
            category,
            name: name.to_string(),
            status: CheckStatus::Warning,
            message: message.into(),
            remediation: Some(remediation.into()),
        }
    }

    fn error(
        category: CheckCategory,
        name: &str,
        message: impl Into<String>,
        remediation: Option<&str>,
    ) -> Self {
        Self {
            category,
            name: name.to_string(),
            status: CheckStatus::Error,
            message: message.into(),
            remediation: remediation.map(str::to_string),
        }
    }

    fn skipped(category: CheckCategory, name: &str, message: impl Into<String>) -> Self {
        Self {
            category,
            name: name.to_string(),
            status: CheckStatus::Skipped,
            message: message.into(),
            remediation: None,
        }
    }
}

/// Aggregate report for `--json` output.
#[derive(Debug, Serialize)]
struct DoctorReport {
    checks: Vec<CheckResult>,
    summary: DoctorSummary,
}

/// Counts per status.
#[derive(Debug, Serialize)]
struct DoctorSummary {
    total: usize,
    ok: usize,
    warnings: usize,
    errors: usize,
    skipped: usize,
}

impl DoctorSummary {
    fn from_checks(checks: &[CheckResult]) -> Self {
        Self {
            total: checks.len(),
            ok: checks
                .iter()
                .filter(|c| c.status == CheckStatus::Ok)
                .count(),
            warnings: checks
                .iter()
                .filter(|c| c.status == CheckStatus::Warning)
                .count(),
            errors: checks
                .iter()
                .filter(|c| c.status == CheckStatus::Error)
                .count(),
            skipped: checks
                .iter()
                .filter(|c| c.status == CheckStatus::Skipped)
                .count(),
        }
    }
}

/// Mask an API key, showing at most a short prefix and suffix.
fn mask_api_key(key: &str) -> String {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return "[not set]".to_string();
    }
    if trimmed.chars().count() <= 8 {
        return "[redacted]".to_string();
    }
    let prefix: String = trimmed.chars().take(3).collect();
    let suffix: String = trimmed
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{prefix}…{suffix}")
}

/// Mask an absolute path by replacing the home directory with `~`.
fn mask_path(path: &std::path::Path) -> String {
    let path_str = path.display().to_string();
    for var in ["HOME", "USERPROFILE"] {
        if let Ok(home) = std::env::var(var)
            && !home.is_empty()
            && let Some(rest) = path_str.strip_prefix(&home)
        {
            return format!("~{rest}");
        }
    }
    if path.is_absolute() {
        path.file_name()
            .map_or_else(|| "…".to_string(), |n| format!("…/{}", n.display()))
    } else {
        path_str
    }
}

/// The `/doctor` command.
pub(crate) struct DoctorCommand;

#[async_trait]
impl CliCommand for DoctorCommand {
    fn name(&self) -> &'static str {
        "/doctor"
    }

    fn description(&self) -> &'static str {
        "Run environment health checks"
    }

    fn usage(&self) -> &'static str {
        "/doctor [--json]"
    }

    async fn execute(&self, arg: &str, ctx: &mut AppContext) -> Result<(), CliError> {
        let json_mode = arg.trim() == "--json";
        let checks = run_checks(ctx).await;

        if json_mode {
            let report = DoctorReport {
                summary: DoctorSummary::from_checks(&checks),
                checks,
            };
            let json = serde_json::to_string_pretty(&report).map_err(|e| {
                CliError::ExecutionFailed(format!("JSON serialization failed: {e}"))
            })?;
            println!("{json}");
        } else {
            print_human(&checks);
        }

        Ok(())
    }
}

async fn run_checks(ctx: &mut AppContext) -> Vec<CheckResult> {
    let mut results = Vec::new();
    let diag = ctx.handle.diagnostics();

    let snapshot = match diag.get_snapshot().await {
        Ok(s) => {
            results.push(CheckResult::ok(
                CheckCategory::Runtime,
                "actor",
                format!(
                    "Actor responsive (session {}, turn {})",
                    s.session_id, s.current_turn_count
                ),
            ));
            Some(s)
        }
        Err(e) => {
            results.push(CheckResult::error(
                CheckCategory::Runtime,
                "actor",
                format!("Actor not responding: {e}"),
                Some("Restart the CLI"),
            ));
            None
        }
    };

    let Some(snapshot) = snapshot else {
        for category in [
            CheckCategory::Config,
            CheckCategory::AiProvider,
            CheckCategory::Embedding,
            CheckCategory::Store,
        ] {
            results.push(CheckResult::skipped(
                category,
                "skipped",
                "Skipped (actor unavailable)",
            ));
        }
        check_tool_registry(&mut results, diag).await;
        check_assets(&mut results);
        return results;
    };

    check_config(&mut results, &snapshot);
    check_ai_provider(&mut results, &snapshot).await;
    check_embedding(&mut results, &snapshot);
    check_store(&mut results, diag, &snapshot);
    check_tool_registry(&mut results, diag).await;
    check_assets(&mut results);

    results
}

fn check_config(results: &mut Vec<CheckResult>, snapshot: &EneStateSnapshot) {
    let card_name = snapshot.card_name.as_str();
    if card_name.is_empty() {
        results.push(CheckResult::warning(
            CheckCategory::Config,
            "character",
            "No character card loaded",
            "Use /card <path> to load a character card",
        ));
    } else {
        results.push(CheckResult::ok(
            CheckCategory::Config,
            "character",
            format!("Character card loaded: {card_name}"),
        ));
    }
}

async fn check_ai_provider(results: &mut Vec<CheckResult>, snapshot: &EneStateSnapshot) {
    let ai_config = snapshot
        .config
        .get_section::<AiConfig>()
        .unwrap_or_default();

    let chat = match ai_config.resolve_chat() {
        Ok(c) => c,
        Err(e) => {
            results.push(CheckResult::error(
                CheckCategory::AiProvider,
                "resolve",
                format!("Cannot resolve chat provider: {e}"),
                Some("Check ai.providers and ai.tasks.chat in settings.json"),
            ));
            return;
        }
    };

    if chat.api_key.trim().is_empty() {
        results.push(CheckResult::warning(
            CheckCategory::AiProvider,
            "api_key",
            "No API key configured for chat provider",
            "Set OPENAI_API_KEY env var or configure ai.providers.<name>.api_key in settings.json",
        ));
        return;
    }

    match ene_ai::validate_api_key(&chat.base_url, &chat.api_key).await {
        Ok(()) => {
            results.push(CheckResult::ok(
                CheckCategory::AiProvider,
                "connectivity",
                format!(
                    "Provider reachable (model: {}, key: {})",
                    chat.model,
                    mask_api_key(&chat.api_key)
                ),
            ));
        }
        Err(e) => {
            results.push(CheckResult::error(
                CheckCategory::AiProvider,
                "connectivity",
                format!("Provider check failed: {e}"),
                Some("Verify API key and base URL; check network connectivity"),
            ));
        }
    }
}

fn check_embedding(results: &mut Vec<CheckResult>, snapshot: &EneStateSnapshot) {
    let ai_config = snapshot
        .config
        .get_section::<AiConfig>()
        .unwrap_or_default();

    match ai_config.resolve_embedding() {
        Ok(embed) => {
            if let Some(local) = embed.as_local() {
                results.push(CheckResult::ok(
                    CheckCategory::Embedding,
                    "backend",
                    format!("Local embedding configured (model: {})", local.name),
                ));
            } else {
                let (_, _, cloud_model, dimensions, _) = embed.cloud_fields();
                results.push(CheckResult::ok(
                    CheckCategory::Embedding,
                    "backend",
                    format!(
                        "Cloud embedding configured (model: {cloud_model}, dims: {dimensions})"
                    ),
                ));
            }
        }
        Err(e) => {
            results.push(CheckResult::warning(
                CheckCategory::Embedding,
                "backend",
                format!("Cannot resolve embedding backend: {e}"),
                "Check ai.tasks.embedding in settings.json",
            ));
        }
    }
}

fn check_store(results: &mut Vec<CheckResult>, diag: &EneDiagnostics, snapshot: &EneStateSnapshot) {
    let store_config = snapshot
        .config
        .get_section::<StoreConfig>()
        .unwrap_or_default();

    if !store_config.enabled {
        results.push(CheckResult::warning(
            CheckCategory::Store,
            "memory",
            "Memory store is disabled",
            "Set store.enabled = true in settings.json to enable long-term memory",
        ));
        return;
    }

    if diag.memory().is_enabled() {
        let db_path = store_config.resolve_memory_db_path(&snapshot.config.character);
        results.push(CheckResult::ok(
            CheckCategory::Store,
            "memory",
            format!("Memory store enabled (db: {})", mask_path(&db_path)),
        ));
    } else {
        results.push(CheckResult::error(
            CheckCategory::Store,
            "memory",
            "Memory store configured but not available at runtime",
            Some("Check store configuration and database file permissions"),
        ));
    }
}

async fn check_tool_registry(results: &mut Vec<CheckResult>, diag: &EneDiagnostics) {
    match diag.list_tools().await {
        Ok(tools) => {
            if tools.is_empty() {
                results.push(CheckResult::warning(
                    CheckCategory::ToolRegistry,
                    "tools",
                    "No tools registered",
                    "Check tools.list in settings.json and ensure tool binaries are built",
                ));
            } else {
                results.push(CheckResult::ok(
                    CheckCategory::ToolRegistry,
                    "tools",
                    format!("{} tool(s) registered", tools.len()),
                ));
            }
        }
        Err(e) => {
            results.push(CheckResult::error(
                CheckCategory::ToolRegistry,
                "tools",
                format!("Failed to list tools: {e}"),
                Some("Check tool host configuration"),
            ));
        }
    }
}

fn check_assets(results: &mut Vec<CheckResult>) {
    let assets = ene_config::paths::assets_dir();
    if assets.is_dir() {
        results.push(CheckResult::ok(
            CheckCategory::Assets,
            "directory",
            format!("Assets directory found ({})", mask_path(&assets)),
        ));
    } else {
        results.push(CheckResult::warning(
            CheckCategory::Assets,
            "directory",
            format!("Assets directory not found ({})", mask_path(&assets)),
            "Run from the repository root or check assets/ installation",
        ));
    }
}

fn print_human(checks: &[CheckResult]) {
    println!("{}", style::header("── Doctor ──"));
    for check in checks {
        let status_str = match check.status {
            CheckStatus::Ok => style::success("[OK]"),
            CheckStatus::Warning => style::warning("[WARN]"),
            CheckStatus::Error => style::error("[ERROR]"),
            CheckStatus::Skipped => style::header("[SKIP]"),
        };
        println!(
            "  {} [{}] {}: {}",
            status_str,
            check.category,
            style::header(check.name.as_str()),
            check.message
        );
        if let Some(remediation) = &check.remediation {
            println!("      → {remediation}");
        }
    }
    let summary = DoctorSummary::from_checks(checks);
    println!("{}", style::header("────────────"));
    let summary_line = format!(
        "{} ok, {} warning(s), {} error(s), {} skipped",
        summary.ok, summary.warnings, summary.errors, summary.skipped
    );
    if summary.errors > 0 {
        println!("  {}", style::error(summary_line));
    } else if summary.warnings > 0 {
        println!("  {}", style::warning(summary_line));
    } else {
        println!("  {}", style::success(summary_line));
    }
}
