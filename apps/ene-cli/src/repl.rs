use crate::{commands, context::AppContext, stream, style};
use ene_core::{MemoryConfig, SessionConfig, poll_split_result, run_ene_with_tools, truncate};

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

        check_session_split(ctx, &input).await;
        embed_input(ctx, &input).await;
        process_ai_response(ctx, &input).await;
    }
}
async fn check_session_split(ctx: &mut AppContext, input: &str) {
    let mem_config = ctx
        .settings
        .get_section::<MemoryConfig>("memory")
        .unwrap_or_default();
    let session_config = ctx
        .settings
        .get_section::<SessionConfig>("session")
        .unwrap_or_default();
    if !mem_config.enabled || !session_config.auto_session_split {
        return;
    }

    if let Some(result) = poll_split_result(&mut ctx.pending_split) {
        match result {
            Ok(result) => {
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
                ctx.session.reset_session();
                ctx.session.memory.session_id = result.new_session_id;
                println!("{}\n", style::warning("[Session] 新しい会話を開始します。"));
            }
            Err(e) => {
                if !matches!(e, ene_core::SessionError::SplitNotNeeded) {
                    eprintln!(
                        "{}",
                        style::error(format!("[Session] 要約生成エラー: {}", e))
                    );
                }
            }
        }
    }

    let user_name = ctx.settings.user_name.clone();
    ctx.runtime.check_and_perform_split(input, &user_name);
}

async fn embed_input(ctx: &mut AppContext, input: &str) {
    if let Err(e) = ctx.runtime.embed_input(input).await {
        eprintln!("[Embedding] Error: {}", e);
    }
}

async fn process_ai_response(ctx: &mut AppContext, input: &str) {
    ctx.session.record_user_input();
    ctx.session.add_user_message(input);

    match run_ene_with_tools(&ctx.settings, &ctx.session, input, ctx.registry.clone()).await {
        Ok(stream) => {
            stream::process_stream(stream, &mut ctx.session).await;
        }
        Err(err) => {
            println!("[Error] Failed to start stream: {}", err);
        }
    }
}
