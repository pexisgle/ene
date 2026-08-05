//! Host mediation for plugin-to-plugin capability calls.
//!
//! A consumer plugin opens the host-service `capability` passenger and sends
//! [`CapabilityCall`]s; this module authenticates each call against the
//! consumer's declared `requires`, resolves the provider from the post-gate
//! capability registry, and forwards the call over the provider's plugin IPC
//! connection — so the provider's supervision, timeout, and circuit-breaker
//! machinery all apply to mediated calls.

use std::sync::Arc;

use async_trait::async_trait;
use ene_plugin_proto::{
    CapabilityCall, CapabilityCallError, CapabilityCallErrorCode, CapabilityRef,
    CapabilityServiceHandler, CapabilityServiceRequest, CapabilityServiceResponse, IpcStream,
    read_capability_service_request, write_capability_service_response,
};

use crate::capability_registry::CapabilityRegistry;
use crate::manager::PluginHostManager;

/// Resolves the provider for one capability call, enforcing the mediation ACL.
///
/// The caller (`consumer`, derived from the session's auth token) must have
/// declared a `requires` entry matching the requested capability's name and
/// major — hard and soft requirements both authorize, since both are
/// declarations of intent. The requirement is then resolved through the
/// registry, which is the post-gate registry: a provider disabled by the
/// startup fixpoint never satisfies a mediated call.
///
/// Returns the provider plugin name.
pub fn resolve_capability_provider<'a>(
    registry: &'a CapabilityRegistry,
    consumer: &str,
    call: &CapabilityCall,
) -> Result<&'a str, CapabilityCallError> {
    let requested = CapabilityRef::parse(call.capability.as_str()).map_err(|e| {
        CapabilityCallError::new(
            CapabilityCallErrorCode::InvalidRequest,
            format!("malformed capability reference: {e}"),
        )
    })?;
    let declarations = registry.declarations(consumer).ok_or_else(|| {
        CapabilityCallError::new(
            CapabilityCallErrorCode::Forbidden,
            format!("plugin {consumer} declared no capability requirements"),
        )
    })?;
    let Some(requirement) = declarations.requires.iter().find(|requirement| {
        requirement.name() == requested.name() && requirement.major() == requested.major()
    }) else {
        return Err(CapabilityCallError::new(
            CapabilityCallErrorCode::Forbidden,
            format!("plugin {consumer} did not declare a requirement for {requested}"),
        ));
    };
    registry.resolve(requirement).ok_or_else(|| {
        CapabilityCallError::new(
            CapabilityCallErrorCode::NoProvider,
            format!("no provider registered for {requested}"),
        )
    })
}

/// Executes one authenticated capability call on behalf of a consumer.
#[async_trait]
pub trait CapabilityCallHandler: Send + Sync {
    /// Resolves and executes `call` for `consumer`, returning the provider's
    /// JSON result or a typed capability error.
    async fn call(
        &self,
        consumer: &str,
        call: CapabilityCall,
    ) -> Result<serde_json::Value, CapabilityCallError>;
}

/// The runtime handler: resolves through the live plugin host and forwards
/// over the provider's connection.
///
/// The host mutex is held only for resolution and connection lookup, never
/// across the provider round trip, so a slow provider call cannot stall the
/// actor's access to the manager.
pub struct ManagerCapabilityHandler {
    host: Arc<tokio::sync::Mutex<Option<PluginHostManager>>>,
}

impl ManagerCapabilityHandler {
    /// Wraps the shared plugin-host handle the actor owns.
    #[must_use]
    pub fn new(host: Arc<tokio::sync::Mutex<Option<PluginHostManager>>>) -> Self {
        Self { host }
    }
}

#[async_trait]
impl CapabilityCallHandler for ManagerCapabilityHandler {
    async fn call(
        &self,
        consumer: &str,
        call: CapabilityCall,
    ) -> Result<serde_json::Value, CapabilityCallError> {
        let guard = self.host.lock().await;
        let Some(manager) = &*guard else {
            return Err(CapabilityCallError::new(
                CapabilityCallErrorCode::Internal,
                "plugin host is not running",
            ));
        };
        let provider = resolve_capability_provider(manager.capability_registry(), consumer, &call)?;
        let connection = manager.connection(provider).ok_or_else(|| {
            CapabilityCallError::new(
                CapabilityCallErrorCode::Internal,
                format!("provider plugin {provider} is not connected"),
            )
        })?;
        if !connection.capabilities().supports_capability_calls {
            return Err(CapabilityCallError::new(
                CapabilityCallErrorCode::NotSupported,
                format!("provider plugin {provider} predates capability calls"),
            ));
        }
        drop(guard);
        connection.call_capability(&call).await
    }
}

/// Session server for the host-service `capability` passenger.
///
/// Serves one authenticated session: read a call, execute it through the
/// handler, write the response. A failed call is one response and the session
/// continues; only EOF or an I/O error ends it.
pub struct CapabilityMediator {
    handler: Arc<dyn CapabilityCallHandler>,
}

impl CapabilityMediator {
    /// Creates a mediator backed by the live plugin host.
    #[must_use]
    pub fn new(host: Arc<tokio::sync::Mutex<Option<PluginHostManager>>>) -> Self {
        Self {
            handler: Arc::new(ManagerCapabilityHandler::new(host)),
        }
    }

    /// Creates a mediator over an explicit handler (tests, alternative hosts).
    #[must_use]
    pub fn with_handler(handler: Arc<dyn CapabilityCallHandler>) -> Self {
        Self { handler }
    }
}

#[async_trait]
impl CapabilityServiceHandler for CapabilityMediator {
    async fn serve(&self, stream: IpcStream, consumer: String) -> std::io::Result<()> {
        serve_capability_session(stream, consumer, Arc::clone(&self.handler)).await
    }
}

async fn serve_capability_session(
    mut stream: IpcStream,
    consumer: String,
    handler: Arc<dyn CapabilityCallHandler>,
) -> std::io::Result<()> {
    loop {
        let Some(request) = read_capability_service_request(&mut stream).await? else {
            return Ok(());
        };
        let CapabilityServiceRequest::Call { call } = request;
        let result = handler.call(&consumer, call).await;
        write_capability_service_response(
            &mut stream,
            &CapabilityServiceResponse::Result { result },
        )
        .await?;
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "tests use unwrap for concise assertions"
)]
mod tests {
    use super::*;
    use ene_plugin_proto::{CapabilityRef, CapabilityRequirement, PluginCapabilities};

    fn caps(provides: &[&str], requires: &[&str]) -> PluginCapabilities {
        PluginCapabilities {
            provides: provides
                .iter()
                .map(|raw| CapabilityRef::parse(raw).unwrap())
                .collect(),
            requires: requires
                .iter()
                .map(|raw| CapabilityRequirement::parse(raw).unwrap())
                .collect(),
            ..PluginCapabilities::default()
        }
    }

    fn call(capability: &str, method: &str) -> CapabilityCall {
        CapabilityCall {
            capability: CapabilityRef::parse(capability).unwrap(),
            method: method.to_string(),
            payload: serde_json::json!({ "model": "stories260K" }),
        }
    }

    fn registry_with(declarations: &[(&str, PluginCapabilities)]) -> CapabilityRegistry {
        let mut registry = CapabilityRegistry::new();
        for (plugin, capabilities) in declarations {
            registry.register(plugin, capabilities);
        }
        registry
    }

    #[test]
    fn resolves_provider_for_hard_requirement() {
        let registry = registry_with(&[
            ("local-llm", caps(&["gguf-runner@1"], &[])),
            ("consumer", caps(&[], &["gguf-runner@^1"])),
        ]);
        assert_eq!(
            resolve_capability_provider(&registry, "consumer", &call("gguf-runner@1", "generate"))
                .unwrap(),
            "local-llm"
        );
    }

    #[test]
    fn soft_requirement_authorizes() {
        let registry = registry_with(&[
            ("local-llm", caps(&["gguf-runner@1"], &[])),
            ("consumer", caps(&[], &["gguf-runner@^1?"])),
        ]);
        assert_eq!(
            resolve_capability_provider(&registry, "consumer", &call("gguf-runner@1", "embed"))
                .unwrap(),
            "local-llm"
        );
    }

    #[test]
    fn consumer_without_declaration_is_forbidden() {
        let registry = registry_with(&[("local-llm", caps(&["gguf-runner@1"], &[]))]);
        let error =
            resolve_capability_provider(&registry, "ghost", &call("gguf-runner@1", "generate"))
                .unwrap_err();
        assert_eq!(error.code, CapabilityCallErrorCode::Forbidden);
    }

    #[test]
    fn mismatched_declaration_is_forbidden() {
        let registry = registry_with(&[
            ("local-llm", caps(&["gguf-runner@1"], &[])),
            ("consumer", caps(&[], &["embed@^1"])),
        ]);
        let error =
            resolve_capability_provider(&registry, "consumer", &call("gguf-runner@1", "generate"))
                .unwrap_err();
        assert_eq!(error.code, CapabilityCallErrorCode::Forbidden);
    }

    #[test]
    fn major_mismatch_is_forbidden() {
        let registry = registry_with(&[
            ("local-llm", caps(&["gguf-runner@1"], &[])),
            ("consumer", caps(&[], &["gguf-runner@^2"])),
        ]);
        let error =
            resolve_capability_provider(&registry, "consumer", &call("gguf-runner@1", "generate"))
                .unwrap_err();
        assert_eq!(error.code, CapabilityCallErrorCode::Forbidden);
    }

    #[test]
    fn malformed_capability_is_invalid_request() {
        let registry = registry_with(&[
            ("local-llm", caps(&["gguf-runner@1"], &[])),
            ("consumer", caps(&[], &["gguf-runner@^1"])),
        ]);
        let malformed = CapabilityCall {
            capability: CapabilityRef::parse("gguf-runner@1").unwrap(),
            method: "generate".into(),
            payload: serde_json::json!({}),
        };
        let malformed = CapabilityCall {
            capability: serde_json::from_str("\"not a capability\"").unwrap(),
            ..malformed
        };
        let error = resolve_capability_provider(&registry, "consumer", &malformed).unwrap_err();
        assert_eq!(error.code, CapabilityCallErrorCode::InvalidRequest);
    }

    #[test]
    fn missing_provider_is_no_provider() {
        let registry = registry_with(&[("consumer", caps(&[], &["gguf-runner@^1"]))]);
        let error =
            resolve_capability_provider(&registry, "consumer", &call("gguf-runner@1", "generate"))
                .unwrap_err();
        assert_eq!(error.code, CapabilityCallErrorCode::NoProvider);
    }

    #[test]
    fn self_declared_capability_resolves_to_self() {
        let registry =
            registry_with(&[("local-llm", caps(&["gguf-runner@1"], &["gguf-runner@^1"]))]);
        assert_eq!(
            resolve_capability_provider(&registry, "local-llm", &call("gguf-runner@1", "unload"))
                .unwrap(),
            "local-llm"
        );
    }
}
