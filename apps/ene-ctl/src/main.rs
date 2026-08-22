#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "CLI client writes to the terminal"
)]
#![deny(unsafe_code)]

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use ene_api::{
    ApiClient, CreateSessionRequest, MessageMode, MessageRequest, ResourceKind, SoulSkillsPatch,
};
use ene_ctl::core;
use ene_ctl::session;

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
    /// Manage the ene-core daemon process
    Core {
        #[command(subcommand)]
        op: CoreCmd,
    },
    /// GET /health
    Status,
    /// Text turn (surface by default)
    Chat { session: String, text: String },
    Soul {
        #[command(subcommand)]
        op: SoulCmd,
    },
    Session {
        #[command(subcommand)]
        op: SessionCmd,
    },
    /// List and cancel background tasks (jobs API)
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
enum CoreCmd {
    /// Spawn ene-core and wait until api.json is ready
    Start {
        #[arg(long)]
        data_dir: PathBuf,
        /// Stay attached to the child process
        #[arg(long)]
        foreground: bool,
    },
    /// Stop ene-core using the pid file written by start
    Stop {
        #[arg(long)]
        data_dir: PathBuf,
    },
}

#[derive(Subcommand, Debug)]
enum SoulCmd {
    List,
    Show {
        id: String,
    },
    /// Replace enabled skills. Omit names to allow every installed skill.
    Skills {
        id: String,
        #[arg(num_args = 0..)]
        names: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
enum SessionCmd {
    List,
    Show {
        id: String,
    },
    Create {
        soul_id: String,
    },
    Fork {
        id: String,
    },
    Export {
        id: String,
    },
    Compact {
        id: String,
    },
    /// Search sessions by title (server q= or client-side filter)
    Search {
        query: String,
    },
    /// Split a session at the current turn
    Split {
        id: String,
    },
    /// End a session (explicit; idle timeout is server-side)
    End {
        id: String,
    },
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
    Config { id: String },
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
    let result = if let Cmd::Core { op } = &args.cmd {
        run_core(op).await.map_err(|err| err.to_string())
    } else {
        let client = ApiClient::new(&args.url, &args.token, &args.client_id);
        run_api(&client, &args).await.map_err(|err| err.to_string())
    };
    if let Err(err) = result {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

async fn run_core(op: &CoreCmd) -> Result<(), core::CtlError> {
    match op {
        CoreCmd::Start {
            data_dir,
            foreground,
        } => core::start_core(data_dir, *foreground).await,
        CoreCmd::Stop { data_dir } => core::stop_core(data_dir),
    }
}

async fn run_api(client: &ApiClient, args: &Args) -> Result<(), ene_api::ApiError> {
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
                        input_modality: None,
                    },
                    None,
                )
                .await?;
            if args.verbose {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&sent)
                        .map_err(|err| ene_api::ApiError::Codec(err.to_string()))?
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
        Cmd::Soul { op } => match op {
            SoulCmd::List => print_json(&client.list_souls().await?)?,
            SoulCmd::Show { id } => print_json(&client.get_soul(id).await?)?,
            SoulCmd::Skills { id, names } => print_json(
                &client
                    .patch_soul_skills(
                        id,
                        &SoulSkillsPatch {
                            skill_refs: names.clone(),
                        },
                    )
                    .await?,
            )?,
        },
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
            SessionCmd::Search { query } => {
                print_json(&session::search_sessions(client, query).await?)?;
            }
            SessionCmd::Split { id } => print_json(&session::split_session(client, id).await?)?,
            SessionCmd::End { id } => print_json(
                &client
                    .end_session(
                        id,
                        &ene_api::EndSessionRequest {
                            reason: "explicit".to_owned(),
                        },
                    )
                    .await?,
            )?,
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
                            completed: None,
                            schedule_id: None,
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
            PluginCmd::Config { id } => print_json(&client.plugin_config(id).await?)?,
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
        Cmd::Core { .. } => unreachable!("core commands are handled before run_api"),
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
