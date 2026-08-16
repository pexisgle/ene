use crate::commands::{CliCommand, CliError, CommandOutcome};
use crate::context::AppContext;
use crate::style;
use async_trait::async_trait;
use ene_connector::{AccountCredentials, ConnectionState, CredentialStore};
use ene_runtime::{EneEvent, PermissionDecision};

pub struct ConnectorCommand;

/// Environment variable a CLI connect reads its API key from.
///
/// The connector id is sanitized (`[A-Za-z0-9._-]` → `_`) so the variable
/// name stays shell-exportable. The value is handled inside the protected
/// store boundary and is never echoed.
fn api_key_env_var(id: &str) -> String {
    let sanitized: String = id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("ENE_CONNECTOR_{sanitized}_API_KEY")
}

/// Connector commands run while the REPL select loop is not polling the
/// event bus, so they must answer their own prompts; this mirrors the
/// numbered prompt `stream.rs` renders during turns.
fn answer_prompt(ctx: &AppContext, request_id: &ene_runtime::RequestId) {
    let choices = vec![
        i18n_embed_fl::fl!(crate::i18n::loader(), "permission-allow-once"),
        i18n_embed_fl::fl!(crate::i18n::loader(), "permission-allow-session"),
        i18n_embed_fl::fl!(crate::i18n::loader(), "permission-deny"),
    ];
    crate::terminal_ui::TerminalUi::global().pause_for_external_prompt();
    let selection = dialoguer::Select::new()
        .with_prompt(i18n_embed_fl::fl!(
            crate::i18n::loader(),
            "permission-prompt"
        ))
        .items(&choices)
        .default(0)
        .interact()
        .unwrap_or(2);
    crate::terminal_ui::TerminalUi::global().resume_after_external_prompt();
    let decision = match selection {
        0 => PermissionDecision::AllowOnce,
        1 => PermissionDecision::AllowSession,
        _ => PermissionDecision::Deny,
    };
    drop(ctx.handle.decide_permission(request_id.clone(), decision));
}

/// The bus subscription is created *before* the operation future is polled
/// so a prompt emitted immediately after the command reaches the actor can
/// never be missed (broadcast receivers only see events sent after
/// subscription).
async fn await_with_prompts<T, F>(ctx: &AppContext, fut: F) -> Result<T, CliError>
where
    F: std::future::Future<Output = Result<T, ene_runtime::ConnectorHandleError>>,
{
    tokio::pin!(fut);
    let mut bus = ctx.handle.subscribe();
    loop {
        tokio::select! {
            result = &mut fut => return result.map_err(|e| CliError::ActorError(e.to_string())),
            event = bus.recv() => {
                if let Ok(EneEvent::PermissionRequired { request_id, .. }) = event {
                    answer_prompt(ctx, &request_id);
                }
            }
        }
    }
}

fn connection_label(connection: &ConnectionState) -> &'static str {
    match connection {
        ConnectionState::Disconnected => "disconnected",
        ConnectionState::Connected { .. } => "connected",
        ConnectionState::Error { .. } => "error",
    }
}

fn parse_connector_id(id: &str) -> Result<ene_connector::ConnectorId, CliError> {
    ene_connector::ConnectorId::try_new(id).map_err(|e| CliError::UsageError {
        usage: format!("Invalid connector id '{id}': {e}"),
    })
}

fn actor_error<E: std::fmt::Display>(error: E) -> CliError {
    CliError::ActorError(error.to_string())
}

#[async_trait]
impl CliCommand for ConnectorCommand {
    fn name(&self) -> &'static str {
        "/connector"
    }

    fn description(&self) -> &'static str {
        "List and manage external-service connectors"
    }

    fn usage(&self) -> &'static str {
        "/connector <list|status <id>|check <id>|connect <id>|disconnect <id> [account]|grant <id> <action> <target>|revoke <id> <action> <target>|permissions <id>>"
    }

    async fn execute(&self, arg: &str, ctx: &mut AppContext) -> Result<CommandOutcome, CliError> {
        let subparts: Vec<&str> = arg.splitn(4, ' ').collect();
        let connectors = ctx.handle.connectors();
        match subparts.first().copied() {
            None | Some("list") => {
                let summaries = connectors.list();
                if summaries.is_empty() {
                    println!("No connectors registered.");
                } else {
                    println!("{}", style::success("Connectors:"));
                    for summary in summaries {
                        println!(
                            "  - {}: {} ({}, {} account(s), {} action(s))",
                            style::header(summary.identity.id.as_str()),
                            summary.identity.display_name,
                            connection_label(&summary.connection),
                            summary.account_count,
                            summary.action_count
                        );
                    }
                }
                Ok(CommandOutcome::Continue)
            }
            Some("status") => {
                let id = subparts.get(1).copied().map_or("", str::trim);
                if id.is_empty() {
                    return Err(CliError::UsageError {
                        usage: "Usage: /connector status <id>".to_string(),
                    });
                }
                let status = connectors
                    .status(&parse_connector_id(id)?)
                    .map_err(actor_error)?;
                println!(
                    "{} ({}): {}",
                    style::header(status.identity.id.as_str()),
                    status.identity.display_name,
                    connection_label(&status.connection)
                );
                if let Some(health) = &status.health {
                    println!(
                        "  Health: {} ({})",
                        if health.healthy { "ok" } else { "unhealthy" },
                        health.message.as_deref().unwrap_or("no detail")
                    );
                } else {
                    println!("  Health: not checked yet");
                }
                if status.accounts.is_empty() {
                    println!("  Accounts: none");
                } else {
                    println!("  Accounts:");
                    for account in &status.accounts {
                        println!("    - {} ({:?})", account.label, account.auth);
                    }
                }
                Ok(CommandOutcome::Continue)
            }
            Some("check") => {
                let id = subparts.get(1).copied().map_or("", str::trim);
                if id.is_empty() {
                    return Err(CliError::UsageError {
                        usage: "Usage: /connector check <id>".to_string(),
                    });
                }
                let parsed = parse_connector_id(id)?;
                let health = await_with_prompts(ctx, connectors.check(&parsed)).await?;
                println!(
                    "{}: {}",
                    style::header(id),
                    if health.healthy {
                        style::success("reachable")
                    } else {
                        style::error("unreachable")
                    }
                );
                if let Some(message) = &health.message {
                    println!("  {message}");
                }
                Ok(CommandOutcome::Continue)
            }
            Some("connect") => {
                let id = subparts.get(1).copied().map_or("", str::trim);
                if id.is_empty() {
                    return Err(CliError::UsageError {
                        usage: "Usage: /connector connect <id>".to_string(),
                    });
                }
                let env_var = api_key_env_var(id);
                let key = std::env::var(&env_var).map_err(|_| CliError::ExecutionFailed(
                    format!("No API key found; set the {env_var} environment variable (the value is never echoed or logged)."),
                ))?;
                let parsed = parse_connector_id(id)?;
                let credential =
                    AccountCredentials::new("default", CredentialStore::from_api_key(key));
                let accounts =
                    await_with_prompts(ctx, ctx.handle.connectors().connect(&parsed, credential))
                        .await?;
                println!(
                    "{} Connected {}; {} account(s).",
                    style::success("OK"),
                    id,
                    accounts.len()
                );
                for account in &accounts {
                    println!("  - {}", account.label);
                }
                Ok(CommandOutcome::Continue)
            }
            Some("disconnect") => {
                let id = subparts.get(1).copied().map_or("", str::trim);
                if id.is_empty() {
                    return Err(CliError::UsageError {
                        usage: "Usage: /connector disconnect <id> [account]".to_string(),
                    });
                }
                let parsed = parse_connector_id(id)?;
                let account = subparts.get(2).copied().map_or("default", str::trim);
                await_with_prompts(ctx, ctx.handle.connectors().disconnect(&parsed, account))
                    .await?;
                println!("{} Disconnected {account} from {id}.", style::success("OK"));
                Ok(CommandOutcome::Continue)
            }
            Some("grant") => {
                let (id, action, target) = (subparts.get(1), subparts.get(2), subparts.get(3));
                let (Some(id), Some(action), Some(target)) = (id, action, target) else {
                    return Err(CliError::UsageError {
                        usage: "Usage: /connector grant <id> <action> <target-pattern>".to_string(),
                    });
                };
                await_with_prompts(
                    ctx,
                    ctx.handle
                        .connectors()
                        .grant(&parse_connector_id(id)?, action, target),
                )
                .await?;
                println!(
                    "{} Granted {action} on {target} for {id}.",
                    style::success("OK")
                );
                Ok(CommandOutcome::Continue)
            }
            Some("revoke") => {
                let (id, action, target) = (subparts.get(1), subparts.get(2), subparts.get(3));
                let (Some(id), Some(action), Some(target)) = (id, action, target) else {
                    return Err(CliError::UsageError {
                        usage: "Usage: /connector revoke <id> <action> <target-pattern>"
                            .to_string(),
                    });
                };
                let removed = await_with_prompts(
                    ctx,
                    ctx.handle
                        .connectors()
                        .revoke(&parse_connector_id(id)?, action, target),
                )
                .await?;
                if removed {
                    println!(
                        "{} Revoked {action} on {target} for {id}.",
                        style::success("OK")
                    );
                } else {
                    println!("No matching grant to revoke.");
                }
                Ok(CommandOutcome::Continue)
            }
            Some("permissions") => {
                let id = subparts.get(1).copied().map_or("", str::trim);
                if id.is_empty() {
                    return Err(CliError::UsageError {
                        usage: "Usage: /connector permissions <id>".to_string(),
                    });
                }
                let grants = connectors
                    .permissions(&parse_connector_id(id)?)
                    .map_err(actor_error)?;
                if grants.is_empty() {
                    println!("No permission grants for {id}.");
                } else {
                    println!("{}", style::success(format!("Permission grants for {id}:")));
                    for grant in grants {
                        println!(
                            "  [{}] {} on {} (granted {})",
                            style::header(grant.action.as_str()),
                            grant.action,
                            grant.target_pattern,
                            grant.granted_at.format("%Y-%m-%d %H:%M:%S")
                        );
                    }
                }
                Ok(CommandOutcome::Continue)
            }
            _ => Err(CliError::UsageError {
                usage: self.usage().to_string(),
            }),
        }
    }
}
