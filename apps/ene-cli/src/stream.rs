use ene_core::{
    EneEvent, EneEventReceiver, EneHandle, MultiAnswer, PermissionDecision, Truncate,
    UserInputResponse, extract_emotion_from_token,
};
use std::io::{self, Write};

/// Processes AI events from the actor in real-time, printing them to stdout.
/// Returns when the stream finishes or an error occurs.
pub async fn process_stream(rx: &mut EneEventReceiver, handle: &EneHandle) {
    loop {
        match rx.recv().await {
            Ok(EneEvent::TextDelta { delta }) => {
                print!("{delta}");
                let _ = io::stdout().flush();
            }
            Ok(EneEvent::SpecialToken { token }) => {
                if let Some(emotion) = extract_emotion_from_token(&token) {
                    print!("[Emotion: {emotion}]");
                } else {
                    print!("{token}");
                }
                let _ = io::stdout().flush();
            }
            Ok(EneEvent::Expression { name, source }) => {
                print!("\n[Expression: {name} ({source})]");
                let _ = io::stdout().flush();
            }
            Ok(EneEvent::ToolCallStart { name, arguments }) => {
                tracing::info!(tool = %name, arguments = %arguments, "Tool calling started");
            }
            Ok(EneEvent::ToolCallResult { name, result }) => {
                tracing::info!(tool = %name, result = %result, "Tool result");
            }
            Ok(EneEvent::SessionSplit { summary, reason }) => {
                tracing::info!(reason = %reason, summary = %Truncate::simple(&summary, 80), "Session split");
            }
            Ok(EneEvent::Terminal(reason)) => {
                if let ene_core::TerminalReason::Failed { message } = &reason {
                    tracing::error!(error = %message, "Terminal failure");
                } else {
                    tracing::info!(?reason, "Stream terminal");
                }
                break;
            }
            Ok(EneEvent::PermissionRequired {
                request_id,
                action,
                target,
                description,
            }) => {
                tracing::info!(
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
            Ok(EneEvent::UserInputRequired { request_id, prompt }) => {
                let total = prompt.items.len();
                tracing::info!(request_id = %request_id, total, "User input required");

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
                            .unwrap_or(choices.len().saturating_sub(1));

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
                        // No options, no free text: record a skip and move on.
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
            Ok(EneEvent::TaskProgress {
                task_id,
                step,
                total_steps,
                description,
            }) => {
                let steps_display = match total_steps {
                    Some(total) => format!("{step}/{total}"),
                    None => format!("{step}/?"),
                };
                tracing::info!(task_id, step = %steps_display, description = %description, "Task progress");
            }
            Ok(EneEvent::PipelinePhase { phase }) => {
                print!("\x1b[2K\r    {phase}...");
                let _ = io::stdout().flush();
            }
            Ok(EneEvent::PipelineMetrics { timings }) => {
                let emb_ms = timings.get("Embedding").copied().unwrap_or(0);
                let ctx_ms = timings
                    .get("Context Search (Total)")
                    .copied()
                    .or_else(|| timings.get("Context Search").copied())
                    .unwrap_or(0);
                let ctx_mem = timings
                    .get("Context Search (Memory DB)")
                    .copied()
                    .unwrap_or(0);
                let ctx_tool = timings
                    .get("Context Search (Tool RAG)")
                    .copied()
                    .unwrap_or(0);
                let prompt_ms = timings.get("Prompt Building").copied().unwrap_or(0);
                let total_ms = timings.get("Total Pre-generation").copied().unwrap_or(0);

                print!("\x1b[2K\r");
                tracing::info!(
                    emb_ms,
                    ctx_ms,
                    ctx_mem_ms = ctx_mem,
                    ctx_tool_ms = ctx_tool,
                    prompt_ms,
                    total_ms,
                    "Pipeline timings"
                );
            }
            Ok(EneEvent::StatusChanged { .. }) => {}
            Err(e) => {
                tracing::warn!(error = ?e, "Event receive error");
                break;
            }
        }
    }
}
