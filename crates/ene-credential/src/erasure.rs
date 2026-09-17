//! Credential-owned Targeted Deletion participant (lifecycle §9-§10).
//!
//! The credential / secret owner's local surface is **local metadata and local
//! copies**, never the external service account (SO §4.21, targeted-deletion
//! §4.2 row "漏洩した認証情報（Credential）のコピー"):
//!
//! - the usable-ref registry (`credential_ref`, including the derived
//!   `provider:label` identity), the pending registration rows, and the device
//!   pairing rows (`paired_device` / `pairing_pending`), whose descriptors are
//!   caller-supplied display text; and
//! - the protected device-auth file's entries
//!   ([`FileDeviceAuthStore`](crate::FileDeviceAuthStore)), whose descriptor is
//!   the same display text and whose key can be the target itself.
//!
//! Boundary decisions that the design fixes and this participant keeps:
//!
//! - **No external revoke or rotation.** Targeted Deletion erases local
//!   metadata; it never invalidates, rotates, or re-issues a credential at the
//!   external provider. The registered provider bearer itself stays under the
//!   credential store's own validity management; the erasure does not read,
//!   render, or delete protected bearer values (K-C: values never leave the
//!   credential boundary, and a completion fact never carries one).
//! - **Fail closed.** A credential ref whose identity carries the target is
//!   deleted, so the pair is no longer usable until the Owner re-registers;
//!   deleting a ref advances the credential-set revision, because the usable
//!   set changed and stale scrub premises must be refused. A device entry
//!   whose descriptor or key carries the target is removed, so the device must
//!   pair again.
//! - Host-stamped times (`paired_at`, `requested_at`) and the revision counter
//!   are not copies of caller text and are never mutated.
//!
//! The implementation receives only the protected exact-text material of the
//! operation (lifecycle §9) and is idempotent: a repeated demand finds nothing
//! left to erase and never advances the revision twice (§9.1). A pass whose
//! condition is no longer the operation's current unfinished condition mutates
//! nothing — neither the tables nor the device-auth file — so a late duplicate
//! from a superseded sweep can never erase text the Owner provided after
//! completion (§7).

use std::pin::Pin;
use std::sync::Arc;

use ene_preservation::{
    DemandLocalErasureCommand, ErasureConditionRef, ErasureParticipant, MechanicalDeletionTarget,
    ParticipantCompletionFact, ParticipantHoldClass, ParticipantOwnerRef,
};
use ene_primitive::WallClockWithTz;

use crate::{CredentialTechnicalError, FileDeviceAuthStore};

/// Result of one bounded local erasure pass over credential-owned metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialErasureOutcome {
    /// The condition is not the operation's current unfinished condition (a
    /// superseded sweep or a completed operation): nothing was read or
    /// mutated, and no verification is claimed for it.
    NotCurrent,
    /// The bounded pass ran: `erased` target-bearing metadata rows were
    /// removed, `remainder` target-bearing metadata rows remain.
    Applied { erased: u64, remainder: u64 },
}

/// Store port for the credential owner's local metadata erasure.
///
/// The `ene-store` adapter implements this against the canonical tables; the
/// protected device-auth file is handled by the participant itself, because it
/// is this crate's own custody boundary and must be gated by the same
/// current-condition decision.
pub trait CredentialErasureRepository: Send + Sync {
    /// Runs one bounded local metadata erasure pass for `condition`.
    ///
    /// The pass must be idempotent for the same `(condition, target)` and must
    /// not mutate anything when `condition` is not current.
    fn erase_target_text(
        &self,
        condition: ErasureConditionRef,
        target: &str,
    ) -> impl std::future::Future<
        Output = Result<CredentialErasureOutcome, CredentialTechnicalError>,
    > + Send;
}

/// The credential owner's [`ErasureParticipant`] implementation.
pub struct CredentialErasureParticipant<R> {
    repository: Arc<R>,
    device_auth: Arc<FileDeviceAuthStore>,
}

impl<R> CredentialErasureParticipant<R> {
    const OWNER: ParticipantOwnerRef = ParticipantOwnerRef::Credential;

    #[must_use]
    pub fn new(repository: Arc<R>, device_auth: Arc<FileDeviceAuthStore>) -> Self {
        Self {
            repository,
            device_auth,
        }
    }
}

impl<R: CredentialErasureRepository + 'static> ErasureParticipant
    for CredentialErasureParticipant<R>
{
    fn owner(&self) -> ParticipantOwnerRef {
        Self::OWNER
    }

    fn demand_local_erasure(
        &self,
        command: DemandLocalErasureCommand,
    ) -> Pin<Box<dyn std::future::Future<Output = ParticipantCompletionFact> + Send + '_>> {
        Box::pin(async move {
            let condition = command.condition();
            let held = |reason: ParticipantHoldClass| {
                ParticipantCompletionFact::held(
                    condition,
                    Self::OWNER,
                    reason,
                    WallClockWithTz::now(),
                )
            };
            // A correlation-only demand carries no protected material: this
            // in-process owner cannot run its mechanical pass, so it holds the
            // sweep explicitly instead of claiming a verification (lifecycle
            // §8.1/§9).
            let Some(target) = command.scope().target() else {
                return held(ParticipantHoldClass::Failed);
            };
            let MechanicalDeletionTarget::ExactText(material) = &target.mechanical;
            let text = material.expose_for_erasure();
            if text.trim().is_empty() {
                return held(ParticipantHoldClass::Failed);
            }
            match self.repository.erase_target_text(condition, text).await {
                // Not-current work mutates nothing, including the protected
                // file: the durable record refuses the fact as stale or
                // completed, so the pass ends without a false completion.
                Ok(CredentialErasureOutcome::NotCurrent) => {
                    ParticipantCompletionFact::local_complete(
                        condition,
                        Self::OWNER,
                        0,
                        0,
                        WallClockWithTz::now(),
                    )
                }
                Ok(CredentialErasureOutcome::Applied { erased, remainder }) => {
                    // The protected device-auth file is part of the same
                    // credential-owned local surface and runs only after the
                    // database pass proved the condition current.
                    let file_erased = match self.device_auth.erase_target_text(text) {
                        Ok(erased) => erased,
                        Err(_) => return held(ParticipantHoldClass::Failed),
                    };
                    let file_remainder = match self.device_auth.count_target_text(text) {
                        Ok(remainder) => remainder,
                        Err(_) => return held(ParticipantHoldClass::Failed),
                    };
                    let erased = erased.saturating_add(file_erased);
                    let remainder = remainder.saturating_add(file_remainder);
                    if remainder == 0 {
                        ParticipantCompletionFact::verified(
                            condition,
                            Self::OWNER,
                            erased,
                            WallClockWithTz::now(),
                        )
                    } else {
                        ParticipantCompletionFact::more_work(
                            condition,
                            Self::OWNER,
                            erased,
                            remainder,
                            WallClockWithTz::now(),
                        )
                    }
                }
                // Storage failure is a hold, never a completion: the residual
                // metadata is unproven, so the operation stays
                // retryable-incomplete.
                Err(_) => held(ParticipantHoldClass::Failed),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Mutex;

    use super::*;
    use crate::DeviceId;
    use ene_preservation::{
        DeletionOperationId, DeletionSearchMaterial, DeletionSweepGeneration,
        ParticipantErasureScope, TargetedDeletionTarget,
    };
    use ene_primitive::RawId;

    fn condition() -> ErasureConditionRef {
        ErasureConditionRef {
            operation: DeletionOperationId::from_raw(RawId::new()),
            sweep: DeletionSweepGeneration::from_u64(1),
        }
    }

    fn command(
        condition: ErasureConditionRef,
        scope: ParticipantErasureScope,
    ) -> DemandLocalErasureCommand {
        DemandLocalErasureCommand::new(condition, ParticipantOwnerRef::Credential, scope)
    }

    fn local_scope(text: &str) -> ParticipantErasureScope {
        ParticipantErasureScope::local(
            TargetedDeletionTarget {
                mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(
                    text.to_owned(),
                )),
                semantic_hints: vec![],
            },
            vec![],
        )
    }

    struct FakeRepo {
        outcome: Mutex<CredentialErasureOutcome>,
    }

    impl CredentialErasureRepository for FakeRepo {
        fn erase_target_text(
            &self,
            _condition: ErasureConditionRef,
            _target: &str,
        ) -> impl std::future::Future<
            Output = Result<CredentialErasureOutcome, CredentialTechnicalError>,
        > + Send {
            let outcome = *self.outcome.lock().unwrap();
            async move { Ok(outcome) }
        }
    }

    fn device_file() -> (tempfile::TempDir, FileDeviceAuthStore, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device-auth.json");
        let store = FileDeviceAuthStore::open(&path).unwrap();
        (dir, store, path)
    }

    #[tokio::test]
    async fn a_verified_pass_erases_the_protected_descriptor_copy() {
        let (_dir, file, _path) = device_file();
        let device = DeviceId(RawId::new());
        file.save_secret(&device, "phone target-label", "device-secret")
            .unwrap();
        let other = DeviceId(RawId::new());
        file.save_secret(&other, "laptop", "other-secret").unwrap();

        let repo = Arc::new(FakeRepo {
            outcome: Mutex::new(CredentialErasureOutcome::Applied {
                erased: 2,
                remainder: 0,
            }),
        });
        let participant = CredentialErasureParticipant::new(repo, Arc::new(file.clone()));
        let condition = condition();
        let fact = participant
            .demand_local_erasure(command(condition, local_scope("target-label")))
            .await;
        assert_eq!(
            fact.status(),
            ene_preservation::ParticipantCompletionStatus::Verified
        );
        assert_eq!(
            fact.erased_count(),
            3,
            "two metadata rows plus one file entry"
        );
        assert!(file.load_secret(&device).unwrap().is_none());
        assert!(file.load_secret(&other).unwrap().is_some());
        assert!(!format!("{fact:?}").contains("target-label"));
    }

    #[tokio::test]
    async fn a_not_current_pass_never_touches_the_protected_file() {
        let (_dir, file, _path) = device_file();
        let device = DeviceId(RawId::new());
        file.save_secret(&device, "phone target-label", "device-secret")
            .unwrap();
        let repo = Arc::new(FakeRepo {
            outcome: Mutex::new(CredentialErasureOutcome::NotCurrent),
        });
        let participant = CredentialErasureParticipant::new(repo, Arc::new(file.clone()));
        let fact = participant
            .demand_local_erasure(command(condition(), local_scope("target-label")))
            .await;
        assert_eq!(
            fact.status(),
            ene_preservation::ParticipantCompletionStatus::LocalComplete
        );
        assert!(file.load_secret(&device).unwrap().is_some());
    }

    #[tokio::test]
    async fn a_correlation_only_demand_is_an_explicit_hold() {
        let (_dir, file, _path) = device_file();
        let repo = Arc::new(FakeRepo {
            outcome: Mutex::new(CredentialErasureOutcome::Applied {
                erased: 0,
                remainder: 0,
            }),
        });
        let participant = CredentialErasureParticipant::new(repo, Arc::new(file));
        let fact = participant
            .demand_local_erasure(command(
                condition(),
                ParticipantErasureScope::correlation_only(vec![RawId::new()]),
            ))
            .await;
        assert_eq!(
            fact.status(),
            ene_preservation::ParticipantCompletionStatus::Held(ParticipantHoldClass::Failed)
        );
    }

    #[tokio::test]
    async fn the_protected_file_erasure_is_idempotent_and_scoped() {
        let (_dir, file, _path) = device_file();
        let device = DeviceId(RawId::new());
        file.save_secret(&device, "phone target-label", "device-secret")
            .unwrap();
        assert_eq!(file.erase_target_text("target-label").unwrap(), 1);
        assert_eq!(
            file.erase_target_text("target-label").unwrap(),
            0,
            "a duplicate sweep finds nothing left to erase"
        );
        assert_eq!(file.count_target_text("target-label").unwrap(), 0);
        assert!(file.load_secret(&device).unwrap().is_none());
    }
}
