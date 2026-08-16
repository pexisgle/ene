//! Routes `Ask` resolutions from the plugin broker hub through the actor:
//! [`EneCommand::BrokerApprovalRequested`] registers the request in the
//! actor's `pending_permissions` map and broadcasts
//! [`EneEvent::BrokerApprovalRequired`] for the desktop dialog. The user's
//! [`PermissionDecision`](crate::streaming::PermissionDecision) resolves the
//! request through the same channel the tool-permission flow uses. Timeouts
//! and dropped receivers fail safe to denial.

use async_trait::async_trait;
use ene_approval::{ApprovalCategory, ResolvedMode};
use ene_plugin_host::ApprovalResponder;
use tokio::sync::{mpsc, oneshot};

use crate::handle::EneCommand;
use crate::streaming::PermissionDecision;

/// The actor-backed approval responder attached to the broker hub.
pub struct ActorApprovalResponder {
    cmd_tx: mpsc::UnboundedSender<EneCommand>,
    timeout: std::time::Duration,
}

impl ActorApprovalResponder {
    #[must_use]
    pub fn new(cmd_tx: mpsc::UnboundedSender<EneCommand>, timeout: std::time::Duration) -> Self {
        Self { cmd_tx, timeout }
    }
}

#[async_trait]
impl ApprovalResponder for ActorApprovalResponder {
    async fn request(
        &self,
        plugin: &str,
        category: ApprovalCategory,
        target: &str,
    ) -> ResolvedMode {
        let request_id = crate::types::RequestId::new(uuid::Uuid::new_v4().to_string());
        let (tx, rx) = oneshot::channel();
        let category_label = format!("{category:?}");
        let description = format!("Plugin '{plugin}' requests {category_label} access to {target}");
        let command = EneCommand::BrokerApprovalRequested {
            request_id: request_id.clone(),
            plugin: plugin.to_string(),
            category: category_label,
            target: target.to_string(),
            description,
            reply: tx,
        };
        if self.cmd_tx.send(command).is_err() {
            return ResolvedMode::Deny;
        }
        match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(PermissionDecision::Deny) | Err(_)) | Err(_) => ResolvedMode::Deny,
            Ok(Ok(_)) => ResolvedMode::Allow,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_approval::ApprovalCategory;
    use tokio::sync::mpsc;

    /// The responder must submit a `BrokerApprovalRequested` command and
    /// resolve the user's decision: `AllowOnce` → allow, `Deny` → deny,
    /// dropped channel → deny.
    #[tokio::test]
    async fn responder_routes_decisions_through_the_mailbox() {
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel();
        let responder = ActorApprovalResponder::new(cmd_tx, std::time::Duration::from_secs(5));
        let responder = std::sync::Arc::new(responder);

        let pending = {
            let responder = std::sync::Arc::clone(&responder);
            tokio::spawn(async move {
                responder
                    .request("fs", ApprovalCategory::FsDelete, "workspace/notes.txt")
                    .await
            })
        };

        let command = cmd_rx
            .recv()
            .await
            .expect("the responder must submit a command");
        let EneCommand::BrokerApprovalRequested {
            request_id,
            plugin,
            category,
            target,
            reply,
            ..
        } = command
        else {
            panic!("unexpected command submitted by the responder");
        };
        assert_eq!(plugin, "fs");
        assert_eq!(category, "FsDelete");
        assert_eq!(target, "workspace/notes.txt");
        assert!(!request_id.as_str().is_empty());
        #[expect(
            clippy::let_underscore_must_use,
            reason = "oneshot send error is Copy; drop() would trip dropping_copy_types"
        )]
        let _ = reply.send(PermissionDecision::AllowOnce);

        assert_eq!(pending.await.expect("responder task"), ResolvedMode::Allow);

        let pending = {
            let responder = std::sync::Arc::clone(&responder);
            tokio::spawn(async move {
                responder
                    .request("web", ApprovalCategory::DynamicHttps, "https://x.test")
                    .await
            })
        };
        let command = cmd_rx.recv().await.expect("second command");
        let EneCommand::BrokerApprovalRequested { reply, .. } = command else {
            panic!("unexpected command submitted by the responder");
        };
        #[expect(
            clippy::let_underscore_must_use,
            reason = "oneshot send error is Copy; drop() would trip dropping_copy_types"
        )]
        let _ = reply.send(PermissionDecision::Deny);
        assert_eq!(pending.await.expect("responder task"), ResolvedMode::Deny);

        // Dropped decision channel (timeout equivalent) fails safe to deny.
        let pending = {
            let responder = std::sync::Arc::clone(&responder);
            tokio::spawn(async move {
                responder
                    .request("fs", ApprovalCategory::FsRead, "workspace/a.txt")
                    .await
            })
        };
        let command = cmd_rx.recv().await.expect("third command");
        let EneCommand::BrokerApprovalRequested { reply, .. } = command else {
            panic!("unexpected command submitted by the responder");
        };
        drop(reply);
        assert_eq!(pending.await.expect("responder task"), ResolvedMode::Deny);
    }
}
