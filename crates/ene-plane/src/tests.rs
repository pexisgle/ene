use crate::{
    AiJudgement, ApprovalMode, ApprovalPlane, ApprovalSettings, AuditLog, AuthzRequest,
    PolicyDecision, PolicyFile, PolicyRule, PopupDecision, Risk, ScriptedAi, ScriptedPopup,
    Sensitivity, Vault,
};
use std::sync::Arc;
use tempfile::TempDir;

fn req(tool: &str, se: &[&str], sensitivity: Sensitivity, in_workspace: bool) -> AuthzRequest {
    AuthzRequest {
        tool: tool.to_owned(),
        side_effects: se.iter().map(|s| (*s).to_owned()).collect(),
        sensitivity,
        target: "/workspace/a.txt".to_owned(),
        in_workspace,
    }
}

fn make_plane(
    popup: Arc<ScriptedPopup>,
    ai: Option<Arc<dyn crate::ApproveModel>>,
) -> (TempDir, ApprovalPlane) {
    let dir = TempDir::new().unwrap();
    let audit = AuditLog::open(dir.path().join("audit.db")).unwrap();
    let plane = ApprovalPlane::new(ApprovalSettings::default(), audit, popup, ai);
    (dir, plane)
}

#[tokio::test]
async fn policy_allows_workspace_write() {
    let (_dir, plane) = make_plane(ScriptedPopup::deny_all(), None);
    plane.set_policy(PolicyFile {
        rules: vec![PolicyRule {
            tool: "fs.*".to_owned(),
            scope: Some("workspace".to_owned()),
            decision: PolicyDecision::Allow,
        }],
    });
    plane
        .authorize(&req("fs.write", &["fs.write"], Sensitivity::None, true))
        .await
        .unwrap();
}

#[tokio::test]
async fn screenshot_with_empty_side_effects_still_asks() {
    let popup = Arc::new(ScriptedPopup::new([PopupDecision::Deny]));
    let (_dir, plane) = make_plane(popup, None);
    let err = plane
        .authorize(&req("app.screenshot", &[], Sensitivity::High, true))
        .await
        .unwrap_err();
    assert!(matches!(err, crate::PlaneError::Denied { .. }));
}

#[tokio::test]
async fn implicit_side_effect_without_popup_is_denied() {
    let (_dir, plane) = make_plane(ScriptedPopup::deny_all(), None);
    let err = plane
        .authorize(&req("fs.write", &["fs.write"], Sensitivity::None, true))
        .await
        .unwrap_err();
    assert!(matches!(err, crate::PlaneError::Denied { .. }));
}

#[tokio::test]
async fn audit_hash_chain_verifies() {
    let (dir, plane) = make_plane(ScriptedPopup::deny_all(), None);
    drop(
        plane
            .authorize(&req("utility.hash", &[], Sensitivity::None, true))
            .await,
    );
    drop(
        plane
            .authorize(&req("fs.write", &["fs.write"], Sensitivity::None, true))
            .await,
    );
    plane.audit().verify_chain().unwrap();
    assert!(plane.audit().records().unwrap().len() >= 2);
    drop(dir);
}

#[tokio::test]
async fn policy_add_requires_confirmation() {
    let popup = Arc::new(ScriptedPopup::new([PopupDecision::Deny]));
    let (_dir, plane) = make_plane(popup, None);
    let err = plane
        .policy_add(
            PolicyRule {
                tool: "fs.write".to_owned(),
                scope: None,
                decision: PolicyDecision::Allow,
            },
            &req("approval.policy_add", &["policy"], Sensitivity::None, true),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, crate::PlaneError::Denied { .. }));
    assert!(plane.policy().rules.is_empty());

    let popup = Arc::new(ScriptedPopup::new([PopupDecision::Allow]));
    let (_dir, plane) = make_plane(popup, None);
    plane
        .policy_add(
            PolicyRule {
                tool: "fs.write".to_owned(),
                scope: None,
                decision: PolicyDecision::Allow,
            },
            &req("approval.policy_add", &["policy"], Sensitivity::None, true),
        )
        .await
        .unwrap();
    assert_eq!(plane.policy().rules.len(), 1);
}

#[tokio::test]
async fn ai_judgement_reason_is_audited() {
    let ai = Arc::new(ScriptedAi {
        judgement: Ok(AiJudgement {
            allow: true,
            reason: "workspace write looks safe".to_owned(),
        }),
    });
    let (_dir, plane) = make_plane(ScriptedPopup::deny_all(), Some(ai));
    plane.set_mode(ApprovalMode::AiAuto).unwrap();
    plane
        .authorize(&req("fs.write", &["fs.write"], Sensitivity::None, true))
        .await
        .unwrap();
    let records = plane.audit().records().unwrap();
    assert!(records.iter().any(|row| {
        row.kind == "ai_judgement"
            && row.payload.get("reason").and_then(|v| v.as_str())
                == Some("workspace write looks safe")
    }));
}

#[tokio::test]
async fn high_risk_skips_ai_and_pops() {
    let ai = Arc::new(ScriptedAi {
        judgement: Ok(AiJudgement {
            allow: true,
            reason: "should not run".to_owned(),
        }),
    });
    let (_dir, plane) = make_plane(ScriptedPopup::deny_all(), Some(ai));
    plane.set_mode(ApprovalMode::AiAuto).unwrap();
    let err = plane
        .authorize(&req("exec.run", &["exec"], Sensitivity::None, true))
        .await
        .unwrap_err();
    assert!(matches!(err, crate::PlaneError::Denied { .. }));
    let records = plane.audit().records().unwrap();
    assert!(!records.iter().any(|row| row.kind == "ai_judgement"));
}

#[test]
fn vault_inject_ref_does_not_embed_plaintext() {
    let dir = TempDir::new().unwrap();
    let vault = Vault::open_file(dir.path().join("vault.bin"), "pass").unwrap();
    let inject = vault.put("openai", b"sk-secret").unwrap();
    assert_eq!(inject.credential_id, "openai");
    assert!(!format!("{inject:?}").contains("sk-secret"));
    let bytes = vault.inject(&inject).unwrap();
    assert_eq!(bytes, b"sk-secret");
    let exported = vault.export("openai").unwrap();
    assert_eq!(exported, b"sk-secret");
}

#[test]
fn exec_is_higher_risk_than_workspace_fs_write() {
    let fs = req("fs.write", &["fs.write"], Sensitivity::None, true);
    let exec = req("exec.run", &["exec"], Sensitivity::None, true);
    assert_eq!(Risk::classify(&fs), Risk::Low);
    assert_eq!(Risk::classify(&exec), Risk::High);
}

#[test]
fn risk_screenshot_is_medium_with_empty_side_effects() {
    let classified = req("app.screenshot", &[], Sensitivity::High, true);
    assert_eq!(Risk::classify(&classified), Risk::Medium);
}
