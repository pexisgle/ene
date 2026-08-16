use crate::commands::{CliCommand, CliError, CommandOutcome};
use crate::context::AppContext;
use crate::style;
use async_trait::async_trait;
use ene_runtime::EneRuntimeError;

pub struct WorkspaceCommand;

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() > max_chars {
        let truncated: String = text.chars().take(max_chars.saturating_sub(3)).collect();
        format!("{truncated}...")
    } else {
        text.to_string()
    }
}

fn phase_label(phase: ene_runtime::workspace::WorkspaceSyncPhase) -> &'static str {
    match phase {
        ene_runtime::workspace::WorkspaceSyncPhase::Discovering => "discovering",
        ene_runtime::workspace::WorkspaceSyncPhase::Embedding => "embedding",
        ene_runtime::workspace::WorkspaceSyncPhase::Pruning => "pruning",
        ene_runtime::workspace::WorkspaceSyncPhase::Done => "done",
    }
}

#[async_trait]
impl CliCommand for WorkspaceCommand {
    fn name(&self) -> &'static str {
        "/workspace"
    }

    fn description(&self) -> &'static str {
        "Manage the workspace document index"
    }

    fn usage(&self) -> &'static str {
        "/workspace <sync|cancel|status|search <query>>"
    }

    async fn execute(&self, arg: &str, ctx: &mut AppContext) -> Result<CommandOutcome, CliError> {
        let subparts: Vec<&str> = arg.splitn(2, ' ').collect();
        match subparts.first().copied() {
            Some("sync") => {
                match ctx.handle.workspace().start_sync().await {
                    Ok(()) => {
                        println!(
                            "{}",
                            style::success(i18n_embed_fl::fl!(
                                crate::i18n::loader(),
                                "workspace-sync-started"
                            ))
                        );
                    }
                    Err(EneRuntimeError::Busy { .. }) => {
                        println!(
                            "{}",
                            i18n_embed_fl::fl!(crate::i18n::loader(), "workspace-sync-busy")
                        );
                    }
                    Err(e) => {
                        return Err(CliError::ExecutionFailed(format!(
                            "Failed to start workspace sync: {e}"
                        )));
                    }
                }
                Ok(CommandOutcome::Continue)
            }
            Some("cancel") => {
                ctx.handle
                    .workspace()
                    .cancel_sync()
                    .map_err(|e| CliError::ExecutionFailed(format!("Failed to cancel: {e}")))?;
                println!(
                    "{}",
                    i18n_embed_fl::fl!(crate::i18n::loader(), "workspace-cancel-sent")
                );
                Ok(CommandOutcome::Continue)
            }
            Some("status") => {
                let status = ctx.handle.workspace().status().await.map_err(|e| {
                    CliError::ExecutionFailed(format!("Failed to read status: {e}"))
                })?;
                println!(
                    "{}",
                    style::header(i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "workspace-status-title"
                    ))
                );
                if status.enabled {
                    if status.folders.is_empty() {
                        println!(
                            "{}",
                            i18n_embed_fl::fl!(
                                crate::i18n::loader(),
                                "workspace-status-no-folders"
                            )
                        );
                    } else {
                        println!(
                            "{}",
                            i18n_embed_fl::fl!(crate::i18n::loader(), "workspace-status-folders")
                        );
                        for folder in &status.folders {
                            println!("  - {folder}");
                        }
                    }
                    println!(
                        "{}",
                        i18n_embed_fl::fl!(
                            crate::i18n::loader(),
                            "workspace-status-indexed",
                            files = status.indexed_files,
                            chunks = status.indexed_chunks
                        )
                    );
                    if status.in_progress {
                        let p = &status.progress;
                        println!(
                            "{}",
                            i18n_embed_fl::fl!(
                                crate::i18n::loader(),
                                "workspace-status-progress",
                                phase = phase_label(p.phase),
                                scanned = p.files_scanned,
                                indexed = p.files_indexed,
                                skipped = p.files_skipped,
                                chunks = p.chunks_embedded
                            )
                        );
                        if let Some(file) = &p.current_file {
                            println!("  {file}");
                        }
                    }
                    if let Some(report) = &status.last_report {
                        println!(
                            "{}",
                            i18n_embed_fl::fl!(
                                crate::i18n::loader(),
                                "workspace-status-report",
                                indexed = report.files_indexed,
                                unchanged = report.files_unchanged,
                                renamed = report.files_renamed,
                                deleted = report.files_deleted,
                                skipped = report.files_skipped,
                                chunks = report.chunks_embedded,
                                elapsed = report.elapsed.as_secs()
                            )
                        );
                    }
                    if let Some(error) = &status.last_error {
                        println!(
                            "{}",
                            i18n_embed_fl::fl!(
                                crate::i18n::loader(),
                                "workspace-status-last-error",
                                error = error
                            )
                        );
                    }
                } else {
                    println!(
                        "{}",
                        i18n_embed_fl::fl!(crate::i18n::loader(), "workspace-status-disabled")
                    );
                }
                Ok(CommandOutcome::Continue)
            }
            Some("search") => {
                let query = subparts.get(1).map_or("", |q| q.trim());
                if query.is_empty() {
                    return Err(CliError::UsageError {
                        usage: "Usage: /workspace search <query>".to_string(),
                    });
                }
                // The configured final_n caps manual search results, matching
                // the prompt-injection budget and the config docs.
                let config = ene_config::get_global_config()
                    .get_section::<ene_rag::WorkspaceRagConfig>()
                    .unwrap_or_default();
                let limit = config.final_n.max(1);
                let hits = ctx
                    .handle
                    .workspace()
                    .search(query.to_string(), limit)
                    .await
                    .map_err(|e| CliError::ExecutionFailed(format!("Search failed: {e}")))?;
                if hits.is_empty() {
                    println!(
                        "{}",
                        i18n_embed_fl::fl!(crate::i18n::loader(), "workspace-search-empty")
                    );
                } else {
                    println!(
                        "{}",
                        style::success(i18n_embed_fl::fl!(
                            crate::i18n::loader(),
                            "workspace-search-title"
                        ))
                    );
                    for hit in &hits {
                        let heading = if hit.heading.is_empty() {
                            String::new()
                        } else {
                            format!(" [{}]", hit.heading)
                        };
                        println!(
                            "{} (score {:.2})",
                            style::header(format!(
                                "{}:{}-{}{}",
                                hit.path, hit.start_line, hit.end_line, heading
                            )),
                            hit.similarity
                        );
                        println!("  {}", truncate(&hit.content, 300));
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
