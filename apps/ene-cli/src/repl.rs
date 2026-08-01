use crate::{
    commands::{self, CommandOutcome, SHUTDOWN_TIMEOUT},
    context::AppContext,
    output::EXIT_INTERRUPTED,
    stream,
    terminal_ui::{self, TerminalUi},
};
use ene_runtime::{EneEvent, EneEventReceiver, TurnId, TurnOrigin};
use std::future::Future;
use std::pin::Pin;
use tokio::sync::broadcast::error::RecvError;

async fn read_line() -> Option<String> {
    TerminalUi::global().read_line().await
}

/// Shut the actor down before returning, so the process does not abort with
/// `std::process::exit(0)` while pending memory writes, session splits, and
/// tool processes are still in flight.
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

/// The raw-mode editor owns the terminal while it runs, so it must be
/// cancelled and awaited to completion before the stream writes to stderr;
/// otherwise the two would fight over the same lines.
async fn cancel_editor(editor: &mut Pin<Box<impl Future<Output = Option<String>>>>) {
    terminal_ui::request_read_cancel();
    let _ = editor.as_mut().await;
    terminal_ui::clear_read_cancel();
}

#[must_use = "the returned future must be polled to drive the editor"]
fn restart_editor() -> Pin<Box<impl Future<Output = Option<String>>>> {
    Box::pin(read_line())
}

/// One iteration of the REPL `select!`, classified so the side effects
/// (which borrow the editor and the event receiver) can run *after* the
/// `select!` completes and releases its borrows.
enum ReplStep {
    /// Shut down with the interrupted exit code.
    Interrupt,
    /// The runtime is gone; drain and exit.
    Disconnected,
    /// A proactive (spontaneous) turn started while idle.
    Proactive(TurnId),
    /// The line editor produced input (or `None` on EOF).
    Input(Option<String>),
    /// A stray chat-bus event with nothing to render; loop again.
    Skip,
}

/// Extracted from `run` so the arm ordering can be locked down in unit tests.
/// The `biased` order is deliberate: Ctrl-C first, then the line
/// editor, then the chat bus. If a ready `TurnStarted::Proactive` were
/// polled before a *completed* editor, the proactive arm would win,
/// `cancel_editor` would await the already-finished future and drop the
/// line the user had just submitted — silently losing their message.
/// Input wins instead, so the user sees the existing single-flight
/// busy-warning (or their turn renders), and the buffered proactive
/// event is received on the next iteration. A *pending* editor yields
/// on its first poll, so the chat-bus arm is still reached and
/// proactive turns are never starved.
async fn next_step<B>(
    bus: B,
    editor: &mut Pin<Box<impl Future<Output = Option<String>>>>,
) -> ReplStep
where
    B: Future<Output = Result<EneEvent, RecvError>>,
{
    tokio::select! {
        biased;

        // tokio::signal::ctrl_c() resolves on the first SIGINT, including
        // the one a TTY forwards when the user presses Ctrl-C while
        // line editing is blocked.
        ctrl_c_result = tokio::signal::ctrl_c() => {
            if let Err(e) = ctrl_c_result {
                tracing::error!(error = %e, "Failed to install Ctrl-C handler");
            } else {
                tracing::info!("[Runtime] Ctrl-C received, shutting down...");
            }
            ReplStep::Interrupt
        }

        // Polled before the chat bus so a completed edit always beats a
        // concurrently buffered proactive event.
        maybe_input = editor.as_mut() => ReplStep::Input(maybe_input),

        // User-initiated turns are rendered by `process_stream` inside the
        // `Input` handling below, so a `TurnStarted` observed here is
        // necessarily proactive (the host is single-flight).
        event = bus => {
            match event {
                Ok(EneEvent::TurnStarted {
                    turn,
                    origin: TurnOrigin::Proactive,
                }) => ReplStep::Proactive(turn),
                // Turn-scoped events for the proactive turn arrive
                // while `process_stream` owns `rx`; anything seen
                // here is a stray event for a turn rendered
                // elsewhere and is ignored.
                Ok(_) => ReplStep::Skip,
                Err(RecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "Chat bus lagged while idle");
                    ReplStep::Skip
                }
                Err(RecvError::Closed) => {
                    tracing::error!("Event channel closed; runtime disconnected");
                    ReplStep::Disconnected
                }
            }
        }
    }
}

/// Render a turn's chat stream while still honouring Ctrl-C.
///
/// The REPL's `select!` is not polled while `process_stream` awaits
/// events, so without this a SIGINT arriving mid-render is captured by
/// the tokio signal handler but not acted on until the turn finishes —
/// leaving an unresponsive REPL on a long turn. Returns `true` when
/// Ctrl-C was received; the caller drains and exits.
async fn stream_turn(rx: &mut EneEventReceiver, ctx: &AppContext, turn: &TurnId) -> bool {
    tokio::select! {
        biased;
        ctrl_c_result = tokio::signal::ctrl_c() => {
            if let Err(e) = ctrl_c_result {
                tracing::error!(error = %e, "Failed to install Ctrl-C handler");
            } else {
                tracing::info!("[Runtime] Ctrl-C received, shutting down...");
            }
            true
        }
        () = stream::process_stream(rx, &ctx.handle, turn) => false,
    }
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

    // A broadcast channel silently drops `send` calls when it has no
    // subscribers, so proactive (spontaneous) turns arriving while the REPL
    // is idle at the prompt would be lost without a standing subscription.
    // Holding the receiver here also satisfies the "subscribe before `run`"
    // ordering naturally.
    let mut rx = ctx.handle.subscribe();

    // The line editor is held across loop iterations rather than
    // re-created inside the `read_line` arm: when a proactive turn
    // arrives mid-edit we must cancel and await the blocking editor
    // before rendering, and re-arm it afterwards.
    let mut editor = restart_editor();

    loop {
        let step = next_step(rx.recv(), &mut editor).await;

        match step {
            ReplStep::Interrupt => {
                terminal_ui::request_read_cancel();
                return drain_and_exit(ctx, EXIT_INTERRUPTED).await;
            }
            ReplStep::Disconnected => {
                // Ask any in-flight line editor to unwind (raw-mode
                // terminal restore) before draining, mirroring the
                // Ctrl-C path above.
                terminal_ui::request_read_cancel();
                return drain_and_exit(ctx, 0).await;
            }
            ReplStep::Skip => {}
            ReplStep::Proactive(turn) => {
                tracing::info!(%turn, "Proactive turn started");
                if TerminalUi::global().is_interactive() {
                    cancel_editor(&mut editor).await;
                    if stream_turn(&mut rx, ctx, &turn).await {
                        return drain_and_exit(ctx, EXIT_INTERRUPTED).await;
                    }
                    editor = restart_editor();
                }
                // Non-interactive stdin (piped/redirected): the stdio
                // fallback read blocks on `read_line` and cannot observe
                // `request_read_cancel`, so `cancel_editor` would hang
                // until the next input line. Skip the cancel/render dance;
                // proactive turns are an interactive-mode feature.
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
                                if stream_turn(&mut rx, ctx, &turn).await {
                                    return drain_and_exit(ctx, EXIT_INTERRUPTED).await;
                                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::{pending, ready};

    fn proactive_started() -> EneEvent {
        EneEvent::TurnStarted {
            turn: TurnId::new(),
            origin: TurnOrigin::Proactive,
        }
    }

    /// A completed editor is polled (`biased`) before a buffered proactive
    /// `TurnStarted`, so the submitted input wins; without the biased order
    /// the proactive arm would drop the user's message via `cancel_editor`.
    #[tokio::test]
    async fn completed_editor_beats_buffered_proactive_turn() {
        let mut editor = Box::pin(ready(Some("hello".to_string())));
        let step = next_step(ready(Ok(proactive_started())), &mut editor).await;
        assert!(matches!(step, ReplStep::Input(Some(line)) if line == "hello"));
    }

    /// A *pending* editor yields on its first poll, so the chat-bus arm is
    /// still reached: a proactive turn buffered while the user is mid-edit
    /// surfaces immediately rather than being starved.
    #[tokio::test]
    async fn pending_editor_does_not_starve_chat_bus() {
        let mut editor = Box::pin(pending::<Option<String>>());
        let step = next_step(ready(Ok(proactive_started())), &mut editor).await;
        assert!(matches!(step, ReplStep::Proactive(_)));
    }

    /// Events for user-initiated turns, and bus lag, are not proactive;
    /// they are ignored while idle. Only a closed bus signals that the
    /// runtime is gone.
    #[tokio::test]
    async fn idle_chat_bus_classification() {
        let mut editor = Box::pin(pending::<Option<String>>());
        let user_started = EneEvent::TurnStarted {
            turn: TurnId::new(),
            origin: TurnOrigin::User,
        };
        assert!(matches!(
            next_step(ready(Ok(user_started)), &mut editor).await,
            ReplStep::Skip
        ));
        assert!(matches!(
            next_step(ready(Err(RecvError::Lagged(3))), &mut editor).await,
            ReplStep::Skip
        ));
        assert!(matches!(
            next_step(ready(Err(RecvError::Closed)), &mut editor).await,
            ReplStep::Disconnected
        ));
    }
}
