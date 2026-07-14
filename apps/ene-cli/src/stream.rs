use ene_runtime::{
    CueSource, EneEvent, EneEventReceiver, EneHandle, MultiAnswer, PerfKind, PermissionDecision,
    TurnId, UserInputResponse,
};
use std::io::{self, Write};

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
    loop {
        match rx.recv().await {
            Ok(EneEvent::TextDelta { turn, delta }) => {
                if !turn_matches(active_turn, &turn) {
                    continue;
                }
                print!("{delta}");
                let _ = io::stdout().flush();
            }
            Ok(EneEvent::Performance { turn, cues, source }) => {
                if !turn_matches(active_turn, &turn) {
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
                    print!("\n[Performance: {} ({kind_label}) ({label})]", cue.name);
                }
                let _ = io::stdout().flush();
            }
            Ok(EneEvent::ToolCallStart {
                turn,
                name,
                arguments,
            }) => {
                if !turn_matches(active_turn, &turn) {
                    continue;
                }
                tracing::info!(%turn, tool = %name, arguments = %arguments, "Tool calling started");
            }
            Ok(EneEvent::ToolCallResult { turn, name, result }) => {
                if !turn_matches(active_turn, &turn) {
                    continue;
                }
                tracing::info!(%turn, tool = %name, result = %result, "Tool result");
            }
            Ok(EneEvent::ContextCompressed { turn, level, .. }) => {
                if !turn_matches(active_turn, &turn) {
                    continue;
                }
                tracing::info!(%turn, level = %level, "Context compressed");
            }
            Ok(EneEvent::Terminal { turn, reason }) => {
                if !turn_matches(active_turn, &turn) {
                    continue;
                }
                if let ene_runtime::TerminalReason::Failed { message } = &reason {
                    tracing::error!(%turn, error = %message, "Terminal failure");
                } else {
                    tracing::info!(%turn, ?reason, "Stream terminal");
                }
                break;
            }
            Ok(EneEvent::PermissionRequired {
                turn,
                request_id,
                action,
                target,
                description,
            }) => {
                if !turn_matches(active_turn, &turn) {
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
                    "1回のみ許可 (Allow Once)",
                    "このセッションで常に許可 (Allow Session)",
                    "拒否 (Deny)",
                ];
                let selection = dialoguer::Select::new()
                    .with_prompt("操作の権限を選択してください")
                    .items(&choices)
                    .default(0)
                    .interact()
                    .unwrap_or(2);

                let decision = match selection {
                    0 => PermissionDecision::AllowOnce,
                    1 => PermissionDecision::AllowSession,
                    _ => PermissionDecision::Deny,
                };

                let _ = handle.decide_permission(request_id, decision);
                tracing::info!("Permission decision submitted; resuming processing");
            }
            Ok(EneEvent::UserInputRequired {
                turn,
                request_id,
                prompt,
            }) => {
                if !turn_matches(active_turn, &turn) {
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
                        choices.push("(skip)".to_string());
                        choices.push("(cancel all)".to_string());
                        let selection = dialoguer::Select::new()
                            .with_prompt("回答を選択 (上下キーで選択, Enterで確定)")
                            .items(&choices)
                            .default(0)
                            .interact()
                            .unwrap_or_else(|_| choices.len().saturating_sub(1));

                        let chosen = &choices[selection];
                        if chosen == "(cancel all)" {
                            cancelled = true;
                            break;
                        } else if chosen == "(skip)" {
                            MultiAnswer::Skip
                        } else {
                            MultiAnswer::Selected {
                                option: chosen.clone(),
                            }
                        }
                    } else if item.allow_free_text {
                        let text: String = dialoguer::Input::new()
                            .with_prompt("自由入力 (空でskip, 'cancel'で全キャンセル)")
                            .allow_empty(true)
                            .interact_text()
                            .unwrap_or_default();
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
                let _ = handle.submit_user_input(request_id, decision);
                tracing::info!("User input submitted; resuming processing");
            }
            Ok(EneEvent::StatusChanged { .. }) => {}
            Err(e) => {
                tracing::warn!(error = ?e, "Event receive error");
                break;
            }
        }
    }
}

fn turn_matches(active: Option<&TurnId>, event_turn: &TurnId) -> bool {
    active.is_none_or(|t| t == event_turn)
}
