use crate::{
    commands::{self, CommandOutcome, SHUTDOWN_TIMEOUT},
    context::AppContext,
    output::EXIT_INTERRUPTED,
    stream,
    terminal_ui::{self, TerminalUi},
};

/// Read a single line of REPL input on a blocking thread. Returns
/// `None` if stdin reaches EOF (Ctrl-D in a TTY, or the input stream
/// was closed).
async fn read_line() -> Option<String> {
    TerminalUi::global().read_line().await
}

/// Drains the actor and returns the exit code. Used on both `/quit`
/// and Ctrl-C paths so the process does not abort with
/// `std::process::exit(0)` while pending memory writes, session
/// splits, and tool processes are still in flight.
async fn drain_and_exit(ctx: &AppContext, code: i32) -> i32 {
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

/// Discard any text the user had typed into the in-flight line editor
/// and return the terminal to a clean state before a proactive turn
/// renders. The raw-mode editor owns the terminal while it runs, so it
/// must be cancelled and awaited to completion before the stream writes
/// to stderr; otherwise the two would fight over the same lines.
async fn cancel_editor(
    editor: &mut std::pin::Pin<Box<impl std::future::Future<Output = Option<String>>>>,
) {
    terminal_ui::request_read_cancel();
    drop(editor.as_mut().await);
    terminal_ui::clear_read_cancel();
}

/// Restart the line editor after a proactive turn finished rendering.
#[must_use = "the returned future must be polled to drive the editor"]
fn restart_editor() -> std::pin::Pin<Box<impl std::future::Future<Output = Option<String>>>> {
    Box::pin(read_line())
}

/// One iteration of the REPL `select!`, classified so the side effects
/// (which borrow the editor and the event receiver) can run *after* the
/// `select!` completes and releases its borrows.
enum ReplStep {
    /// Ctrl-C: shut down with the interrupted exit code.
    Interrupt,
    /// The chat bus closed: the runtime is gone, so drain and exit.
    Disconnected,
    /// A proactive (spontaneous) turn started while idle (#402).
    Proactive(ene_runtime::TurnId),
    /// The line editor produced input (or `None` on EOF).
    Input(Option<String>),
    /// A stray chat-bus event with nothing to render; loop again.
    Skip,
}

pub async fn run(ctx: &mut AppContext) -> i32 {
    if let Ok(snapshot) = ctx.handle.diagnostics().get_snapshot().await {
        let issues = ene_ai::validate_settings(&snapshot.config);
        if !issues.is_empty() {
            eprintln!(
                "{} AI settings look incomplete — use `/config` to review or `/config set` to fix.",
                crate::style::warning("WARN")
            );
            for issue in issues {
                eprintln!("  - {}", issue.message());
            }
        }
    }

    // Subscribe to the chat bus once, before the loop, and keep the
    // receiver alive for the whole session (#402). The desktop app
    // subscribes continuously (`ai_bridge`); the CLI used to subscribe
    // only for the duration of a turn, so while idle at the prompt the
    // broadcast channel had zero subscribers and proactive (spontaneous)
    // speech was silently dropped by `broadcast::Sender::send`. Holding
    // the receiver here also satisfies the "subscribe before `run`"
    // ordering naturally.
    let mut rx = ctx.handle.subscribe();

    // The line editor is held across loop iterations rather than
    // re-created inside the `read_line` arm: when a proactive turn
    // arrives mid-edit we must cancel and await the blocking editor
    // before rendering, and re-arm it afterwards.
    let mut editor = restart_editor();

    loop {
        let step = tokio::select! {
            biased;

            // Ctrl-C: clean shutdown path. tokio::signal::ctrl_c()
            // resolves on the first SIGINT, including the one a TTY
            // forwards when the user presses Ctrl-C while
            // line editing is blocked.
            ctrl_c_result = tokio::signal::ctrl_c() => {
                if let Err(e) = ctrl_c_result {
                    tracing::error!(error = %e, "Failed to install Ctrl-C handler");
                } else {
                    tracing::info!("[Runtime] Ctrl-C received, shutting down...");
                }
                ReplStep::Interrupt
            }

            // Continuous chat-bus subscription (#402): surface proactive
            // (spontaneous) turns that start while the REPL is idle at
            // the prompt. User-initiated turns are rendered by
            // `process_stream` inside the `Input` handling below, so a
            // `TurnStarted` observed here is necessarily proactive (the
            // host is single-flight).
            event = rx.recv() => {
                match event {
                    Ok(ene_runtime::EneEvent::TurnStarted {
                        turn,
                        origin: ene_runtime::TurnOrigin::Proactive,
                    }) => ReplStep::Proactive(turn),
                    // Turn-scoped events for the proactive turn arrive
                    // while `process_stream` owns `rx`; anything seen
                    // here is a stray event for a turn rendered
                    // elsewhere and is ignored.
                    Ok(_) => ReplStep::Skip,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(skipped, "Chat bus lagged while idle");
                        ReplStep::Skip
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        tracing::error!("Event channel closed; runtime disconnected");
                        ReplStep::Disconnected
                    }
                }
            }

            // Normal REPL line input. The blocking editor runs on a
            // worker thread; we await its result here.
            maybe_input = &mut editor => ReplStep::Input(maybe_input),
        };

        match step {
            ReplStep::Interrupt => {
                terminal_ui::request_read_cancel();
                return drain_and_exit(ctx, EXIT_INTERRUPTED).await;
            }
            ReplStep::Disconnected => {
                return drain_and_exit(ctx, 0).await;
            }
            ReplStep::Skip => {}
            ReplStep::Proactive(turn) => {
                tracing::info!(%turn, "Proactive turn started");
                cancel_editor(&mut editor).await;
                stream::process_stream(&mut rx, &ctx.handle, &turn).await;
                editor = restart_editor();
            }
            ReplStep::Input(maybe_input) => {
                let Some(input) = maybe_input else {
                    // EOF (Ctrl-D) — treat like /quit so we still
                    // drain the actor before exiting.
                    return drain_and_exit(ctx, 0).await;
                };
                let input = input.trim().to_string();
                if !input.is_empty() {
                    if input.starts_with('/') {
                        match commands::execute(&input, ctx).await {
                            CommandOutcome::Continue => {}
                            CommandOutcome::Exit(code) => {
                                return drain_and_exit(ctx, code).await;
                            }
                        }
                    } else {
                        // `rx` was subscribed before the loop, so no
                        // events are missed between `run` and
                        // `process_stream`.
                        match ctx.handle.run(&input) {
                            Ok(turn) => {
                                tracing::info!(%turn, "Turn started");
                                stream::process_stream(&mut rx, &ctx.handle, &turn).await;
                            }
                            Err(ene_runtime::RunError::Busy) => {
                                println!(
                                    "{}",
                                    crate::style::warning(i18n_embed_fl::fl!(
                                        crate::i18n::loader(),
                                        "busy-warning"
                                    ))
                                );
                            }
                            Err(e) => {
                                println!(
                                    "{}",
                                    crate::style::error(i18n_embed_fl::fl!(
                                        crate::i18n::loader(),
                                        "run-failed",
                                        error = e.to_string()
                                    ))
                                );
                            }
                        }
                    }
                }
                editor = restart_editor();
            }
        }
    }
}
