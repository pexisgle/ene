use crate::style;
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
                    print!("{}", style::emotion(format!("[Emotion: {emotion}]")));
                } else {
                    print!("{}", style::warning(token));
                }
                let _ = io::stdout().flush();
            }
            Ok(EneEvent::ToolCallStart { name, arguments }) => {
                println!(
                    "\n{}",
                    style::header(format!("[Tool Calling: {name}({arguments})]"))
                );
            }
            Ok(EneEvent::ToolCallResult { name: _, result }) => {
                println!("{}\n", style::success(format!("[Tool Result: {result}]")));
            }
            Ok(EneEvent::SessionSplit { summary, reason }) => {
                println!("\n{}", style::warning(format!("[Session] {reason} ")));
                println!(
                    "{}",
                    style::warning(format!(
                        "[Session] Summary: {}",
                        Truncate::simple(&summary, 80)
                    ))
                );
            }
            Ok(EneEvent::Terminal(reason)) => {
                if let ene_core::TerminalReason::Failed { message } = &reason {
                    eprintln!("\n[Error] {message}");
                } else {
                    println!();
                }
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
                        "[Permission Required] {action} on {target} ({description})"
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
                let total = prompt.items.len();
                println!(
                    "\n{}",
                    style::header(format!("[Question] {total} 件の質問があります"))
                );

                let mut answers: Vec<MultiAnswer> = Vec::with_capacity(total);
                let mut cancelled = false;

                for (i, item) in prompt.items.iter().enumerate() {
                    println!(
                        "\n{}",
                        style::header(format!("({}/{}) {}", i + 1, total, item.question))
                    );

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
                    Some(total) => format!("{step}/{total}"),
                    None => format!("{step}/?"),
                };
                println!(
                    "\n{}",
                    style::header(format!(
                        "[Task {task_id}] Step {steps_display}: {description}"
                    ))
                );
            }
            Ok(EneEvent::StatusChanged { .. }) => {}
            Err(e) => {
                eprintln!("\n[Warning] Event receive error: {e:?}");
                break;
            }
        }
    }
}
