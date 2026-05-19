use crate::{context::AppContext, style};
use ene_ai_core::{SplitReason, execute_split, truncate};

pub async fn execute(arg: &str, ctx: &mut AppContext) {
    let parts: Vec<&str> = arg.splitn(2, ' ').collect();
    let subcmd = parts.first().copied().unwrap_or("");

    match subcmd {
        "info" => handle_info(ctx),
        "split" => handle_split(ctx).await,
        "summaries" => handle_summaries(ctx),
        _ => {
            println!("Usage: /session <info|split|summaries>");
        }
    }
}

fn handle_info(ctx: &AppContext) {
    println!("--- Session Info ---");
    println!("Session ID: {}", ctx.session.session_id);
    println!(
        "Started: {}",
        ctx.session
            .session_started_at
            .format("%Y-%m-%d %H:%M:%S UTC")
    );
    println!("Elapsed: {} min", ctx.session.session_elapsed_minutes());
    println!("Turn count: {}", ctx.session.current_turn_count);
    println!(
        "History messages: {}",
        ctx.session.conversation_history.len()
    );
    println!("Auto-split: {}", ctx.settings.memory.auto_session_split);
    println!(
        "Timeout: {} min",
        ctx.settings.memory.session_timeout_minutes
    );
    println!(
        "Topic threshold: {}",
        ctx.settings.memory.topic_change_threshold
    );
    println!("--------------------");
}

async fn handle_split(ctx: &mut AppContext) {
    if ctx.session.conversation_history.is_empty() {
        println!(
            "{}",
            style::warning("[Session] 会話履歴がないため分割できません。")
        );
        return;
    }
    let Some(store) = &ctx.session.memory_store else {
        println!(
            "{}",
            style::warning("[Session] メモリが有効ではありません。")
        );
        return;
    };
    let Some(embedder) = &ctx.session.embedding_provider else {
        println!(
            "{}",
            style::warning("[Session] Embedding プロバイダーが利用できません。")
        );
        return;
    };
    println!(
        "{}",
        style::header("[Session] 手動でセッションを分割しています...")
    );
    let reason = SplitReason::Manual;
    match execute_split(
        &ctx.session.conversation_history,
        &ctx.session.session_id,
        ctx.session.card_name(),
        &ctx.settings.user_name,
        store,
        embedder,
        &ctx.settings,
        reason,
    )
    .await
    {
        Ok(result) => {
            println!(
                "{}",
                style::warning(format!(
                    "[Session] 要約: {}",
                    truncate(&result.summary, 120)
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
                    "  {}",
                    style::warning(format!("[Session] 重要な事実: {}", facts_str))
                );
            }
            ctx.session.reset_session();
            ctx.session.session_id = result.new_session_id;
            println!("{}", style::warning("[Session] 新しい会話を開始しました。"));
        }
        Err(e) => {
            println!("{}", style::error(format!("[Session] 分割エラー: {}", e)));
        }
    }
}

fn handle_summaries(ctx: &AppContext) {
    let Some(store) = &ctx.session.memory_store else {
        println!(
            "{}",
            style::warning("[Session] メモリが有効ではありません。")
        );
        return;
    };
    let card_name = ctx.session.card_name();
    match store.list_recent_summaries(card_name, 10) {
        Ok(summaries) => {
            if summaries.is_empty() {
                println!("[Session] 保存された会話要約はありません。");
            } else {
                println!("--- Past Conversation Summaries ({}) ---", summaries.len());
                for s in &summaries {
                    println!(
                        "  {} | {}",
                        s.ended_at.format("%Y-%m-%d %H:%M"),
                        truncate(&s.summary, 80),
                    );
                }
                println!("----------------------------------------");
            }
        }
        Err(e) => println!("[Session] Error: {}", e),
    }
}
