use crate::style;
use ene_core::{ConversationSession, stream::EneStreamEvent, truncate};
use std::io::{self, Write};
use tokio_stream::StreamExt;

pub async fn process_stream<S>(stream: S, session: &mut ConversationSession)
where
    S: futures_core::Stream<Item = EneStreamEvent>,
{
    session.reset_display_buffer();
    tokio::pin!(stream);

    while let Some(event) = stream.next().await {
        match event {
            EneStreamEvent::TextDelta(delta) => {
                let (text_deltas, special_tokens) = session.process_delta(&delta);
                for t in text_deltas {
                    print!("{}", t);
                    let _ = io::stdout().flush();
                }
                for token in special_tokens {
                    if let Some(emotion) = ene_core::extract_emotion_from_token(&token) {
                        print!("{}", style::emotion(format!("[Emotion: {}]", emotion)));
                    } else {
                        print!("{}", style::warning(token));
                    }
                    let _ = io::stdout().flush();
                }
            }
            EneStreamEvent::ToolCallStart { name, arguments } => {
                println!(
                    "\n{}",
                    style::header(format!("[Tool Calling: {}({})]", name, arguments))
                );
            }
            EneStreamEvent::ToolCallResult { name: _, result } => {
                println!("{}\n", style::success(format!("[Tool Result: {}]", result)));
            }
            EneStreamEvent::SessionSplit { summary, reason } => {
                println!("\n{}", style::warning(format!("[Session] {} ", reason)));
                println!(
                    "{}",
                    style::warning(format!("[Session] Summary: {}", truncate(&summary, 80)))
                );
            }
            EneStreamEvent::Finished => {
                if let Some(tail) = session.finalize_response() {
                    print!("{}", tail);
                    let _ = io::stdout().flush();
                }
                session.record_assistant_response();
                println!();
            }
            EneStreamEvent::PermissionRequired {
                request_id,
                action,
                target,
                description,
            } => {
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
                    0 => ene_core::stream::PermissionDecision::AllowOnce,
                    1 => ene_core::stream::PermissionDecision::AllowSession,
                    _ => ene_core::stream::PermissionDecision::Deny,
                };

                if let Err(e) = ene_core::stream::submit_permission_decision(&request_id, decision)
                {
                    eprintln!("\n[Error] Failed to submit permission decision: {}", e);
                } else {
                    println!(
                        "\n{}",
                        style::success("承認の入力を送信しました。処理を再開します...")
                    );
                }
            }
            EneStreamEvent::TaskProgress {
                task_id,
                step,
                total_steps,
                description,
            } => {
                println!(
                    "\n{}",
                    style::header(format!(
                        "[Task {}] Step {}/{}: {}",
                        task_id, step, total_steps, description
                    ))
                );
            }
            EneStreamEvent::Error(err) => {
                eprintln!("\n[Error] {}", err);
            }
            EneStreamEvent::SpecialToken(_) => {}
        }
    }
}
