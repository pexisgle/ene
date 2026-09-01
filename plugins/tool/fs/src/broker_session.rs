use ene_plugin_ipc::{
    BrokerClient, BrokerClientTransport, BrokerRequest, BrokerResponse, BrokerSession,
};
use ene_tool_registry::arg_str;
use serde_json::{Value, json};
use std::sync::OnceLock;
use tokio::sync::Mutex as AsyncMutex;

static BROKER_SESSION: OnceLock<BrokerSession<BrokerClientTransport>> = OnceLock::new();
static BROKER_INIT: OnceLock<AsyncMutex<()>> = OnceLock::new();

pub(crate) fn execute(name: &str, args: &Value) -> Result<Value, String> {
    if name == "fs.search" && std::env::var_os("ENE_BROKER_SOCKET").is_some() {
        return broker_search(args);
    }
    super::logic::execute(name, args)
}

fn broker_search(args: &Value) -> Result<Value, String> {
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(broker_search_async(args))
    })
}

async fn broker_search_async(args: &Value) -> Result<Value, String> {
    let session = broker_session().await?;
    let token = std::env::var("ENE_PLUGIN_SPAWN_TOKEN")
        .map_err(|_| "ENE_PLUGIN_SPAWN_TOKEN is not set".to_owned())?;
    let query = arg_str(args, "query")?;
    let path = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let request = BrokerRequest::FsSearch {
        path: path.to_owned(),
        query: query.to_owned(),
        regex: args.get("regex").and_then(Value::as_bool).unwrap_or(false),
        case_insensitive: args
            .get("case_insensitive")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        include: args
            .get("include")
            .and_then(Value::as_str)
            .map(str::to_owned),
        context_lines: u32::try_from(
            args.get("context_lines")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        )
        .unwrap_or(0)
        .min(10),
        count: args.get("count").and_then(Value::as_bool).unwrap_or(false),
        max: u32::try_from(args.get("max").and_then(Value::as_u64).unwrap_or(50))
            .unwrap_or(50)
            .min(200),
    };

    match session.call(&token, request).await {
        Ok(BrokerResponse::FsSearchOk { matches }) => Ok(json!({ "matches": matches })),
        Ok(BrokerResponse::Error { message, .. }) => Err(message),
        Err(err) => Err(err.to_string()),
        Ok(_) => Err("unexpected broker response".to_owned()),
    }
}

async fn broker_session() -> Result<&'static BrokerSession<BrokerClientTransport>, String> {
    if let Some(session) = BROKER_SESSION.get() {
        return Ok(session);
    }
    let init = BROKER_INIT.get_or_init(AsyncMutex::default);
    let _guard = init.lock().await;
    if let Some(session) = BROKER_SESSION.get() {
        return Ok(session);
    }
    let client = BrokerClient::from_env()
        .await
        .map_err(|err| format!("broker unavailable: {err}"))?;
    Ok(BROKER_SESSION.get_or_init(|| BrokerSession::new(client)))
}
