use std::io::{BufRead, Write};

use ene_api::{
    ApiClient, ApiError, CreateSessionRequest, HistoryResponse, MessageMode, MessageRequest,
    MessageResponse,
};

/// One-shot text or a stdin REPL.
pub struct ChatOpts<'a> {
    pub target: &'a str,
    pub text: Option<&'a str>,
    pub verbose: bool,
}

/// Terminal (or test) streams for [`run_chat`].
pub struct ChatPorts<In, Out, Prompt> {
    pub input: In,
    pub output: Out,
    pub prompt: Prompt,
}

/// Send `opts.text`, or read turns from `ports.input` until `.quit` / EOF.
///
/// # Errors
///
/// Returns API failures from `ene-core`, or terminal I/O failures.
pub async fn run_chat<In, Out, Prompt>(
    client: &ApiClient,
    opts: &ChatOpts<'_>,
    mut ports: ChatPorts<In, Out, Prompt>,
) -> Result<(), ApiError>
where
    In: BufRead,
    Out: Write,
    Prompt: Write,
{
    let session_id = resolve_chat_target(client, opts.target).await?;
    writeln!(ports.prompt, "session {session_id}")
        .map_err(|err| ApiError::Transport(err.to_string()))?;
    ports
        .prompt
        .flush()
        .map_err(|err| ApiError::Transport(err.to_string()))?;

    if let Some(text) = opts.text {
        send_and_print(
            client,
            &session_id,
            text,
            opts.verbose,
            None,
            &mut ports.output,
        )
        .await?;
        return Ok(());
    }

    let mut last_seq = last_seq(&client.history(&session_id, depth(opts.verbose)).await?);
    loop {
        write!(ports.prompt, "> ").map_err(|err| ApiError::Transport(err.to_string()))?;
        ports
            .prompt
            .flush()
            .map_err(|err| ApiError::Transport(err.to_string()))?;
        let mut line = String::new();
        let n = ports
            .input
            .read_line(&mut line)
            .map_err(|err| ApiError::Transport(err.to_string()))?;
        if n == 0 {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if is_repl_quit(line) {
            break;
        }
        last_seq = send_and_print(
            client,
            &session_id,
            line,
            opts.verbose,
            last_seq,
            &mut ports.output,
        )
        .await?;
    }
    Ok(())
}

async fn resolve_chat_target(client: &ApiClient, target: &str) -> Result<String, ApiError> {
    match client.get_session(target).await {
        Ok(session) => Ok(session.id),
        Err(ApiError::Problem { status: 404, .. }) => open_or_create_for_soul(client, target).await,
        Err(err) => Err(err),
    }
}

async fn open_or_create_for_soul(client: &ApiClient, soul_id: &str) -> Result<String, ApiError> {
    let soul = client.get_soul(soul_id).await?;
    let page = client.list_sessions(Some(&soul.id)).await?;
    if let Some(existing) = page
        .items
        .into_iter()
        .find(|session| session.kind != "delegation" && session.ended_at.is_none())
    {
        return Ok(existing.id);
    }
    let created = client
        .create_session(&CreateSessionRequest {
            soul_id: soul.id,
            title: None,
        })
        .await?;
    Ok(created.id)
}

async fn send_and_print(
    client: &ApiClient,
    session_id: &str,
    text: &str,
    verbose: bool,
    after_seq: Option<u64>,
    out: &mut impl Write,
) -> Result<Option<u64>, ApiError> {
    client
        .send_message(
            session_id,
            &MessageRequest {
                text: text.to_owned(),
                mode: MessageMode::Prompt,
                input_modality: None,
            },
            None,
        )
        .await?;
    let history = client.history(session_id, depth(verbose)).await?;
    print_messages(&history.messages, verbose, after_seq, out)?;
    Ok(last_seq(&history))
}

fn print_messages(
    messages: &[MessageResponse],
    verbose: bool,
    after_seq: Option<u64>,
    out: &mut impl Write,
) -> Result<(), ApiError> {
    for message in messages {
        if after_seq.is_some_and(|seq| message.seq <= seq) {
            continue;
        }
        if !verbose && omit_on_surface(&message.role) {
            continue;
        }
        writeln!(out, "{}: {}", message.role, message.text)
            .map_err(|err| ApiError::Transport(err.to_string()))?;
    }
    out.flush()
        .map_err(|err| ApiError::Transport(err.to_string()))
}

fn last_seq(history: &HistoryResponse) -> Option<u64> {
    history.messages.last().map(|message| message.seq)
}

const fn depth(verbose: bool) -> &'static str {
    if verbose { "detail" } else { "surface" }
}

fn omit_on_surface(role: &str) -> bool {
    matches!(role, "inner" | "thinking" | "tool")
}

fn is_repl_quit(line: &str) -> bool {
    matches!(line, ".quit" | ".exit")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quit_aliases() {
        assert!(is_repl_quit(".quit"));
        assert!(is_repl_quit(".exit"));
        assert!(!is_repl_quit("hello"));
        assert!(!is_repl_quit("quit"));
    }

    #[test]
    fn surface_omits_inner_thinking_and_tool() {
        assert!(omit_on_surface("inner"));
        assert!(omit_on_surface("thinking"));
        assert!(omit_on_surface("tool"));
        assert!(!omit_on_surface("user"));
        assert!(!omit_on_surface("assistant"));
        assert!(!omit_on_surface("system"));
    }

    #[test]
    fn print_messages_skips_prior_seq_and_inner_on_surface() {
        let messages = [
            MessageResponse {
                seq: 1,
                role: "user".into(),
                text: "old".into(),
            },
            MessageResponse {
                seq: 2,
                role: "inner".into(),
                text: "secret".into(),
            },
            MessageResponse {
                seq: 3,
                role: "assistant".into(),
                text: "hi".into(),
            },
        ];
        let mut surface = Vec::new();
        print_messages(&messages, false, Some(1), &mut surface).unwrap();
        assert_eq!(String::from_utf8(surface).unwrap(), "assistant: hi\n");

        let mut detail = Vec::new();
        print_messages(&messages, true, Some(1), &mut detail).unwrap();
        let detail = String::from_utf8(detail).unwrap();
        assert!(detail.contains("inner: secret"));
        assert!(detail.contains("assistant: hi"));
        assert!(!detail.contains("old"));
    }
}
