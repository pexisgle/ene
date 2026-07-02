use crate::{
    commands::{self, CommandOutcome, SHUTDOWN_TIMEOUT},
    context::AppContext,
    stream,
};

/// Read a single line of REPL input on a blocking thread. Returns
/// `None` if stdin reaches EOF (Ctrl-D in a TTY, or the input stream
/// was closed).
async fn read_line() -> Option<String> {
    tokio::task::spawn_blocking(|| {
        dialoguer::Input::<String>::new()
            .with_prompt(">")
            .allow_empty(true)
            .interact()
            .ok()
    })
    .await
    .unwrap_or(None)
}

/// Drains the actor and returns the exit code. Used on both `/quit`
/// and Ctrl-C paths so the process does not abort with
/// `std::process::exit(0)` while pending memory writes, session
/// splits, and tool processes are still in flight.
async fn drain_and_exit(ctx: &mut AppContext, code: i32) -> i32 {
    match ctx.handle.shutdown(SHUTDOWN_TIMEOUT).await {
        Ok(()) => {}
        Err(e) => {
            tracing::error!(
                timeout = ?SHUTDOWN_TIMEOUT,
                error = %e,
                "Actor did not shut down within timeout"
            );
        }
    }
    code
}

pub async fn run(ctx: &mut AppContext) -> i32 {
    loop {
        tokio::select! {
            biased;

            // Ctrl-C: clean shutdown path. tokio::signal::ctrl_c()
            // resolves on the first SIGINT, including the one a TTY
            // forwards when the user presses Ctrl-C while
            // `dialoguer::Input::interact()` is blocked.
            ctrl_c_result = tokio::signal::ctrl_c() => {
                if let Err(e) = ctrl_c_result {
                    tracing::error!(error = %e, "Failed to install Ctrl-C handler");
                } else {
                    tracing::info!("[Runtime] Ctrl-C received, shutting down...");
                }
                return drain_and_exit(ctx, 130).await;
            }

            // Normal REPL line input. The blocking dialoguer call
            // runs on a worker thread; we await its result here.
            maybe_input = read_line() => {
                let Some(input) = maybe_input else {
                    // EOF (Ctrl-D) — treat like /quit so we still
                    // drain the actor before exiting.
                    return drain_and_exit(ctx, 0).await;
                };
                let input = input.trim().to_string();
                if input.is_empty() {
                    continue;
                }

                if input.starts_with('/') {
                    match commands::execute(&input, ctx).await {
                        CommandOutcome::Continue => continue,
                        CommandOutcome::Exit(code) => {
                            return drain_and_exit(ctx, code).await;
                        }
                    }
                }

                // Subscribe before sending the run command to avoid missing events
                let mut rx = ctx.handle.subscribe();
                let _ = ctx.handle.run(&input);
                stream::process_stream(&mut rx, &ctx.handle).await;
            }
        }
    }
}
