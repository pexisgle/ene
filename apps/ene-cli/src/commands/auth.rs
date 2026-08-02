use crate::commands::{CliCommand, CliError, CommandOutcome};
use crate::context::AppContext;
use crate::style;
use async_trait::async_trait;

pub struct AuthCommand;

#[async_trait]
impl CliCommand for AuthCommand {
    fn name(&self) -> &'static str {
        "/auth"
    }

    fn description(&self) -> &'static str {
        "List, inspect, and revoke stored credentials"
    }

    fn usage(&self) -> &'static str {
        "/auth <list|status <id>|revoke <id>|authorize <id>>"
    }

    async fn execute(&self, arg: &str, ctx: &mut AppContext) -> Result<CommandOutcome, CliError> {
        let subparts: Vec<&str> = arg.splitn(2, ' ').collect();
        match subparts.first().copied() {
            None | Some("list") => {
                let rows = ctx
                    .handle
                    .list_credentials()
                    .await
                    .map_err(|e| CliError::ActorError(e.to_string()))?;
                if rows.is_empty() {
                    println!("No credentials stored or declared.");
                    return Ok(CommandOutcome::Continue);
                }
                println!("{}", style::success("Credentials:"));
                for row in &rows {
                    let status = match (row.stored, row.expired) {
                        (true, true) => "expired",
                        (true, false) => "authorized",
                        (false, _) => "missing",
                    };
                    println!(
                        "  [{}] {} ({}, {status}, expires {})",
                        style::header(&row.id),
                        kind_label(row),
                        if row.stored { "stored" } else { "declared" },
                        row.expires_at
                            .map(|t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                            .unwrap_or_else(|| "never".to_string()),
                    );
                }
                Ok(CommandOutcome::Continue)
            }
            Some("authorize") => {
                // The flow opens a browser, so it cannot run from the CLI
                // (especially headless); point the user at the desktop app.
                Err(CliError::ExecutionFailed(
                    "The OAuth authorization flow is not available in the CLI; \
                     run it from the desktop app's Credentials settings page instead."
                        .to_string(),
                ))
            }
            Some("status") => {
                let id = subparts.get(1).copied().map_or("", str::trim);
                if id.is_empty() {
                    return Err(CliError::UsageError {
                        usage: "Usage: /auth status <id>".to_string(),
                    });
                }
                let rows = ctx
                    .handle
                    .list_credentials()
                    .await
                    .map_err(|e| CliError::ActorError(e.to_string()))?;
                match rows.iter().find(|row| row.id == id) {
                    Some(row) => {
                        let status = match (row.stored, row.expired) {
                            (true, true) => "expired — refresh may be required",
                            (true, false) => "authorized",
                            (false, _) => "not authorized",
                        };
                        println!(
                            "{}",
                            style::success(format!(
                                "{} ({}): {}; expires {}",
                                row.id,
                                kind_label(row),
                                status,
                                row.expires_at
                                    .map(|t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                                    .unwrap_or_else(|| "never".to_string())
                            ))
                        );
                    }
                    None => {
                        return Err(CliError::ExecutionFailed(format!(
                            "No credential with id '{id}'"
                        )));
                    }
                }
                Ok(CommandOutcome::Continue)
            }
            Some("revoke") => {
                let id = subparts.get(1).copied().map_or("", str::trim);
                if id.is_empty() {
                    return Err(CliError::UsageError {
                        usage: "Usage: /auth revoke <id>".to_string(),
                    });
                }
                let removed = ctx
                    .handle
                    .revoke_credential(vec![id.to_string()])
                    .await
                    .map_err(|e| CliError::ActorError(e.to_string()))?;
                if removed > 0 {
                    println!("{}", style::success(format!("Revoked credential '{id}'.")));
                } else {
                    return Err(CliError::ExecutionFailed(format!(
                        "No stored credential with id '{id}'"
                    )));
                }
                Ok(CommandOutcome::Continue)
            }
            _ => Err(CliError::UsageError {
                usage: self.usage().to_string(),
            }),
        }
    }
}

fn kind_label(row: &ene_plugin_host::oauth::CredentialInfo) -> &'static str {
    match row.kind {
        ene_plugin_host::oauth::CredentialKindName::OAuth2 => "OAuth2",
        ene_plugin_host::oauth::CredentialKindName::ApiKey => "api_key",
        ene_plugin_host::oauth::CredentialKindName::None => "none",
    }
}
