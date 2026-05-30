use crate::{commands, context::AppContext, stream};

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

        // Subscribe before sending the run command to avoid missing events
        let mut rx = ctx.handle.subscribe();
        let _ = ctx.handle.run(&input);
        stream::process_stream(&mut rx, &ctx.handle).await;
    }
}
