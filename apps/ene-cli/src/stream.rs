use crate::style;
use ene_core::{
    EneEvent, EneEventReceiver, EneHandle, PermissionDecision, Truncate, UserInputResponse,
    extract_emotion_from_token,
};
use std::io::{self, Write};

/// Processes AI events from the actor in real-time, printing them to stdout.
/// Returns when the stream finishes or an error occurs.
pub async fn process_stream(rx: &mut EneEventReceiver, handle: &EneHandle) {
    loop {
        match rx.recv().await {
            Ok(EneEvent::TextDelta { delta }) => {
                print!("{}", delta);
                let _ = io::stdout().flush();
            }
            Ok(EneEvent::SpecialToken { token }) => {
                if let Some(emotion) = extract_emotion_from_token(&token) {
                    print!("{}", style::emotion(format!("[Emotion: {}]", emotion)));
                } else {
                    print!("{}", style::warning(token));
                }
                let _ = io::stdout().flush();
            }
            Ok(EneEvent::ToolCallStart { name, arguments }) => {
                println!(
                    "\n{}",
                    style::header(format!("[Tool Calling: {}({})]", name, arguments))
                );
            }
            Ok(EneEvent::ToolCallResult { name: _, result }) => {
                println!("{}\n", style::success(format!("[Tool Result: {}]", result)));
            }
            Ok(EneEvent::SessionSplit { summary, reason }) => {
                println!("\n{}", style::warning(format!("[Session] {} ", reason)));
                println!(
                    "{}",
                    style::warning(format!(
                        "[Session] Summary: {}",
                        Truncate::simple(&summary, 80)
                    ))
                );
            }
            Ok(EneEvent::Done) => {
                println!();
                break;
            }
            Ok(EneEvent::PermissionRequired {
                request_id,
                action,
                target,
                description,
            }) => {
                println!(
                    "\n{}",
                    style::warning(format!(
                        "[Permission Required] {} on {} ({})",
                        action, target, description
                    ))
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
                println!(
                    "\n{}",
                    style::success("承認の入力を送信しました。処理を再開します...")
                );
            }
            Ok(EneEvent::UserInputRequired { request_id, prompt }) => {
                println!(
                    "\n{}",
                    style::header(format!("[Question] {}", prompt.question))
                );
                if let Some(header) = &prompt.header {
                    println!("{}", style::warning(format!("  (kind: {})", header)));
                }

                let response = if !prompt.options.is_empty() {
                    println!("\n選択肢から選んでください:");
                    let selection = dialoguer::Select::new()
                        .with_prompt("回答を選択")
                        .items(&prompt.options)
                        .default(0)
                        .interact()
                        .unwrap_or(prompt.options.len());
                    if selection < prompt.options.len() {
                        Some(UserInputResponse::Selected(
                            prompt.options[selection].clone(),
                        ))
                    } else {
                        None
                    }
                } else {
                    None
                };

                let response = response.or_else(|| {
                    if prompt.allow_free_text {
                        let text: String = dialoguer::Input::new()
                            .with_prompt("自由入力 (空でキャンセル)")
                            .allow_empty(true)
                            .interact_text()
                            .unwrap_or_default();
                        if !text.is_empty() {
                            Some(UserInputResponse::Answer(text))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                });

                let decision = response.unwrap_or(UserInputResponse::Cancel);
                let _ = handle.submit_user_input(request_id, decision);
                println!(
                    "\n{}",
                    style::success("回答を送信しました。処理を再開します...")
                );
            }
            Ok(EneEvent::TaskProgress {
                task_id,
                step,
                total_steps,
                description,
            }) => {
                let steps_display = match total_steps {
                    Some(total) => format!("{}/{}", step, total),
                    None => format!("{}/?", step),
                };
                println!(
                    "\n{}",
                    style::header(format!(
                        "[Task {}] Step {}: {}",
                        task_id, steps_display, description
                    ))
                );
            }
            Ok(EneEvent::Failed { message }) => {
                eprintln!("\n[Error] {}", message);
                break;
            }
            Ok(EneEvent::StatusChanged { .. }) => {}
            Err(e) => {
                eprintln!("\n[Warning] Event receive error: {:?}", e);
                break;
            }
        }
    }
}
