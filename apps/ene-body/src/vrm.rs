//! VRM loader boundary.
//!
//! Provisional preferred runtime is `vrm-runtime` (expression, humanoid,
//! LookAt, SpringBone). It is **not** a Cargo dependency here: 0.1 API churn
//! must not become the product contract, and lock-in waits on a VRM probe
//! (first-party-desktop §7.2). Hand-rolled glTF `VRMC_vrm` extras are not
//! the product path. SpringBone is not dropped as a design decision —
//! [`FeatureSupport::Unsupported`] is explicit until a runtime is adopted.
//!
//! Asset refs are paths or parent-owned temp files, never Host PKs. This
//! module does not read conversation text or secrets.

use crate::ipc::{AssetFailInfo, AssetFailReason, AssetRef, FeatureSupport, PoseHint};

/// Per-process VRM session. Holds at most one current asset ref.
#[derive(Debug, Default)]
pub struct VrmSession {
    current: Option<AssetRef>,
    pose: PoseHint,
}

impl VrmSession {
    #[must_use]
    pub fn new() -> Self {
        Self {
            current: None,
            pose: PoseHint::Idle,
        }
    }

    #[must_use]
    pub fn expressions(&self) -> FeatureSupport {
        FeatureSupport::Unsupported
    }

    #[must_use]
    pub fn spring_bone(&self) -> FeatureSupport {
        FeatureSupport::Unsupported
    }

    #[must_use]
    pub fn pose(&self) -> PoseHint {
        self.pose
    }

    #[must_use]
    pub fn current(&self) -> Option<&AssetRef> {
        self.current.as_ref()
    }

    /// Records a pose class. Expression / SpringBone evaluation is
    /// unsupported until a runtime is adopted; the hint is still kept so
    /// health ticks and a later loader see the last projection.
    pub fn set_pose(&mut self, pose: PoseHint) {
        self.pose = pose;
    }

    /// Accepts a path or temp-file ref after a metadata check. Does not parse
    /// glTF extras.
    ///
    /// # Errors
    ///
    /// Empty path, missing path, or a path that is not a regular file.
    pub fn set_asset(&mut self, asset: AssetRef) -> Result<(), AssetFailInfo> {
        let path = asset.path();
        if path.is_empty() {
            return Err(AssetFailInfo {
                reason: AssetFailReason::EmptyPath,
            });
        }
        let meta = std::fs::metadata(path).map_err(|_| AssetFailInfo {
            reason: AssetFailReason::Missing,
        })?;
        if !meta.is_file() {
            return Err(AssetFailInfo {
                reason: AssetFailReason::NotAFile,
            });
        }
        self.current = Some(asset);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::VrmSession;
    use crate::ipc::{AssetFailReason, AssetRef, FeatureSupport, PoseHint};
    use std::io::Write as _;

    #[test]
    fn expressions_and_spring_bone_are_explicitly_unsupported() {
        let session = VrmSession::new();
        assert_eq!(session.expressions(), FeatureSupport::Unsupported);
        assert_eq!(session.spring_bone(), FeatureSupport::Unsupported);
        assert_eq!(session.pose(), PoseHint::Idle);
    }

    #[test]
    fn pose_hint_is_kept_without_claiming_runtime_evaluation() {
        let mut session = VrmSession::new();
        session.set_pose(PoseHint::Speaking);
        assert_eq!(session.pose(), PoseHint::Speaking);
        assert_eq!(session.expressions(), FeatureSupport::Unsupported);
        assert_eq!(session.spring_bone(), FeatureSupport::Unsupported);
    }

    #[test]
    fn missing_asset_is_a_domain_fail_not_a_process_crash() {
        let mut session = VrmSession::new();
        let err = session
            .set_asset(AssetRef::Path {
                path: "/no/such/ene-body-asset.vrm".into(),
            })
            .expect_err("missing path");
        assert_eq!(err.reason, AssetFailReason::Missing);
        assert!(session.current().is_none());
    }

    #[test]
    fn existing_file_is_referenced_without_gltf_parse() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("placeholder.vrm");
        let mut file = std::fs::File::create(&path).expect("create");
        file.write_all(b"not-a-vrm").expect("write");
        drop(file);
        let mut session = VrmSession::new();
        session
            .set_asset(AssetRef::BytesTemp {
                path: path.to_string_lossy().into_owned(),
            })
            .expect("placeholder file is a legal ref");
        assert!(session.current().is_some());
        assert_eq!(session.spring_bone(), FeatureSupport::Unsupported);
    }

    #[test]
    fn directory_is_not_a_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut session = VrmSession::new();
        let err = session
            .set_asset(AssetRef::Path {
                path: dir.path().to_string_lossy().into_owned(),
            })
            .expect_err("dir");
        assert_eq!(err.reason, AssetFailReason::NotAFile);
    }

    #[test]
    fn empty_path_is_rejected() {
        let mut session = VrmSession::new();
        let err = session
            .set_asset(AssetRef::Path {
                path: String::new(),
            })
            .expect_err("empty");
        assert_eq!(err.reason, AssetFailReason::EmptyPath);
    }
}
