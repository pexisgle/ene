use crate::{commands, context::AppContext, style};
use ene_core::truncate;

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
                crate::stream::process_stream(stream, &mut ctx.runtime.session).await;
            }
            Err(err) => {
                eprintln!("{}", style::error(format!("Stream start error: {}", err)));
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
