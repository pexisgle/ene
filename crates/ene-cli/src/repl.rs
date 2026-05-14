use ene_ai_core::{
    spawn_split_task, poll_split_result, run_ai_with_tools, truncate,
};
use crate::{commands, context::AppContext, stream, style};

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
    if !ctx.settings.memory.enabled || !ctx.settings.memory.auto_session_split {
        return;
    }

    if let Some(result) = poll_split_result(&mut ctx.pending_split) {
        match result {
            Ok(result) => {
                println!("\n{}", style::warning(format!("[Session] {} ", result.reason)));
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
                    println!("{}", style::warning(format!("[Session] 重要な事実: {}", facts_str)));
                }
                ctx.session.reset_session();
                ctx.session.session_id = result.new_session_id;
                println!("{}\n", style::warning("[Session] 新しい会話を開始します。"));
            }
            Err(e) => {
                if e.to_string() != "no split needed" {
                    eprintln!("{}", style::error(format!("[Session] 要約生成エラー: {}", e)));
                }
            }
        }
    }

    if let (Some(store), Some(embedder)) =
        (&ctx.session.memory_store, &ctx.session.embedding_provider)
    {
        spawn_split_task(
            &mut ctx.pending_split,
            ctx.session.last_input_embedding.clone(),
            ctx.session.last_message_time,
            ctx.session.current_turn_count,
            input.to_string(),
            ctx.settings.clone(),
            ctx.session.conversation_history.clone(),
            ctx.session.session_id.clone(),
            ctx.session.card_name().to_string(),
            ctx.settings.user_name.clone(),
            store.clone(),
            embedder.clone(),
        );
    }
}

async fn embed_input(ctx: &mut AppContext, input: &str) {
    if let Some(emb) = ctx.session.embedding_provider.clone() {
        match emb.embed_query(input).await {
            Ok(embedding) => {
                ctx.session.set_pending_embedding(embedding.clone());
                ctx.session.set_last_input_embedding(embedding);
            }
            Err(e) => {
                eprintln!("[Embedding] Error: {}", e);
            }
        }
    }
}

async fn process_ai_response(ctx: &mut AppContext, input: &str) {
    ctx.session.record_user_input();
    ctx.session.add_user_message(input);

    match run_ai_with_tools(&ctx.settings, &ctx.session, input, ctx.registry.clone()).await {
        Ok(stream) => {
            stream::process_stream(stream, &mut ctx.session).await;
        }
        Err(err) => {
            println!("[Error] Failed to start stream: {}", err);
        }
    }
}
