use crate::{commands, context::AppContext, style};
use ene_core::truncate;
use tokio_stream::StreamExt;

pub async fn run(ctx: &mut AppContext) {
    loop {
        let input = match dialoguer::Input::<String>::new()
            .with_prompt(">")
            .allow_empty(true)
            .interact()
        {
            Ok(input) => input,
            Err(_) => break,
        };
        let input = input.trim().to_string();
        if input.is_empty() {
            continue;
        }

        if input.starts_with('/') {
            commands::execute(&input, ctx).await;
            continue;
        }

        check_session_split(ctx).await;
        match ctx.runtime.run(&input).await {
            Ok(stream) => {
                tokio::pin!(stream);
                while let Some(event) = stream.next().await {
                    match event {
                        ene_core::EneStreamEvent::TextDelta(delta) => {
                            let (text_deltas, special_tokens) =
                                ctx.runtime.session.process_delta(&delta);
                            for td in text_deltas {
                                print!("{}", td);
                            }
                            for token in special_tokens {
                                println!("\n[{}]", token);
                            }
                        }
                        ene_core::EneStreamEvent::SpecialToken(token) => {
                            println!("\n[{}]", token);
                        }
                        ene_core::EneStreamEvent::ToolCallStart { name, arguments } => {
                            println!(
                                "\n{}",
                                style::header(
                                    format!("Tool: {}", name)
                                )
                            );
                            println!(
                                "{}",
                                style::warning(format!("Args: {}", arguments))
                            );
                        }
                        ene_core::EneStreamEvent::ToolCallResult { name, result } => {
                            println!(
                                "{}",
                                style::success(format!("Tool {} result: {}", name, &result[..result.len().min(200)]))
                            );
                        }
                        ene_core::EneStreamEvent::PermissionRequired {
                            request_id,
                            action,
                            target,
                            description,
                        } => {
                            println!(
                                "\n{}",
                                style::warning(format!(
                                    "Permission required: {} ({})",
                                    action, target
                                ))
                            );
                            println!(
                                "   {}",
                                style::warning(description)
                            );
                            handle_permission_request(request_id);
                        }
                        ene_core::EneStreamEvent::TaskProgress {
                            task_id: _,
                            step,
                            total_steps,
                            description,
                        } => {
                            println!(
                                "{}",
                                style::warning(format!(
                                    "[Progress: {}/{}] {}",
                                    step, total_steps, description
                                ))
                            );
                        }
                        ene_core::EneStreamEvent::Finished => {
                            if let Some(tail) = ctx.runtime.session.finalize_response() {
                                print!("{}", tail);
                            }
                            println!();
                        }
                        ene_core::EneStreamEvent::Error(err) => {
                            eprintln!(
                                "{}",
                                style::error(format!("Error: {}", err))
                            );
                        }
                        _ => {}
                    }
                }
            }
            Err(err) => {
                eprintln!(
                    "{}",
                    style::error(format!("Stream start error: {}", err))
                );
            }
        }
    }
}

async fn check_session_split(ctx: &mut AppContext) {
    match ctx.runtime.apply_pending_split() {
        Some(Ok(result)) => {
            println!(
                "\n{}",
                style::warning(format!("[Session] {} ", result.reason))
            );
            println!(
                "{}",
                style::warning(format!(
                    "[Session] 会話を要約して保存しました: {}",
                    truncate(&result.summary, 80)
                ))
            );
            if !result.key_facts.is_empty() {
                let facts_str = result
                    .key_facts
                    .iter()
                    .map(|f| format!("{}:{}", f.key, f.value))
                    .collect::<Vec<_>>()
                    .join(", ");
                println!(
                    "{}",
                    style::warning(format!("[Session] 重要な事実: {}", facts_str))
                );
            }
            println!("{}\n", style::warning("[Session] 新しい会話を開始します。"));
        }
        Some(Err(e)) => {
            if !matches!(e, ene_core::SessionError::SplitNotNeeded) {
                eprintln!(
                    "{}",
                    style::error(format!("[Session] 要約生成エラー: {}", e))
                );
            }
        }
        None => {}
    }
}

fn handle_permission_request(request_id: String) {
    loop {
        let choice = dialoguer::Select::new()
            .with_prompt("許可しますか？")
            .items(&["1度だけ許可", "セッション中許可", "拒否"])
            .default(0)
            .interact();

        match choice {
            Ok(0) => {
                ene_core::submit_permission_decision(
                    &request_id,
                    ene_core::PermissionDecision::AllowOnce,
                )
                .ok();
                break;
            }
            Ok(1) => {
                ene_core::submit_permission_decision(
                    &request_id,
                    ene_core::PermissionDecision::AllowSession,
                )
                .ok();
                break;
            }
            Ok(2) => {
                ene_core::submit_permission_decision(
                    &request_id,
                    ene_core::PermissionDecision::Deny,
                )
                .ok();
                break;
            }
            _ => continue,
        }
    }
}
