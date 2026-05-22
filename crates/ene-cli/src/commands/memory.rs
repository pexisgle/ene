use crate::{context::AppContext, style};
use ene_ai_core::truncate;

pub async fn execute(arg: &str, ctx: &mut AppContext) {
    let parts: Vec<&str> = arg.splitn(2, ' ').collect();
    let subcmd = parts.first().copied().unwrap_or("");

    match subcmd {
        "search" => handle_search(parts.get(1).copied().unwrap_or(""), ctx).await,
        "list" => handle_list(ctx),
        _ => {
            println!("Usage: /memory <search|list>");
            println!("  search <query> - Search memories by similarity");
            println!("  list           - List all stored memories");
        }
    }
}

async fn handle_search(query: &str, ctx: &AppContext) {
    if query.is_empty() {
        println!(
            "{}",
            style::warning("[Memory] Usage: /memory search <query>")
        );
        return;
    }
    let Some(store) = &ctx.session.memory.memory_store else {
        println!(
            "{}",
            style::warning("[Memory] メモリが有効ではありません。")
        );
        return;
    };
    let Some(embedder) = &ctx.session.memory.embedding_provider else {
        println!(
            "{}",
            style::warning("[Memory] Embedding プロバイダーが利用できません。")
        );
        return;
    };
    println!(
        "{}",
        style::header(format!("[Memory] クエリを検索中: {}", query))
    );
    match embedder.embed_query(query).await {
        Ok(embedding) => {
            let card_name = ctx.session.card_name();
            let threshold = 0.0f32;
            match store.search_summaries(&embedding, card_name, 10, threshold) {
                Ok(results) => {
                    if results.is_empty() {
                        println!(
                            "{}",
                            style::warning("[Memory] ヒットするmemoryはありませんでした。")
                        );
                    } else {
                        println!(
                            "{}",
                            style::success(format!(
                                "[Memory] {} 件のmemoryがヒットしました:",
                                results.len()
                            ))
                        );
                        for (i, recalled) in results.iter().enumerate() {
                            println!(
                                "\n--- Memory #{} (similarity: {:.4}) ---",
                                i + 1,
                                recalled.similarity
                            );
                            println!("  Session ID: {}", recalled.entry.session_id);
                            println!(
                                "  Date: {}",
                                recalled.entry.ended_at.format("%Y-%m-%d %H:%M")
                            );
                            println!("  Summary: {}", recalled.entry.summary);
                        }
                    }
                }
                Err(e) => println!("{}", style::error(format!("[Memory] 検索エラー: {}", e))),
            }
        }
        Err(e) => println!(
            "{}",
            style::error(format!("[Memory] Embeddingエラー: {}", e))
        ),
    }
}

fn handle_list(ctx: &AppContext) {
    let Some(store) = &ctx.session.memory.memory_store else {
        println!(
            "{}",
            style::warning("[Memory] メモリが有効ではありません。")
        );
        return;
    };
    let card_name = ctx.session.card_name();
    match store.list_recent_summaries(card_name, 50) {
        Ok(summaries) => {
            if summaries.is_empty() {
                println!("[Memory] 保存された会話要約はありません。");
            } else {
                println!("--- Stored Summaries ({}) ---", summaries.len());
                for s in &summaries {
                    println!(
                        "  {} | {} | {}",
                        s.ended_at.format("%Y-%m-%d %H:%M"),
                        s.session_id,
                        truncate(&s.summary, 80)
                    );
                }
                println!("----------------------------------------");
            }
        }
        Err(e) => println!("[Memory] Error: {}", e),
    }
    if let Ok(facts) = store.get_all_keyfacts(card_name) {
        if !facts.is_empty() {
            println!("\n--- Key Facts ({}) ---", facts.len());
            for f in &facts {
                println!("  {}: {}", f.key, f.value);
            }
            println!("------------------------");
        }
    }
}
