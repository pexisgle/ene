use crate::style;
use ene_ai_core::{session::ConversationSession, stream::AiStreamEvent, truncate};
use std::io::{self, Write};
use tokio_stream::StreamExt;

pub async fn process_stream<S>(stream: S, session: &mut ConversationSession)
where
    S: futures_core::Stream<Item = AiStreamEvent>,
{
    session.reset_display_buffer();
    tokio::pin!(stream);

    while let Some(event) = stream.next().await {
        match event {
            AiStreamEvent::TextDelta(delta) => {
                let (text_deltas, special_tokens) = session.process_delta(&delta);
                for t in text_deltas {
                    print!("{}", t);
                    let _ = io::stdout().flush();
                }
                for token in special_tokens {
                    if let Some(emotion) =
                        ene_ai_core::special_token::extract_emotion_from_token(&token)
                    {
                        print!("{}", style::emotion(format!("[Emotion: {}]", emotion)));
                    } else {
                        print!("{}", style::warning(token));
                    }
                    let _ = io::stdout().flush();
                }
            }
            AiStreamEvent::ToolCallStart { name, arguments } => {
                println!(
                    "\n{}",
                    style::header(format!("[Tool Calling: {}({})]", name, arguments))
                );
            }
            AiStreamEvent::ToolCallResult { name: _, result } => {
                println!("{}\n", style::success(format!("[Tool Result: {}]", result)));
            }
            AiStreamEvent::SessionSplit { summary, reason } => {
                println!("\n{}", style::warning(format!("[Session] {} ", reason)));
                println!(
                    "{}",
                    style::warning(format!("[Session] Summary: {}", truncate(&summary, 80)))
                );
            }
            AiStreamEvent::Finished => {
                if let Some(tail) = session.finalize_response() {
                    print!("{}", tail);
                    let _ = io::stdout().flush();
                }
                session.record_assistant_response();
                println!();
            }
            AiStreamEvent::PermissionRequired {
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
                println!(
                    "{}",
                    style::warning(format!(
                        "To approve, use: approve_permission {}",
                        request_id
                    ))
                );
            }
            AiStreamEvent::TaskProgress {
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
            AiStreamEvent::Error(err) => {
                println!("\n[Error] {}", err);
            }
            AiStreamEvent::SpecialToken(_) => {}
        }
    }
}
