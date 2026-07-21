use crate::commands::{CliCommand, CliError};
use crate::context::AppContext;
use crate::style;
use async_trait::async_trait;

pub struct PermissionsCommand;

#[async_trait]
impl CliCommand for PermissionsCommand {
    fn name(&self) -> &'static str {
        "/permissions"
    }

    fn description(&self) -> &'static str {
        "List and manage tool permission grants"
    }

    fn usage(&self) -> &'static str {
        "/permissions <list|revoke <id>|reset>"
    }

    async fn execute(&self, arg: &str, ctx: &mut AppContext) -> Result<(), CliError> {
        let subparts: Vec<&str> = arg.splitn(2, ' ').collect();
        match subparts.first().copied() {
            None | Some("list") => {
                let scopes = ctx
                    .handle
                    .list_permissions()
                    .await
                    .map_err(|e| CliError::ActorError(e.to_string()))?;
                if scopes.is_empty() {
                    println!("No session permission grants.");
                } else {
                    println!("{}", style::success("Session permission grants:"));
                    for scope in scopes {
                        println!(
                            "  [{}] {} on {} ({:?}, granted {})",
                            style::header(scope.id.to_string()),
                            scope.action,
                            scope.target_pattern,
                            scope.grant_type,
                            scope.granted_at.format("%Y-%m-%d %H:%M:%S")
                        );
                    }
                }
                Ok(())
            }
            Some("revoke") => {
                let id_str = subparts.get(1).copied().map_or("", str::trim);
                let id: u64 = id_str.parse().map_err(|_| CliError::UsageError {
                    usage: "Usage: /permissions revoke <id>".to_string(),
                })?;
                if ctx
                    .handle
                    .revoke_permission(id)
                    .await
                    .map_err(|e| CliError::ActorError(e.to_string()))?
                {
                    println!(
                        "{}",
                        style::success(format!("Revoked permission grant {id}."))
                    );
                    Ok(())
                } else {
                    Err(CliError::ExecutionFailed(format!(
                        "No permission grant with id {id}"
                    )))
                }
            }
            Some("reset") => {
                let count = ctx
                    .handle
                    .reset_all_permissions()
                    .await
                    .map_err(|e| CliError::ActorError(e.to_string()))?;
                println!(
                    "{}",
                    style::success(format!("Revoked {count} permission grant(s)."))
                );
                Ok(())
            }
            _ => Err(CliError::UsageError {
                usage: self.usage().to_string(),
            }),
        }
    }
}
