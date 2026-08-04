use crate::commands::{CliCommand, CliError, CommandOutcome};
use crate::context::AppContext;
use crate::style;
use async_trait::async_trait;
use ene_runtime::{NewSchedule, Schedule, ScheduleAction, ScheduleConfirmation, ScheduleKind};

pub struct ScheduleCommand;

#[async_trait]
impl CliCommand for ScheduleCommand {
    fn name(&self) -> &'static str {
        "/schedule"
    }

    fn description(&self) -> &'static str {
        "Manage persistent schedules (one-shot, interval, cron, startup)"
    }

    fn usage(&self) -> &'static str {
        "/schedule <list|history <name>|delete <name>|pause <name>|resume <name>|add ...>"
    }

    async fn execute(&self, arg: &str, ctx: &mut AppContext) -> Result<CommandOutcome, CliError> {
        let mut parts = arg.split_whitespace();
        match parts.next() {
            None | Some("list") => list(ctx).await,
            Some("history") => {
                let name = parts.next().ok_or_else(|| CliError::UsageError {
                    usage: "Usage: /schedule history <name>".to_string(),
                })?;
                history(ctx, name).await
            }
            Some("delete") => {
                let name = parts.next().ok_or_else(|| CliError::UsageError {
                    usage: "Usage: /schedule delete <name>".to_string(),
                })?;
                toggle(ctx, name, false, true).await
            }
            Some("pause") => {
                let name = parts.next().ok_or_else(|| CliError::UsageError {
                    usage: "Usage: /schedule pause <name>".to_string(),
                })?;
                toggle(ctx, name, false, false).await
            }
            Some("resume") => {
                let name = parts.next().ok_or_else(|| CliError::UsageError {
                    usage: "Usage: /schedule resume <name>".to_string(),
                })?;
                toggle(ctx, name, true, false).await
            }
            Some("add") => {
                let rest = arg
                    .split_once(char::is_whitespace)
                    .map_or("", |(_, rest)| rest);
                add(ctx, rest).await
            }
            Some(other) => Err(CliError::UsageError {
                usage: format!("Unknown /schedule subcommand: {other}"),
            }),
        }
    }
}

/// Pulls the next token off `arg`: whitespace-delimited, with single- or
/// double-quoted segments kept intact (quotes stripped). Returns `None` at
/// end of input.
fn next_token(arg: &mut &str) -> Option<String> {
    *arg = arg.trim_start();
    if arg.is_empty() {
        return None;
    }
    if let Some(quote) = arg.chars().next().filter(|c| *c == '"' || *c == '\'') {
        let rest = &arg[1..];
        let end = rest.find(quote)?;
        let token = rest[..end].to_string();
        *arg = &rest[end + 1..];
        Some(token)
    } else {
        let end = arg.find(char::is_whitespace).unwrap_or(arg.len());
        let token = arg[..end].to_string();
        *arg = &arg[end..];
        Some(token)
    }
}

/// Collects the value for `--cron` / `--prompt` / `--args`: every token up
/// to the next `--` flag, joined with single spaces, so unquoted multi-word
/// values work too. Quote handling comes from [`next_token`].
fn collect_value(arg: &mut &str) -> String {
    let mut parts = Vec::new();
    loop {
        let trimmed = arg.trim_start();
        if trimmed.is_empty() || trimmed.starts_with("--") {
            break;
        }
        let Some(token) = next_token(arg) else {
            break;
        };
        parts.push(token);
    }
    parts.join(" ")
}

async fn list(ctx: &mut AppContext) -> Result<CommandOutcome, CliError> {
    let schedules = ctx
        .handle
        .list_schedules()
        .await
        .map_err(|e| CliError::ActorError(e.to_string()))?;
    if schedules.is_empty() {
        println!(
            "No schedules. Add one with /schedule add <name> --kind <one_shot|interval|cron|startup> ..."
        );
    } else {
        println!("{}", style::success("Schedules:"));
        for s in schedules {
            let next = s.next_run_at.map_or_else(
                || "—".to_string(),
                |t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
            );
            let last = s
                .last_status
                .map_or_else(|| "never".to_string(), |st| st.as_str().to_string());
            println!(
                "  [{}] {} ({}, enabled={}, next: {}, last: {})",
                style::header(s.id.to_string()),
                s.name,
                s.kind.as_str(),
                s.enabled,
                next,
                last
            );
        }
    }
    Ok(CommandOutcome::Continue)
}

async fn history(ctx: &mut AppContext, name: &str) -> Result<CommandOutcome, CliError> {
    let schedule = find_by_name(ctx, name).await?;
    let runs = ctx
        .handle
        .list_schedule_runs(schedule.id, 20)
        .await
        .map_err(|e| CliError::ActorError(e.to_string()))?;
    if runs.is_empty() {
        println!("No runs recorded for '{}'.", schedule.name);
    } else {
        println!(
            "{}",
            style::success(format!("Run history for '{}':", schedule.name))
        );
        for r in runs {
            let error = r
                .error
                .as_deref()
                .map_or_else(String::new, |e| format!(" ({e})"));
            println!(
                "  #{} {} at {}{} (retries={})",
                r.id,
                r.status.as_str(),
                r.scheduled_at.format("%Y-%m-%d %H:%M:%S UTC"),
                error,
                r.retries
            );
        }
    }
    Ok(CommandOutcome::Continue)
}

async fn toggle(
    ctx: &mut AppContext,
    name: &str,
    enabled: bool,
    delete: bool,
) -> Result<CommandOutcome, CliError> {
    let schedule = find_by_name(ctx, name).await?;
    if delete {
        let removed = ctx
            .handle
            .delete_schedule(schedule.id)
            .await
            .map_err(|e| CliError::ActorError(e.to_string()))?;
        if removed {
            println!(
                "{}",
                style::success(format!("Deleted schedule '{}'.", schedule.name))
            );
        }
    } else {
        ctx.handle
            .set_schedule_enabled(schedule.id, enabled)
            .await
            .map_err(|e| CliError::ActorError(e.to_string()))?;
        let verb = if enabled { "Resumed" } else { "Paused" };
        println!(
            "{}",
            style::success(format!("{verb} schedule '{}'.", schedule.name))
        );
    }
    Ok(CommandOutcome::Continue)
}

async fn find_by_name(ctx: &mut AppContext, name: &str) -> Result<Schedule, CliError> {
    let schedules = ctx
        .handle
        .list_schedules()
        .await
        .map_err(|e| CliError::ActorError(e.to_string()))?;
    schedules
        .into_iter()
        .find(|s| s.name == name)
        .ok_or_else(|| CliError::ExecutionFailed(format!("No schedule named '{name}'")))
}

async fn add(ctx: &mut AppContext, arg: &str) -> Result<CommandOutcome, CliError> {
    let mut rest = arg;
    let name = next_token(&mut rest).ok_or_else(|| CliError::UsageError {
        usage: "Usage: /schedule add <name> --kind <one_shot|interval|cron|startup> \
                [--at <RFC3339>] [--every <secs>] [--cron <expr>] [--tz <IANA>] \
                [--tool <name> --args <json> | --prompt <text>] [--allow-tools] \
                [--confirm] [--retries <n>] [--retry-delay <secs>]"
            .to_string(),
    })?;
    let new = parse_add(name, rest)?;
    let schedule = ctx
        .handle
        .add_schedule(new)
        .await
        .map_err(|e| CliError::ActorError(e.to_string()))?;
    let next = schedule.next_run_at.map_or_else(
        || "as soon as the actor is idle".to_string(),
        |t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
    );
    println!(
        "{}",
        style::success(format!(
            "Schedule '{}' created (id {}, next run: {}).",
            schedule.name, schedule.id, next
        ))
    );
    Ok(CommandOutcome::Continue)
}

fn parse_add(name: String, mut arg: &str) -> Result<NewSchedule, CliError> {
    let mut kind: Option<ScheduleKind> = None;
    let mut start_at = None;
    let mut interval_secs = None;
    let mut cron_expr = None;
    let mut timezone = "UTC".to_string();
    let mut tool: Option<String> = None;
    let mut tool_args = serde_json::Value::Null;
    let mut prompt: Option<String> = None;
    let mut allow_tools = false;
    let mut confirmation = ScheduleConfirmation::None;
    let mut max_retries = 0;
    let mut retry_delay_secs = 60;

    let value = |flag: &str, arg: &mut &str| -> Result<String, CliError> {
        next_token(arg).ok_or_else(|| CliError::UsageError {
            usage: format!("Missing value for {flag}"),
        })
    };
    while let Some(flag) = next_token(&mut arg) {
        match flag.as_str() {
            "--kind" => {
                kind = Some(match value("--kind", &mut arg)?.as_str() {
                    "one_shot" => ScheduleKind::OneShot,
                    "interval" => ScheduleKind::Interval,
                    "cron" => ScheduleKind::Cron,
                    "startup" => ScheduleKind::Startup,
                    other => {
                        return Err(CliError::UsageError {
                            usage: format!("Unknown schedule kind: {other}"),
                        });
                    }
                });
            }
            "--at" => {
                let raw = value("--at", &mut arg)?;
                start_at = Some(
                    chrono::DateTime::parse_from_rfc3339(&raw)
                        .map_err(|e| CliError::UsageError {
                            usage: format!("Invalid --at timestamp '{raw}': {e}"),
                        })?
                        .with_timezone(&chrono::Utc),
                );
            }
            "--every" => {
                let raw = value("--every", &mut arg)?;
                interval_secs = Some(raw.parse::<i64>().map_err(|_| CliError::UsageError {
                    usage: format!("Invalid --every seconds: {raw}"),
                })?);
            }
            "--cron" => cron_expr = Some(collect_value(&mut arg)),
            "--tz" => timezone = value("--tz", &mut arg)?,
            "--tool" => tool = Some(value("--tool", &mut arg)?),
            "--args" => {
                let raw = collect_value(&mut arg);
                if raw.is_empty() {
                    return Err(CliError::UsageError {
                        usage: "Missing value for --args".to_string(),
                    });
                }
                tool_args = serde_json::from_str(&raw).map_err(|e| CliError::UsageError {
                    usage: format!("Invalid --args JSON '{raw}': {e}"),
                })?;
            }
            "--prompt" => {
                let text = collect_value(&mut arg);
                if text.is_empty() {
                    return Err(CliError::UsageError {
                        usage: "Missing value for --prompt".to_string(),
                    });
                }
                prompt = Some(text);
            }
            "--allow-tools" => allow_tools = true,
            "--confirm" => confirmation = ScheduleConfirmation::Confirm,
            "--retries" => {
                let raw = value("--retries", &mut arg)?;
                max_retries = raw.parse::<i64>().map_err(|_| CliError::UsageError {
                    usage: format!("Invalid --retries value: {raw}"),
                })?;
            }
            "--retry-delay" => {
                let raw = value("--retry-delay", &mut arg)?;
                retry_delay_secs = raw.parse::<i64>().map_err(|_| CliError::UsageError {
                    usage: format!("Invalid --retry-delay value: {raw}"),
                })?;
            }
            other => {
                return Err(CliError::UsageError {
                    usage: format!("Unknown flag: {other}"),
                });
            }
        }
    }

    let action = match (tool, prompt) {
        (Some(tool_name), None) => ScheduleAction::Tool {
            name: tool_name,
            arguments: if tool_args.is_null() {
                serde_json::json!({})
            } else {
                tool_args
            },
        },
        (None, Some(text)) => ScheduleAction::Prompt { text, allow_tools },
        _ => {
            return Err(CliError::UsageError {
                usage: "Exactly one of --tool <name> or --prompt <text> is required".to_string(),
            });
        }
    };
    let kind = kind.ok_or_else(|| CliError::UsageError {
        usage: "--kind <one_shot|interval|cron|startup> is required".to_string(),
    })?;

    Ok(NewSchedule {
        name,
        kind,
        timezone,
        cron_expr,
        interval_secs,
        start_at,
        action,
        confirmation,
        max_retries,
        retry_delay_secs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arg: &str) -> NewSchedule {
        parse_add("test".to_string(), arg).expect("parse succeeds")
    }

    #[test]
    fn cron_expression_with_spaces_parses_quoted_and_unquoted() {
        let quoted = parse("--kind cron --cron \"0 9 * * *\" --tz Asia/Tokyo --prompt ping");
        assert_eq!(quoted.kind, ScheduleKind::Cron);
        assert_eq!(quoted.cron_expr.as_deref(), Some("0 9 * * *"));
        assert_eq!(quoted.timezone, "Asia/Tokyo");

        let unquoted = parse("--kind cron --cron 0 9 * * * --tz UTC --prompt ping");
        assert_eq!(unquoted.cron_expr.as_deref(), Some("0 9 * * *"));
    }

    #[test]
    fn prompt_with_spaces_parses_quoted_and_unquoted() {
        let quoted = parse("--kind startup --prompt \"Remind the user to drink water\"");
        let quoted_ok = match quoted.action {
            ScheduleAction::Prompt { text, allow_tools } => {
                assert_eq!(text, "Remind the user to drink water");
                assert!(!allow_tools);
                true
            }
            ScheduleAction::Tool { .. } => false,
        };
        assert!(quoted_ok, "expected a prompt action");

        let unquoted = parse("--kind startup --prompt Remind the user --allow-tools");
        let unquoted_ok = match unquoted.action {
            ScheduleAction::Prompt { text, allow_tools } => {
                assert_eq!(text, "Remind the user");
                assert!(allow_tools);
                true
            }
            ScheduleAction::Tool { .. } => false,
        };
        assert!(unquoted_ok, "expected a prompt action");
    }

    #[test]
    fn json_args_with_spaces_parse_and_default_to_empty_object() {
        let with_args = parse(
            "--kind one_shot --at 2026-08-05T15:00:00+09:00 \
             --tool fs.write --args '{\"content\": \"hello world\"}'",
        );
        let with_args_ok = match with_args.action {
            ScheduleAction::Tool { name, arguments } => {
                assert_eq!(name, "fs.write");
                assert_eq!(arguments["content"], "hello world");
                true
            }
            ScheduleAction::Prompt { .. } => false,
        };
        assert!(with_args_ok, "expected a tool action");

        let defaulted = parse("--kind startup --tool fs.write");
        let defaulted_ok = match defaulted.action {
            ScheduleAction::Tool { arguments, .. } => {
                assert_eq!(arguments, serde_json::json!({}));
                true
            }
            ScheduleAction::Prompt { .. } => false,
        };
        assert!(defaulted_ok, "expected a tool action");
    }

    #[test]
    fn single_token_flags_still_work() {
        let new = parse(
            "--kind interval --every 3600 --retries 2 --retry-delay 30 \
             --confirm --prompt ping --allow-tools",
        );
        assert_eq!(new.kind, ScheduleKind::Interval);
        assert_eq!(new.interval_secs, Some(3600));
        assert_eq!(new.max_retries, 2);
        assert_eq!(new.retry_delay_secs, 30);
        assert_eq!(new.confirmation, ScheduleConfirmation::Confirm);
        let action_ok = match new.action {
            ScheduleAction::Prompt { allow_tools, .. } => {
                assert!(allow_tools);
                true
            }
            ScheduleAction::Tool { .. } => false,
        };
        assert!(action_ok, "expected a prompt action");
    }

    #[test]
    fn unknown_flag_is_rejected() {
        assert!(parse_add("test".to_string(), "--kind cron --bogus x").is_err());
    }
}
