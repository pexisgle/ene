use ene_runtime::{
    CueSource, EneEvent, EneEventReceiver, EneHandle, MultiAnswer, PerfKind, PermissionDecision,
    TurnId, UserInputResponse,
};

use crate::terminal_ui::TerminalUi;

/// Processes AI events from the actor in real-time, printing them to stdout.
///
/// When `active_turn` is set, turn-scoped events for other turns are ignored
/// (single-flight hosts should only see one turn, but this keeps the stream
/// safe if a lagged subscriber still holds an older id).
/// Returns when the matching stream finishes or an error occurs.
pub async fn process_stream(
    rx: &mut EneEventReceiver,
    handle: &EneHandle,
    active_turn: Option<&TurnId>,
) {
    let ui = TerminalUi::global();
    let mut subscribed_turn = active_turn.cloned();
    loop {
        match rx.recv().await {
            Ok(EneEvent::TurnStarted { turn, origin }) => {
                if subscribed_turn.is_none() && origin == ene_runtime::TurnOrigin::Proactive {
                    subscribed_turn = Some(turn);
                }
            }
            Ok(EneEvent::TextDelta {
                turn,
                origin: _,
                delta,
            }) => {
                if !turn_matches(subscribed_turn.as_ref(), &turn) {
                    continue;
                }
                ui.write_stream(&delta);
            }
            Ok(EneEvent::Performance {
                turn,
                origin: _,
                cues,
                source,
            }) => {
                if !turn_matches(subscribed_turn.as_ref(), &turn) {
                    continue;
                }
                for cue in cues {
                    let label = match source {
                        CueSource::Affect => "affect",
                        CueSource::LlmAdvisory => "llm_advisory",
                        CueSource::LlmCommand => "llm_command",
                        CueSource::Hysteresis => "hysteresis",
                        CueSource::Fallback => "fallback",
                    };
                    let kind_label = match cue.kind {
                        PerfKind::Expression => "expr",
                        PerfKind::Motion => "motion",
                        PerfKind::LookAt => "lookat",
                        PerfKind::Cancel => "cancel",
                    };
                    ui.write_stream(&format!(
                        "\n[Performance: {} ({kind_label}) ({label})]",
                        cue.name
                    ));
                }
            }
            Ok(EneEvent::ToolCallStart {
                turn,
                origin: _,
                name,
                arguments,
            }) => {
                if !turn_matches(subscribed_turn.as_ref(), &turn) {
                    continue;
                }
                tracing::info!(%turn, tool = %name, arguments = %arguments, "Tool calling started");
            }
            Ok(EneEvent::ToolCallResult {
                turn,
                origin: _,
                name,
                result,
            }) => {
                if !turn_matches(subscribed_turn.as_ref(), &turn) {
                    continue;
                }
                tracing::info!(%turn, tool = %name, result = %result, "Tool result");
            }
            Ok(EneEvent::ContextCompressed { turn, level, .. }) => {
                if !turn_matches(subscribed_turn.as_ref(), &turn) {
                    continue;
                }
                tracing::info!(%turn, level = %level, "Context compressed");
            }
            Ok(EneEvent::Terminal {
                turn,
                origin: _,
                reason,
            }) => {
                if !turn_matches(subscribed_turn.as_ref(), &turn) {
                    continue;
                }
                ui.end_stream();
                if let ene_runtime::TerminalReason::Failed { message } = &reason {
                    let user_message = crate::runtime_error::user_message_from_turn(message);
                    tracing::error!(%turn, error = %message, "Terminal failure");
                    eprintln!(
                        "{}",
                        crate::style::error(i18n_embed_fl::fl!(
                            crate::i18n::loader(),
                            "turn-failed",
                            detail = user_message
                        ))
                    );
                } else {
                    tracing::debug!(%turn, ?reason, "Stream terminal");
                }
                break;
            }
            Ok(EneEvent::PermissionRequired {
                turn,
                origin: _,
                request_id,
                action,
                target,
                description,
            }) => {
                if !turn_matches(subscribed_turn.as_ref(), &turn) {
                    continue;
                }
                tracing::info!(
                    %turn,
                    request_id = %request_id,
                    action = %action,
                    target = %target,
                    description = %description,
                    "Permission required"
                );

                let choices = vec![
                    i18n_embed_fl::fl!(crate::i18n::loader(), "permission-allow-once"),
                    i18n_embed_fl::fl!(crate::i18n::loader(), "permission-allow-session"),
                    i18n_embed_fl::fl!(crate::i18n::loader(), "permission-deny"),
                ];
                ui.pause_for_external_prompt();
                let selection = dialoguer::Select::new()
                    .with_prompt(i18n_embed_fl::fl!(
                        crate::i18n::loader(),
                        "permission-prompt"
                    ))
                    .items(&choices)
                    .default(0)
                    .interact()
                    .unwrap_or(2);
                ui.resume_after_external_prompt();

                let decision = match selection {
                    0 => PermissionDecision::AllowOnce,
                    1 => PermissionDecision::AllowSession,
                    _ => PermissionDecision::Deny,
                };

                drop(handle.decide_permission(request_id, decision));
                tracing::info!("Permission decision submitted; resuming processing");
            }
            Ok(EneEvent::UserInputRequired {
                turn,
                origin: _,
                request_id,
                prompt,
            }) => {
                if !turn_matches(subscribed_turn.as_ref(), &turn) {
                    continue;
                }
                let total = prompt.items.len();
                tracing::info!(%turn, request_id = %request_id, total, "User input required");

                let mut answers: Vec<MultiAnswer> = Vec::with_capacity(total);
                let mut cancelled = false;

                for (i, item) in prompt.items.iter().enumerate() {
                    tracing::info!(index = i + 1, total, question = %item.question, "Question prompt");

                    let answer = if !item.options.is_empty() {
                        let mut choices: Vec<String> = item.options.clone();
                        choices.push(i18n_embed_fl::fl!(crate::i18n::loader(), "user-input-skip"));
                        choices.push(i18n_embed_fl::fl!(
                            crate::i18n::loader(),
                            "user-input-cancel"
                        ));
                        ui.pause_for_external_prompt();
                        let selection = dialoguer::Select::new()
                            .with_prompt(i18n_embed_fl::fl!(
                                crate::i18n::loader(),
                                "user-input-select"
                            ))
                            .items(&choices)
                            .default(0)
                            .interact()
                            .unwrap_or_else(|_| choices.len().saturating_sub(1));
                        ui.resume_after_external_prompt();

                        let chosen = &choices[selection];
                        if chosen == &i18n_embed_fl::fl!(crate::i18n::loader(), "user-input-cancel")
                        {
                            cancelled = true;
                            break;
                        } else if chosen
                            == &i18n_embed_fl::fl!(crate::i18n::loader(), "user-input-skip")
                        {
                            MultiAnswer::Skip
                        } else {
                            MultiAnswer::Selected {
                                option: chosen.clone(),
                            }
                        }
                    } else if item.allow_free_text {
                        ui.pause_for_external_prompt();
                        let text: String = dialoguer::Input::new()
                            .with_prompt(i18n_embed_fl::fl!(
                                crate::i18n::loader(),
                                "user-input-freetext"
                            ))
                            .allow_empty(true)
                            .interact_text()
                            .unwrap_or_default();
                        ui.resume_after_external_prompt();
                        if text.eq_ignore_ascii_case("cancel") {
                            cancelled = true;
                            break;
                        } else if text.is_empty() {
                            MultiAnswer::Skip
                        } else {
                            MultiAnswer::Answer { text }
                        }
                    } else {
                        MultiAnswer::Skip
                    };

                    answers.push(answer);
                }

                let decision = if cancelled {
                    UserInputResponse::Cancel
                } else {
                    UserInputResponse::Multi(answers)
                };
                drop(handle.submit_user_input(request_id, decision));
                tracing::info!("User input submitted; resuming processing");
            }
            Err(e) => {
                tracing::warn!(error = ?e, "Event receive error");
                break;
            }
        }
    }
}

fn turn_matches(active: Option<&TurnId>, event_turn: &TurnId) -> bool {
    // When no turn is subscribed yet, accept all events so the loop can
    // observe a Terminal event and exit instead of hanging forever.
    active.is_none_or(|t| t == event_turn)
}
