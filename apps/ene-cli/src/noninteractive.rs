//! Non-interactive subcommand execution for CI / scripting use.
//!
//! Each subcommand runs a single operation against the runtime and returns a
//! process exit code. Structured output (JSON / JSONL) goes to stdout; logs
//! and progress stay on stderr. Side-effecting operations require an explicit
//! `--yes` opt-in — without it, permission gates are denied rather than
//! prompted.

use std::io::Read;

use ene_runtime::{
    CueSource, EneEvent, EneEventReceiver, EneHandle, PerfKind, PermissionDecision, RunError,
    TerminalReason, TurnId, UserInputResponse,
};
use serde::Serialize;

use crate::cli::{Command, MemoryAction, SessionAction, StoreAction, ToolAction};
use crate::context::AppContext;
use crate::output::{
    self, EXIT_OK, EXIT_RUNTIME, ErrorCode, OutputError, OutputFormat, PerfCue, StreamEvent,
};

pub async fn execute(cmd: &Command, ctx: &mut AppContext) -> i32 {
    let result = match cmd {
        Command::Run {
            prompt,
            jsonl,
            json,
            timeout,
            yes,
        } => {
            let format = if *jsonl {
                OutputFormat::Jsonl
            } else if *json {
                OutputFormat::Json
            } else {
                OutputFormat::Text
            };
            run_prompt(ctx, prompt, format, *timeout, *yes).await
        }
        Command::Tool { action, json } => tool_command(ctx, action, *json).await,
        Command::Session { action, json } => session_command(ctx, action, *json).await,
        Command::Memory { action, json } => memory_command(ctx, action, *json).await,
        Command::Doctor { json } => doctor_command(ctx, *json).await,
        Command::Store { action, json } => store_command(ctx, action, *json).await,
    };

    match result {
        Ok(code) => code,
        Err(e) => {
            let format = match cmd {
                Command::Run { jsonl, json, .. } => {
                    if *jsonl {
                        OutputFormat::Jsonl
                    } else if *json {
                        OutputFormat::Json
                    } else {
                        OutputFormat::Text
                    }
                }
                Command::Tool { json, .. }
                | Command::Session { json, .. }
                | Command::Memory { json, .. }
                | Command::Doctor { json }
                | Command::Store { json, .. } => {
                    if *json {
                        OutputFormat::Json
                    } else {
                        OutputFormat::Text
                    }
                }
            };
            emit_error(&e, format);
            e.exit_code()
        }
    }
}

fn emit_error(err: &OutputError, format: OutputFormat) {
    if format.is_structured() {
        println!("{}", err.to_json());
    }
    eprintln!("error: {err}");
}

// ── run ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct ToolCallRecord {
    name: String,
    result: String,
}

#[derive(Debug, Serialize)]
struct RunSummary {
    turn: String,
    text: String,
    reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    tool_calls: Vec<ToolCallRecord>,
}

async fn run_prompt(
    ctx: &mut AppContext,
    prompt_args: &[String],
    format: OutputFormat,
    timeout_secs: Option<u64>,
    yes: bool,
) -> Result<i32, OutputError> {
    let prompt = resolve_prompt(prompt_args)?;
    if prompt.trim().is_empty() {
        return Err(OutputError::new(
            ErrorCode::Usage,
            "no prompt provided (pass text or pipe it to stdin)",
        ));
    }

    // Subscribe before sending the run command to avoid missing events.
    let mut rx = ctx.handle.subscribe();
    let turn = ctx
        .handle
        .run(&prompt)
        .map_err(|e| run_error_to_output(&e))?;

    let fut = process_turn(&ctx.handle, &mut rx, &turn, format, yes);
    match timeout_secs {
        Some(secs) => {
            let duration = std::time::Duration::from_secs(secs);
            if let Ok(code) = tokio::time::timeout(duration, fut).await {
                code
            } else {
                drop(ctx.handle.cancel(&turn));
                if format.is_structured() {
                    output::print_jsonl(&StreamEvent::Terminal {
                        turn: turn.to_string(),
                        reason: "timeout".to_string(),
                        message: Some(format!("exceeded {secs}s timeout")),
                    })?;
                }
                eprintln!("error: turn timed out after {secs}s");
                Ok(ErrorCode::Timeout.exit_code())
            }
        }
        None => fut.await,
    }
}

fn resolve_prompt(prompt_args: &[String]) -> Result<String, OutputError> {
    if !prompt_args.is_empty() {
        return Ok(prompt_args.join(" "));
    }
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| OutputError::new(ErrorCode::Usage, format!("failed to read stdin: {e}")))?;
    Ok(buf)
}

fn run_error_to_output(err: &RunError) -> OutputError {
    match err {
        RunError::Busy => OutputError::new(ErrorCode::Busy, err.to_string()),
        RunError::ActorDead => OutputError::new(ErrorCode::Runtime, err.to_string()),
    }
}

/// Permission gates are auto-approved when `yes` is set and denied
/// otherwise; user-input gates are always cancelled because no interactive
/// prompt is available.
async fn process_turn(
    handle: &EneHandle,
    rx: &mut EneEventReceiver,
    turn: &TurnId,
    format: OutputFormat,
    yes: bool,
) -> Result<i32, OutputError> {
    let mut text = String::new();
    let mut tool_calls: Vec<ToolCallRecord> = Vec::new();
    let mut denied_permission = false;
    let mut exit_code = EXIT_OK;
    let mut terminal_message: Option<String> = None;
    // Defensive default; overwritten in every branch but guards future break paths.
    #[expect(unused_assignments, reason = "safety net for new loop exits")]
    let mut terminal_reason = "unknown".to_string();

    loop {
        let Ok(event) = rx.recv().await else {
            exit_code = EXIT_RUNTIME;
            terminal_reason = "failed".to_string();
            terminal_message = Some("event channel closed unexpectedly".to_string());
            break;
        };

        match event {
            EneEvent::TurnStarted { turn: t, .. } => {
                if t != *turn {
                    continue;
                }
                if format == OutputFormat::Jsonl {
                    output::print_jsonl(&StreamEvent::TurnStarted {
                        turn: t.to_string(),
                    })?;
                }
            }
            EneEvent::TextDelta { turn: t, delta, .. } => {
                if t != *turn {
                    continue;
                }
                text.push_str(&delta);
                match format {
                    OutputFormat::Jsonl => {
                        output::print_jsonl(&StreamEvent::TextDelta {
                            turn: t.to_string(),
                            delta,
                        })?;
                    }
                    OutputFormat::Text => print_text_delta(&delta),
                    OutputFormat::Json => {}
                }
            }
            EneEvent::Performance {
                turn: t,
                cues,
                source,
                ..
            } => {
                if t != *turn {
                    continue;
                }
                if format == OutputFormat::Jsonl {
                    let mapped: Vec<PerfCue> = cues
                        .iter()
                        .map(|cue| PerfCue {
                            name: cue.name.clone(),
                            kind: perf_kind_label(cue.kind).to_string(),
                            source: cue_source_label(source).to_string(),
                        })
                        .collect();
                    output::print_jsonl(&StreamEvent::Performance {
                        turn: t.to_string(),
                        cues: mapped,
                    })?;
                }
            }
            EneEvent::ToolCallStart {
                turn: t,
                name,
                arguments,
                ..
            } => {
                if t != *turn {
                    continue;
                }
                if format == OutputFormat::Jsonl {
                    let args = serde_json::from_str(&arguments)
                        .unwrap_or_else(|_| serde_json::Value::String(arguments.clone()));
                    output::print_jsonl(&StreamEvent::ToolCallStart {
                        turn: t.to_string(),
                        name,
                        arguments: args,
                    })?;
                }
            }
            EneEvent::ToolCallResult {
                turn: t,
                name,
                result,
                ..
            } => {
                if t != *turn {
                    continue;
                }
                tool_calls.push(ToolCallRecord {
                    name: name.clone(),
                    result: result.clone(),
                });
                if format == OutputFormat::Jsonl {
                    output::print_jsonl(&StreamEvent::ToolCallResult {
                        turn: t.to_string(),
                        name,
                        result,
                    })?;
                }
            }
            EneEvent::PermissionRequired {
                turn: t,
                request_id,
                action,
                target,
                ..
            } => {
                if t != *turn {
                    continue;
                }
                if yes {
                    drop(handle.decide_permission(request_id, PermissionDecision::AllowOnce));
                } else {
                    denied_permission = true;
                    if format == OutputFormat::Jsonl {
                        output::print_jsonl(&StreamEvent::PermissionDenied {
                            turn: t.to_string(),
                            action: action.clone(),
                            target: target.clone(),
                        })?;
                    }
                    drop(handle.decide_permission(request_id, PermissionDecision::Deny));
                }
            }
            EneEvent::UserInputRequired {
                turn: t,
                request_id,
                ..
            } => {
                if t != *turn {
                    continue;
                }
                // No interactive prompt is available in non-interactive mode.
                drop(handle.submit_user_input(request_id, UserInputResponse::Cancel));
            }
            EneEvent::Terminal {
                turn: t, reason, ..
            } => {
                if t != *turn {
                    continue;
                }
                match &reason {
                    TerminalReason::Done => {
                        terminal_reason = "done".to_string();
                    }
                    TerminalReason::Failed { message } => {
                        terminal_reason = "failed".to_string();
                        terminal_message = Some(message.clone());
                        exit_code = EXIT_RUNTIME;
                    }
                    TerminalReason::Cancelled => {
                        terminal_reason = "cancelled".to_string();
                    }
                }
                if format == OutputFormat::Jsonl {
                    output::print_jsonl(&StreamEvent::Terminal {
                        turn: t.to_string(),
                        reason: terminal_reason.clone(),
                        message: terminal_message.clone(),
                    })?;
                }
                break;
            }
            EneEvent::ContextCompressed { .. } => {}
        }
    }

    if format == OutputFormat::Text && !text.is_empty() && !text.ends_with('\n') {
        println!();
    }

    if format == OutputFormat::Text
        && let Some(message) = terminal_message.as_deref()
    {
        let user_message = crate::runtime_error::user_message_from_turn(message);
        eprintln!(
            "{}",
            crate::style::error(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "turn-failed",
                detail = user_message
            ))
        );
    }

    if format == OutputFormat::Json {
        output::print_json(&RunSummary {
            turn: turn.to_string(),
            text,
            reason: terminal_reason,
            message: terminal_message,
            tool_calls,
        })?;
    }

    // A denied permission (no --yes) overrides a clean exit so callers can
    // detect that explicit confirmation was required.
    if denied_permission && exit_code == EXIT_OK {
        return Ok(ErrorCode::ConfirmationRequired.exit_code());
    }

    Ok(exit_code)
}

fn print_text_delta(delta: &str) {
    use std::io::Write;
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    drop(lock.write_all(delta.as_bytes()));
    drop(lock.flush());
}

/// Map a [`PerfKind`] to its stable string label.
const fn perf_kind_label(kind: PerfKind) -> &'static str {
    match kind {
        PerfKind::Expression => "expression",
        PerfKind::Motion => "motion",
        PerfKind::LookAt => "lookat",
        PerfKind::Cancel => "cancel",
    }
}

/// Map a [`CueSource`] to its stable string label.
const fn cue_source_label(source: CueSource) -> &'static str {
    match source {
        CueSource::Affect => "affect",
        CueSource::Llm => "llm",
        CueSource::Hysteresis => "hysteresis",
        CueSource::Fallback => "fallback",
    }
}

// ── tool ────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct ToolCallOutput {
    name: String,
    result: String,
}

async fn tool_command(
    ctx: &mut AppContext,
    action: &ToolAction,
    json: bool,
) -> Result<i32, OutputError> {
    let tools = ctx.handle.tools();
    match action {
        ToolAction::List => {
            let tools = tools
                .list_tools()
                .await
                .map_err(|e| OutputError::new(ErrorCode::Runtime, format!("list tools: {e}")))?;
            if json {
                output::print_json(&tools)?;
            } else {
                print_tool_list(&tools);
            }
            Ok(EXIT_OK)
        }
        ToolAction::Search { query } => {
            let tools = tools
                .search_tools(query.clone())
                .await
                .map_err(|e| OutputError::new(ErrorCode::Runtime, format!("search tools: {e}")))?;
            if json {
                output::print_json(&tools)?;
            } else {
                print_tool_list(&tools);
            }
            Ok(EXIT_OK)
        }
        ToolAction::Help { name } => {
            let tools = tools
                .list_tools()
                .await
                .map_err(|e| OutputError::new(ErrorCode::Runtime, format!("list tools: {e}")))?;
            let Some(tool) = tools.iter().find(|t| t.name.as_str() == name) else {
                return Err(OutputError::new(
                    ErrorCode::Usage,
                    format!("tool not found: {name}"),
                ));
            };
            if json {
                output::print_json(&tool)?;
            } else {
                println!("Tool: {}", tool.name.as_str());
                println!("Description: {}", tool.description);
                println!(
                    "Parameters: {}",
                    serde_json::to_string_pretty(&tool.parameters).unwrap_or_default()
                );
            }
            Ok(EXIT_OK)
        }
        ToolAction::Call { name, arguments } => {
            let result = tools
                .call_tool(name.clone(), arguments.clone())
                .await
                .map_err(|e| {
                    OutputError::new(ErrorCode::ToolFailed, format!("tool call failed: {e}"))
                })?;
            if json {
                output::print_json(&ToolCallOutput {
                    name: name.clone(),
                    result,
                })?;
            } else {
                println!("{result}");
            }
            Ok(EXIT_OK)
        }
    }
}

fn print_tool_list(tools: &[ene_runtime::ToolSpec]) {
    if tools.is_empty() {
        println!("No tools registered.");
        return;
    }
    for tool in tools {
        println!("- {}: {}", tool.name.as_str(), tool.description);
    }
}

// ── session ─────────────────────────────────────────────────────────────────

async fn session_command(
    ctx: &mut AppContext,
    action: &SessionAction,
    json: bool,
) -> Result<i32, OutputError> {
    match action {
        SessionAction::List { archived } => {
            let sessions = ctx
                .handle
                .sessions()
                .list(*archived, 50)
                .await
                .map_err(|e| OutputError::new(ErrorCode::Runtime, format!("list sessions: {e}")))?;
            if json {
                output::print_json(&sessions)?;
            } else {
                print_session_list(&sessions);
            }
            Ok(EXIT_OK)
        }
        SessionAction::Export { id } => {
            let bundle = ctx.handle.sessions().export(id).await.map_err(|e| {
                OutputError::new(ErrorCode::Runtime, format!("export session: {e}"))
            })?;
            // The export bundle is already JSON; print it verbatim.
            println!("{bundle}");
            Ok(EXIT_OK)
        }
        SessionAction::Import { path } => {
            let json_text = tokio::fs::read_to_string(path).await.map_err(|e| {
                OutputError::new(ErrorCode::Usage, format!("read {}: {e}", path.display()))
            })?;
            let id = ctx
                .handle
                .sessions()
                .import(&json_text)
                .await
                .map_err(|e| {
                    OutputError::new(ErrorCode::Runtime, format!("import session: {e}"))
                })?;
            if json {
                output::print_json(&serde_json::json!({ "imported_id": id }))?;
            } else {
                println!("Imported session (row id {id}).");
            }
            Ok(EXIT_OK)
        }
        SessionAction::Search { query } => {
            let matches = ctx
                .handle
                .sessions()
                .search(query, 20, 0)
                .await
                .map_err(|e| {
                    OutputError::new(ErrorCode::Runtime, format!("search sessions: {e}"))
                })?;
            if json {
                let rows: Vec<serde_json::Value> = matches
                    .iter()
                    .map(|(session_id, msg)| {
                        serde_json::json!({
                            "session_id": session_id.as_str(),
                            "role": msg.role,
                            "content": msg.content,
                        })
                    })
                    .collect();
                output::print_json(&rows)?;
            } else {
                print_session_search(&matches);
            }
            Ok(EXIT_OK)
        }
        SessionAction::Archive { id } => set_archived(ctx, id, true, json).await,
        SessionAction::Unarchive { id } => set_archived(ctx, id, false, json).await,
    }
}

async fn set_archived(
    ctx: &mut AppContext,
    id: &str,
    archived: bool,
    json: bool,
) -> Result<i32, OutputError> {
    let updated = ctx
        .handle
        .sessions()
        .set_archived(id, archived)
        .await
        .map_err(|e| OutputError::new(ErrorCode::Runtime, format!("update session: {e}")))?;
    if json {
        output::print_json(&serde_json::json!({ "id": id, "updated": updated }))?;
    } else {
        let verb = if archived { "Archived" } else { "Unarchived" };
        println!("{verb} session {id} (updated={updated}).");
    }
    Ok(EXIT_OK)
}

fn print_session_list(sessions: &[ene_runtime::PublicSessionMeta]) {
    if sessions.is_empty() {
        println!("No stored sessions.");
        return;
    }
    for s in sessions {
        let title = if s.title.is_empty() {
            "(untitled)"
        } else {
            s.title.as_str()
        };
        println!(
            "- {} | {} | {} turns | updated {}",
            s.session_id.as_str(),
            title,
            s.turn_count,
            s.updated_at.format("%Y-%m-%d %H:%M:%S")
        );
    }
}

fn print_session_search(matches: &[(String, ene_runtime::PublicExportedMessage)]) {
    if matches.is_empty() {
        println!("No matching messages.");
        return;
    }
    for (session_id, msg) in matches {
        println!("[{session_id}] {}: {}", msg.role, msg.content);
    }
}

// ── memory ──────────────────────────────────────────────────────────────────

async fn memory_command(
    ctx: &mut AppContext,
    action: &MemoryAction,
    json: bool,
) -> Result<i32, OutputError> {
    let diag = ctx.handle.diagnostics();
    let snapshot = diag
        .get_snapshot()
        .await
        .map_err(|e| OutputError::new(ErrorCode::Runtime, format!("actor state: {e}")))?;
    let memory = diag.memory();

    match action {
        MemoryAction::List { kind } => {
            require_memory(memory)?;
            let kind = parse_kind(kind.as_deref())?;
            let memories = memory
                .list_typed_memories(snapshot.card_name.as_str(), kind, 50)
                .await
                .map_err(|e| OutputError::new(ErrorCode::Runtime, format!("list memories: {e}")))?;
            if json {
                output::print_json(&memories)?;
            } else {
                for m in &memories {
                    println!(
                        "id={} [{}|{}] {}",
                        m.id.unwrap_or_default(),
                        m.kind.as_str(),
                        m.status.as_str(),
                        m.title
                    );
                }
            }
            Ok(EXIT_OK)
        }
        MemoryAction::Inspect { id } => match memory.inspect_typed_memory(*id).await {
            Ok(Some(m)) => {
                if json {
                    output::print_json(&m)?;
                } else {
                    println!("id={}", m.id.unwrap_or_default());
                    println!("kind={}", m.kind.as_str());
                    println!("status={}", m.status.as_str());
                    println!("title={}", m.title);
                    println!("content={}", m.content);
                }
                Ok(EXIT_OK)
            }
            Ok(None) => Err(OutputError::new(
                ErrorCode::Usage,
                format!("memory id={id} not found"),
            )),
            Err(e) => Err(OutputError::new(
                ErrorCode::Runtime,
                format!("inspect memory: {e}"),
            )),
        },
        MemoryAction::Search { query } => {
            require_memory(memory)?;
            let results = memory
                .search_typed_memories_hybrid(
                    snapshot.card_name.as_str(),
                    Some(&snapshot.config.user_name),
                    query,
                    10,
                )
                .await
                .map_err(|e| {
                    OutputError::new(ErrorCode::Runtime, format!("search memories: {e}"))
                })?;
            if json {
                output::print_json(&results)?;
            } else {
                for scored in &results {
                    println!(
                        "id={} kind={} total={:.3} title={}",
                        scored.item.id.unwrap_or_default(),
                        scored.item.kind.as_str(),
                        scored.breakdown.total,
                        scored.item.title
                    );
                }
            }
            Ok(EXIT_OK)
        }
    }
}

fn require_memory(memory: &ene_runtime::MemoryHandle) -> Result<(), OutputError> {
    if memory.is_enabled() {
        Ok(())
    } else {
        Err(OutputError::new(
            ErrorCode::Runtime,
            "memory is not enabled",
        ))
    }
}

fn parse_kind(kind: Option<&str>) -> Result<Option<ene_store::MemoryKind>, OutputError> {
    let Some(kind) = kind else {
        return Ok(None);
    };
    crate::util::parse_memory_kind(kind)
        .ok_or_else(|| OutputError::new(ErrorCode::Usage, format!("unknown memory kind: {kind}")))
        .map(Some)
}

// ── doctor ──────────────────────────────────────────────────────────────────

async fn doctor_command(ctx: &mut AppContext, json: bool) -> Result<i32, OutputError> {
    // Reuse the interactive /doctor implementation, which already supports a
    // `--json` report. We locate it through the shared command registry to
    // avoid duplicating the check logic (the module itself is private).
    use crate::commands::CliError;
    let Some(doctor) = crate::commands::COMMANDS
        .iter()
        .find(|c| c.name() == "/doctor")
    else {
        return Err(OutputError::new(
            ErrorCode::Runtime,
            "doctor command unavailable",
        ));
    };
    let arg = if json { "--json" } else { "" };
    match doctor.execute(arg, ctx).await {
        Ok(_) => Ok(EXIT_OK),
        Err(CliError::ExecutionFailed(msg)) => Err(OutputError::new(ErrorCode::Runtime, msg)),
        Err(e) => Err(OutputError::new(ErrorCode::Runtime, e.to_string())),
    }
}

// ── store ───────────────────────────────────────────────────────────────────

async fn store_command(
    ctx: &mut AppContext,
    action: &StoreAction,
    json: bool,
) -> Result<i32, OutputError> {
    let memory = ctx.handle.diagnostics().memory();

    match action {
        StoreAction::Backup => {
            let path = memory
                .backup()
                .await
                .map_err(|e| OutputError::new(ErrorCode::Runtime, e.to_string()))?;
            if json {
                output::print_json(&serde_json::json!({ "backup": path.display().to_string() }))?;
            } else {
                println!("Backup created: {}", path.display());
            }
            Ok(EXIT_OK)
        }
        StoreAction::ListBackups => {
            if !memory.is_enabled() {
                return Err(OutputError::new(
                    ErrorCode::Runtime,
                    "memory store is not enabled",
                ));
            }
            let db_path = memory.path().ok_or_else(|| {
                OutputError::new(ErrorCode::Runtime, "in-memory store has no file backups")
            })?;
            let backups = ene_store::list_backups(db_path)
                .map_err(|e| OutputError::new(ErrorCode::Runtime, e.to_string()))?;
            if json {
                let rows: Vec<String> = backups.iter().map(|p| p.display().to_string()).collect();
                output::print_json(&rows)?;
            } else if backups.is_empty() {
                println!("No backups found.");
            } else {
                for b in backups {
                    println!("{}", b.display());
                }
            }
            Ok(EXIT_OK)
        }
        StoreAction::Restore { path, yes } => {
            if !memory.is_enabled() {
                return Err(OutputError::new(
                    ErrorCode::Runtime,
                    "memory store is not enabled",
                ));
            }
            if !yes {
                return Err(OutputError::new(
                    ErrorCode::ConfirmationRequired,
                    "restore requires --yes (destroys the current database file)",
                ));
            }
            let db_path = memory
                .path()
                .map(std::path::Path::to_path_buf)
                .ok_or_else(|| {
                    OutputError::new(
                        ErrorCode::Runtime,
                        "in-memory store cannot be restored from a file",
                    )
                })?;
            ctx.handle
                .shutdown(crate::commands::SHUTDOWN_TIMEOUT)
                .await
                .map_err(|e| OutputError::new(ErrorCode::Runtime, e.to_string()))?;
            ene_store::restore_database(path, &db_path)
                .map_err(|e| OutputError::new(ErrorCode::Runtime, e.to_string()))?;
            if json {
                output::print_json(&serde_json::json!({
                    "restored": db_path.display().to_string(),
                    "from": path.display().to_string(),
                }))?;
            } else {
                println!(
                    "Restored {} from {}. Restart ene to reopen.",
                    db_path.display(),
                    path.display()
                );
            }
            Ok(EXIT_OK)
        }
        StoreAction::Integrity => {
            memory
                .check_integrity()
                .await
                .map_err(|e| OutputError::new(ErrorCode::Runtime, e.to_string()))?;
            if json {
                output::print_json(&serde_json::json!({ "integrity": "ok" }))?;
            } else {
                println!("integrity_check: ok");
            }
            Ok(EXIT_OK)
        }
    }
}
