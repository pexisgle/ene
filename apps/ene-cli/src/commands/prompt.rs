use crate::commands::{CliCommand, CliError, CommandOutcome};
use crate::context::AppContext;
use async_trait::async_trait;
use ene_ai::{LlmMessage, UserMessagePart};
use ene_mind::MindConfig;
use ene_runtime::MessageBuildContext;

pub struct PromptCommand;

/// Placeholder shown in the final user slot, since `/prompt` previews the
/// packet without an actual user message to send.
const USER_INPUT_PLACEHOLDER: &str = "(your next message)";

/// Renders one [`LlmMessage`] as a labelled, human-readable block.
///
/// The label mirrors the wire role so the output can be compared 1:1 with
/// what `build_messages` actually hands to the provider.
fn render_message(index: usize, message: &LlmMessage) {
    match message {
        LlmMessage::System { content } => {
            println!("--- [{index}] system ---");
            println!("{content}");
        }
        LlmMessage::User { parts } => {
            println!("--- [{index}] user ---");
            for part in parts {
                match part {
                    UserMessagePart::Text { text } => println!("{text}"),
                    UserMessagePart::Image { .. } => println!("[image attachment]"),
                }
            }
        }
        LlmMessage::Assistant {
            content,
            tool_calls,
        } => {
            println!("--- [{index}] assistant ---");
            if let Some(text) = content {
                println!("{text}");
            }
            if let Some(calls) = tool_calls {
                for call in calls {
                    println!("[tool call] {} {}", call.name, call.arguments);
                }
            }
        }
        LlmMessage::Tool {
            tool_call_id,
            content,
        } => {
            println!("--- [{index}] tool ({tool_call_id}) ---");
            println!("{content}");
        }
    }
    println!("-------------------------");
}

#[async_trait]
impl CliCommand for PromptCommand {
    fn name(&self) -> &'static str {
        "/prompt"
    }

    fn description(&self) -> &'static str {
        "Preview the exact message list sent to the AI (rendered from build_messages)"
    }

    fn usage(&self) -> &'static str {
        "/prompt"
    }

    async fn execute(&self, _arg: &str, ctx: &mut AppContext) -> Result<CommandOutcome, CliError> {
        let snapshot = ctx
            .handle
            .diagnostics()
            .get_snapshot()
            .await
            .map_err(|e| CliError::ActorError(format!("Failed to get actor state: {e}")))?;

        let Some(card) = &snapshot.character_card else {
            println!("No character card loaded.");
            return Ok(CommandOutcome::Continue);
        };

        let lang = crate::i18n::loader()
            .current_languages()
            .first()
            .map_or_else(|| "en".to_string(), |l| l.language.as_str().to_string());
        let prompts = ene_config::PromptLibrary::load(&lang);
        let mind = snapshot
            .config
            .get_section::<MindConfig>()
            .unwrap_or_default();

        // Delegate to the exact builder the runtime uses so this view cannot
        // drift from the real packet. The example-message condition,
        // the emotion-aware output contract, and the message ordering all come
        // from `build_messages` rather than being re-implemented here.
        let build_ctx = MessageBuildContext {
            card,
            user_input: USER_INPUT_PLACEHOLDER,
            history: &snapshot.history,
            runtime_context: None,
            runtime_rules: &snapshot.config.runtime_rules,
            user_name: &snapshot.config.user_name,
            prompts: &prompts,
            emotion_enabled: mind.emotion.enabled,
        };

        let messages = ene_runtime::build_messages(&build_ctx)
            .map_err(|e| CliError::ExecutionFailed(format!("Failed to build prompt: {e}")))?;

        println!(
            "Prompt preview ({} messages, exactly as built by build_messages):",
            messages.len()
        );
        for (index, message) in messages.iter().enumerate() {
            render_message(index + 1, message);
        }

        Ok(CommandOutcome::Continue)
    }
}
