use crate::{
    AiJudgement, ApprovalMode, ApprovalPlane, ApprovalSettings, AuditLog, AuthzRequest,
    PolicyDecision, PolicyFile, PolicyRule, PopupDecision, PopupSettings, PopupSink, Risk,
    ScriptedAi, ScriptedPopup, Sensitivity, Vault,
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
        call_id: "call-test".to_owned(),
    }
}

fn make_plane(
    popup: Arc<dyn PopupSink>,
    ai: Option<Arc<dyn crate::ApproveModel>>,
) -> (TempDir, ApprovalPlane) {
    let dir = TempDir::new().unwrap();
    let audit = AuditLog::open(dir.path().join("audit.db")).unwrap();
    let plane = ApprovalPlane::new(ApprovalSettings::default(), audit, popup, ai);
    (dir, plane)
}

#[tokio::test]
async fn policy_allows_workspace_write() {
    let (_dir, plane) = make_plane(Arc::new(ScriptedPopup::deny_all()), None);
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
    let popup: Arc<dyn PopupSink> = Arc::new(ScriptedPopup::new([PopupDecision::Deny]));
    let (_dir, plane) = make_plane(popup, None);
    let err = plane
        .authorize(&req("app.screenshot", &[], Sensitivity::High, true))
        .await
        .unwrap_err();
    assert!(matches!(err, crate::PlaneError::Denied { .. }));
}

#[tokio::test]
async fn implicit_side_effect_without_popup_is_denied() {
    let (_dir, plane) = make_plane(Arc::new(ScriptedPopup::deny_all()), None);
    let err = plane
        .authorize(&req("fs.write", &["fs.write"], Sensitivity::None, true))
        .await
        .unwrap_err();
    assert!(matches!(err, crate::PlaneError::Denied { .. }));
}

#[tokio::test]
async fn audit_hash_chain_verifies() {
    let (dir, plane) = make_plane(Arc::new(ScriptedPopup::deny_all()), None);
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
    let popup: Arc<dyn PopupSink> = Arc::new(ScriptedPopup::new([PopupDecision::Deny]));
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

    let popup: Arc<dyn PopupSink> = Arc::new(ScriptedPopup::new([PopupDecision::Allow]));
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
    let (_dir, plane) = make_plane(Arc::new(ScriptedPopup::deny_all()), Some(ai));
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
async fn set_ai_after_new_is_used_in_ai_auto() {
    let (_dir, plane) = make_plane(Arc::new(ScriptedPopup::deny_all()), None);
    assert!(!plane.has_approve_model());
    plane.set_ai(Arc::new(ScriptedAi {
        judgement: Ok(AiJudgement {
            allow: true,
            reason: "late bind".to_owned(),
        }),
    }));
    assert!(plane.has_approve_model());
    plane.set_mode(ApprovalMode::AiAuto).unwrap();
    plane
        .authorize(&req("fs.write", &["fs.write"], Sensitivity::None, true))
        .await
        .unwrap();
    let records = plane.audit().records().unwrap();
    assert!(records.iter().any(|row| {
        row.kind == "ai_judgement"
            && row.payload.get("reason").and_then(|v| v.as_str()) == Some("late bind")
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
    let (_dir, plane) = make_plane(Arc::new(ScriptedPopup::deny_all()), Some(ai));
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
fn desktop_input_is_high_risk() {
    let click = req("app.click", &["input"], Sensitivity::None, true);
    assert_eq!(Risk::classify(&click), Risk::High);
}

#[test]
fn risk_screenshot_is_medium_with_empty_side_effects() {
    let classified = req("app.screenshot", &[], Sensitivity::High, true);
    assert_eq!(Risk::classify(&classified), Risk::Medium);
}

#[test]
fn risk_fs_read_outside_workspace_is_medium() {
    let outside = req("fs.read", &[], Sensitivity::None, false);
    assert_eq!(Risk::classify(&outside), Risk::Medium);
    let inside = req("fs.read", &[], Sensitivity::None, true);
    assert_eq!(Risk::classify(&inside), Risk::None);
    let alias = req("fs.readfoo", &[], Sensitivity::None, false);
    assert_eq!(Risk::classify(&alias), Risk::None);
}

#[tokio::test]
async fn allow_and_remember_appends_policy_rule() {
    let popup = Arc::new(ScriptedPopup::new([PopupDecision::AllowAndRemember]));
    let (_dir, plane) = make_plane(popup, None);
    plane
        .authorize(&req("fs.write", &["fs.write"], Sensitivity::None, true))
        .await
        .unwrap();
    assert_eq!(plane.policy().rules.len(), 1);
    assert_eq!(plane.policy().rules[0].tool, "fs.write");
    assert_eq!(plane.policy().rules[0].scope.as_deref(), Some("workspace"));
    let records = plane.audit().records().unwrap();
    assert!(records.iter().any(|row| row.kind == "policy"));
}

#[tokio::test]
async fn allow_and_remember_outside_workspace_does_not_persist() {
    let popup = Arc::new(ScriptedPopup::new([PopupDecision::AllowAndRemember]));
    let (_dir, plane) = make_plane(popup, None);
    plane
        .authorize(&req("fs.write", &["fs.write"], Sensitivity::None, false))
        .await
        .unwrap();
    assert!(plane.policy().rules.is_empty());
}

#[tokio::test]
async fn allow_and_remember_writes_policy_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("policy.json");
    let popup = Arc::new(ScriptedPopup::new([PopupDecision::AllowAndRemember]));
    let audit = AuditLog::open(dir.path().join("audit.db")).unwrap();
    let plane = ApprovalPlane::new(ApprovalSettings::default(), audit, popup, None);
    plane.set_policy_path(path.clone());
    plane
        .authorize(&req("fs.write", &["fs.write"], Sensitivity::None, true))
        .await
        .unwrap();
    let loaded = PolicyFile::load_json(&path).unwrap();
    assert_eq!(loaded.rules.len(), 1);
    assert_eq!(loaded.rules[0].tool, "fs.write");
}

#[test]
fn vault_rejects_empty_passphrase() {
    let dir = TempDir::new().unwrap();
    assert!(matches!(
        Vault::open_file(dir.path().join("vault.bin"), ""),
        Err(crate::VaultError::EmptyPassphrase)
    ));
}

#[test]
fn vault_open_or_create_keyfile_roundtrip() {
    let dir = TempDir::new().unwrap();
    let vault = Vault::open_or_create_keyfile(dir.path().join("vault.bin"), dir.path().join("key"))
        .unwrap();
    let inject = vault.put("demo", b"secret-bytes").unwrap();
    assert_eq!(vault.inject(&inject).unwrap(), b"secret-bytes");
    vault.remove("demo").unwrap();
    assert!(matches!(
        vault.inject(&inject),
        Err(crate::VaultError::Unknown(_))
    ));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.path().join("key"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let vault_mode = std::fs::metadata(dir.path().join("vault.bin"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(vault_mode & 0o777, 0o600);
    }
}

#[tokio::test]
async fn pending_popup_timeout_drops_stale_entry() {
    let popup = Arc::new(crate::PendingPopup::new());
    let plane = ApprovalPlane::new(
        ApprovalSettings {
            popup: PopupSettings { timeout_ms: 50 },
            ..ApprovalSettings::default()
        },
        AuditLog::open(tempfile::TempDir::new().unwrap().path().join("audit.db")).unwrap(),
        Arc::clone(&popup) as Arc<dyn crate::PopupSink>,
        None,
    );
    let request = req("fs.write", &["fs.write"], Sensitivity::None, true);
    let authorize = plane.authorize(&request);
    let err = tokio::time::timeout(std::time::Duration::from_secs(1), authorize)
        .await
        .unwrap()
        .unwrap_err();
    assert!(matches!(err, crate::PlaneError::Denied { .. }));
    assert!(popup.list().is_empty());
}

#[tokio::test]
async fn pending_popup_notifies_on_ask() {
    let popup = Arc::new(crate::PendingPopup::new());
    let seen = Arc::new(std::sync::Mutex::new(None::<String>));
    let seen_cb = Arc::clone(&seen);
    popup.set_on_ask(Arc::new(move |view| {
        *seen_cb.lock().unwrap() = Some(view.tool.clone());
    }));
    let request = req("fs.write", &["fs.write"], Sensitivity::None, true);
    let waiting = Arc::clone(&popup);
    let task = tokio::spawn(async move { waiting.as_ref().ask(&request).await });
    let mut listed = None;
    for _ in 0..50 {
        if let Some(item) = popup.list().into_iter().next() {
            listed = Some(item);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let listed = listed.unwrap();
    assert_eq!(seen.lock().unwrap().as_deref(), Some("fs.write"));
    popup.respond(&listed.id, PopupDecision::Allow).unwrap();
    assert_eq!(task.await.unwrap(), PopupDecision::Allow);
}
