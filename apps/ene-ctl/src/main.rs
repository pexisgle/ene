#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "CLI client writes to the terminal"
)]
#![deny(unsafe_code)]

use clap::{Parser, Subcommand};
use ene_api::{ApiClient, CreateSessionRequest, MessageMode, MessageRequest, ResourceKind};

#[derive(Parser, Debug)]
#[command(name = "ene-ctl", about = "Talk to ene-core over the public HTTP API")]
struct Args {
    /// Base URL (`http://127.0.0.1:8080`)
    #[arg(long, env = "ENE_API_URL", default_value = "http://127.0.0.1:0")]
    url: String,
    /// Bearer token (or contents of api.token)
    #[arg(long, env = "ENE_API_TOKEN", default_value = "")]
    token: String,
    /// Client id used for exclusive resources
    #[arg(long, default_value = "cli")]
    client_id: String,
    /// Show inner / thinking (detail depth)
    #[arg(long)]
    verbose: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// GET /health
    Status,
    /// Text turn (surface by default)
    Chat { session: String, text: String },
    Session {
        #[command(subcommand)]
        op: SessionCmd,
    },
    Task {
        #[command(subcommand)]
        op: TaskCmd,
    },
    Memory {
        #[command(subcommand)]
        op: MemoryCmd,
    },
    Schedule {
        #[command(subcommand)]
        op: ScheduleCmd,
    },
    Tool {
        #[command(subcommand)]
        op: ToolCmd,
    },
    Plugin {
        #[command(subcommand)]
        op: PluginCmd,
    },
    Usage {
        #[arg(long)]
        session: Option<String>,
    },
    Debug {
        #[command(subcommand)]
        op: DebugCmd,
    },
    Exclusive {
        #[command(subcommand)]
        op: ExclusiveCmd,
    },
}

#[derive(Subcommand, Debug)]
enum SessionCmd {
    List,
    Show { id: String },
    Create { soul_id: String },
    Fork { id: String },
    Export { id: String },
    Compact { id: String },
}

#[derive(Subcommand, Debug)]
enum TaskCmd {
    List,
    Cancel { id: String },
}

#[derive(Subcommand, Debug)]
enum MemoryCmd {
    List { soul: String },
    Edit { id: String, content: String },
    Delete { id: String },
}

#[derive(Subcommand, Debug)]
enum ScheduleCmd {
    List,
}

#[derive(Subcommand, Debug)]
enum ToolCmd {
    List,
}

#[derive(Subcommand, Debug)]
enum PluginCmd {
    List,
    Restart { id: String },
}

#[derive(Subcommand, Debug)]
enum DebugCmd {
    Log { session: String },
    Spans,
}

#[derive(Subcommand, Debug)]
enum ExclusiveCmd {
    Show,
    Claim { resource: String },
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let client = ApiClient::new(&args.url, &args.token, &args.client_id);
    if let Err(err) = run(&client, &args).await {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

async fn run(client: &ApiClient, args: &Args) -> Result<(), ene_api::ApiError> {
    match &args.cmd {
        Cmd::Status => {
            let health = client.health().await?;
            println!("{} {}", health.status, health.bind);
        }
        Cmd::Chat { session, text } => {
            let sent = client
                .send_message(
                    session,
                    &MessageRequest {
                        text: text.clone(),
                        mode: MessageMode::Prompt,
                    },
                    None,
                )
                .await?;
            if args.verbose {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&sent).unwrap_or_default()
                );
            }
            let depth = if args.verbose { "detail" } else { "surface" };
            let history = client.history(session, depth).await?;
            for message in history.messages {
                if !args.verbose && message.role == "inner" {
                    continue;
                }
                println!("{}: {}", message.role, message.text);
            }
        }
        Cmd::Session { op } => match op {
            SessionCmd::List => print_json(&client.list_sessions(None).await?)?,
            SessionCmd::Show { id } => print_json(&client.get_session(id).await?)?,
            SessionCmd::Create { soul_id } => print_json(
                &client
                    .create_session(&CreateSessionRequest {
                        soul_id: soul_id.clone(),
                        title: None,
                    })
                    .await?,
            )?,
            SessionCmd::Fork { id } => print_json(&client.fork_session(id).await?)?,
            SessionCmd::Export { id } => print_json(&client.export_session(id).await?)?,
            SessionCmd::Compact { id } => print_json(&client.compact(id).await?)?,
        },
        Cmd::Task { op } => match op {
            TaskCmd::List => print_json(&client.list_jobs(None).await?)?,
            TaskCmd::Cancel { id } => print_json(&client.cancel_job(id).await?)?,
        },
        Cmd::Memory { op } => match op {
            MemoryCmd::List { soul } => print_json(&client.list_memories(soul, None).await?)?,
            MemoryCmd::Edit { id, content } => print_json(
                &client
                    .patch_memory(
                        id,
                        &ene_api::MemoryPatch {
                            content: Some(content.clone()),
                            scope: None,
                        },
                    )
                    .await?,
            )?,
            MemoryCmd::Delete { id } => client.delete_memory(id).await?,
        },
        Cmd::Schedule { op } => match op {
            ScheduleCmd::List => print_json(&client.list_schedules().await?)?,
        },
        Cmd::Tool { op } => match op {
            ToolCmd::List => print_json(&client.list_tools().await?)?,
        },
        Cmd::Plugin { op } => match op {
            PluginCmd::List => print_json(&client.list_plugins().await?)?,
            PluginCmd::Restart { id } => print_json(&client.restart_plugin(id).await?)?,
        },
        Cmd::Usage { session } => print_json(&client.usage(session.as_deref()).await?)?,
        Cmd::Debug { op } => match op {
            DebugCmd::Log { session } => print_json(&client.history(session, "detail").await?)?,
            DebugCmd::Spans => print_json(&client.diag_spans().await?)?,
        },
        Cmd::Exclusive { op } => match op {
            ExclusiveCmd::Show => print_json(&client.exclusive().await?)?,
            ExclusiveCmd::Claim { resource } => {
                let kind = ResourceKind::parse(resource).ok_or_else(|| {
                    ene_api::ApiError::Codec(format!("unknown resource {resource}"))
                })?;
                print_json(
                    &client
                        .claim_resource(
                            kind,
                            &ene_api::ClaimResourceRequest {
                                client_id: args.client_id.clone(),
                            },
                        )
                        .await?,
                )?;
            }
        },
    }
    Ok(())
}

fn print_json<T: serde::Serialize>(value: &T) -> Result<(), ene_api::ApiError> {
    println!(
        "{}",
        serde_json::to_string_pretty(value)
            .map_err(|err| ene_api::ApiError::Codec(err.to_string()))?
    );
    Ok(())
}
