use serde::{Deserialize, Serialize};

const LEN_PREFIX_LEN: usize = 4;

pub const MAX_FRAME_BYTES: usize = 64 * 1024;

pub const MAX_MOTION_CLIPS: usize = 16;

pub const MAX_MOTION_PATH_BYTES: usize = 4096;

pub const HEALTH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IpcError {
    #[error("frame body of {len} bytes exceeds the 64 KiB cap")]
    FrameTooLarge { len: usize },
    #[error("truncated frame: have {have} bytes, need {need}")]
    Truncated { have: usize, need: usize },
    #[error("frame body failed to decode: {reason}")]
    DecodeFailed { reason: String },
    #[error("frame body failed to encode: {reason}")]
    EncodeFailed { reason: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ParentToBody {
    Show,
    Hide,
    Placement(PlacementBox),
    PoseHint(PoseHint),
    AssetRef(AssetRef),
    MotionSet(MotionSetInfo),
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BodyToParent {
    Ready(ReadyInfo),
    GpuFail(GpuFailInfo),
    OverlayUnavailable(OverlayUnavailableInfo),
    AssetReady(AssetReadyInfo),
    AssetFail(AssetFailInfo),
    MotionFail(MotionFailInfo),
    HealthTick(HealthTick),
    LocalUi(LocalUiFact),
    Presentation(PresentationFeedback),
    CleanExit,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementBox {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
}

impl PlacementBox {
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.width > 0 && self.height > 0 && self.scale.is_finite() && self.scale > 0.0
    }
}

impl Default for PlacementBox {
    fn default() -> Self {
        Self {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            scale: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PoseHint {
    #[default]
    Idle,
    Listening,
    Speaking,
    Working,
    Attention,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum AssetRef {
    Path { path: String },
    BytesTemp { path: String },
}

impl AssetRef {
    #[must_use]
    pub fn path(&self) -> &str {
        match self {
            Self::Path { path } | Self::BytesTemp { path } => path,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoseClip {
    pub pose: PoseHint,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MotionSetInfo {
    pub clips: Vec<PoseClip>,
}

impl MotionSetInfo {
    #[must_use]
    pub fn clip_for(&self, pose: PoseHint) -> Option<&PoseClip> {
        self.clips.iter().find(|clip| clip.pose == pose)
    }

    pub fn validate(&self) -> Result<(), MotionFailInfo> {
        if self.clips.is_empty() {
            return Err(MotionFailInfo::new(
                MotionFailReason::EmptySet,
                "motion set carries no clips",
            ));
        }
        if self.clips.len() > MAX_MOTION_CLIPS {
            return Err(MotionFailInfo::new(
                MotionFailReason::TooManyClips,
                std::format!(
                    "motion set carries {} clips; the cap is {MAX_MOTION_CLIPS}",
                    self.clips.len()
                ),
            ));
        }
        let mut seen: Vec<PoseHint> = Vec::with_capacity(self.clips.len());
        for clip in &self.clips {
            if clip.path.is_empty() {
                return Err(MotionFailInfo::new(
                    MotionFailReason::EmptyPath,
                    "motion clip path is empty",
                ));
            }
            if clip.path.len() > MAX_MOTION_PATH_BYTES {
                return Err(MotionFailInfo::new(
                    MotionFailReason::PathTooLong,
                    std::format!(
                        "motion clip path of {} bytes exceeds the {MAX_MOTION_PATH_BYTES} byte cap",
                        clip.path.len()
                    ),
                ));
            }
            if seen.contains(&clip.pose) {
                return Err(MotionFailInfo::new(
                    MotionFailReason::DuplicatePose,
                    std::format!("pose {:?} is assigned more than one clip", clip.pose),
                ));
            }
            seen.push(clip.pose);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OverlayKind {
    Headless,
    KdeLayerShell,
    WindowsDwm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GpuInitStatus {
    Ok,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FeatureSupport {
    Available,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadyInfo {
    pub overlay: OverlayKind,
    pub gpu: GpuInitStatus,
    pub expressions: FeatureSupport,
    pub spring_bone: FeatureSupport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GpuFailReason {
    NoAdapter,
    RequestDevice,
    Surface,
    DeviceLost,
    OutOfMemory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GpuFailInfo {
    pub reason: GpuFailReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayUnavailableInfo {
    pub requested: OverlayKind,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AssetFailReason {
    Missing,
    NotAFile,
    EmptyPath,
    InvalidVrm,
    MissingPrimitives,
    MissingExpressions,
    MissingLookAt,
    MissingSpringBone,
    RuntimeEvaluation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetFailInfo {
    pub reason: AssetFailReason,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MotionFailReason {
    EmptySet,
    TooManyClips,
    DuplicatePose,
    EmptyPath,
    PathTooLong,
    Missing,
    NotAFile,
    InvalidVrma,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MotionFailInfo {
    pub reason: MotionFailReason,
    pub detail: String,
}

impl MotionFailInfo {
    #[must_use]
    pub fn new(reason: MotionFailReason, detail: impl Into<String>) -> Self {
        Self {
            reason,
            detail: detail.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetReadyInfo {
    pub primitives: usize,
    pub expressions: usize,
    pub spring_chains: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthTick {
    pub seq: u64,
    pub visible: bool,
    pub pose: PoseHint,
    pub gpu_ok: bool,
    pub overlay: OverlayKind,
    pub expressions: FeatureSupport,
    pub spring_bone: FeatureSupport,
    pub motion: FeatureSupport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum LocalUiFact {
    Drag { x: i32, y: i32 },
    Resize { width: u32, height: u32 },
    Hide,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresentationFeedback {
    pub surface_id: String,
    pub correlation_id: u64,
    pub outcome: PresentationOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PresentationOutcome {
    Submitted,
    Presented {
        timestamp_ns: u64,
        clock_id: i32,
        output: String,
    },
    Discarded,
    Missing {
        reason: String,
    },
}

pub fn encode_parent(message: &ParentToBody) -> Result<Vec<u8>, IpcError> {
    encode_named(message)
}

pub fn encode_body(message: &BodyToParent) -> Result<Vec<u8>, IpcError> {
    encode_named(message)
}

pub fn decode_parent(bytes: &[u8]) -> Result<(ParentToBody, usize), IpcError> {
    decode_named(bytes)
}

pub fn decode_body(bytes: &[u8]) -> Result<(BodyToParent, usize), IpcError> {
    decode_named(bytes)
}

fn encode_named<T: Serialize>(message: &T) -> Result<Vec<u8>, IpcError> {
    let body = rmp_serde::to_vec_named(message).map_err(|error| IpcError::EncodeFailed {
        reason: std::format!("{error}"),
    })?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(IpcError::FrameTooLarge { len: body.len() });
    }
    let len_prefix =
        u32::try_from(body.len()).map_err(|_| IpcError::FrameTooLarge { len: body.len() })?;
    let mut out = Vec::with_capacity(LEN_PREFIX_LEN + body.len());
    out.extend_from_slice(&len_prefix.to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

fn decode_named<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<(T, usize), IpcError> {
    if bytes.len() < LEN_PREFIX_LEN {
        return Err(IpcError::Truncated {
            have: bytes.len(),
            need: LEN_PREFIX_LEN,
        });
    }
    let claimed = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    if claimed > MAX_FRAME_BYTES {
        return Err(IpcError::FrameTooLarge { len: claimed });
    }
    let need = LEN_PREFIX_LEN + claimed;
    if bytes.len() < need {
        return Err(IpcError::Truncated {
            have: bytes.len(),
            need,
        });
    }
    let message = rmp_serde::from_slice(&bytes[LEN_PREFIX_LEN..need]).map_err(|error| {
        IpcError::DecodeFailed {
            reason: std::format!("{error}"),
        }
    })?;
    Ok((message, need))
}

#[cfg(test)]
mod tests {
    use super::{
        AssetFailInfo, AssetFailReason, AssetRef, BodyToParent, FeatureSupport, GpuFailInfo,
        GpuFailReason, GpuInitStatus, HealthTick, IpcError, LocalUiFact, MAX_FRAME_BYTES,
        MAX_MOTION_CLIPS, MAX_MOTION_PATH_BYTES, MotionFailInfo, MotionFailReason, MotionSetInfo,
        OverlayKind, OverlayUnavailableInfo, ParentToBody, PlacementBox, PoseClip, PoseHint,
        PresentationFeedback, PresentationOutcome, ReadyInfo, decode_body, decode_parent,
        encode_body, encode_parent,
    };

    fn roundtrip_parent(message: ParentToBody) {
        let bytes = encode_parent(&message).expect("encode");
        let (decoded, used) = decode_parent(&bytes).expect("decode");
        assert_eq!(used, bytes.len());
        assert_eq!(decoded, message);
    }

    fn roundtrip_body(message: BodyToParent) {
        let bytes = encode_body(&message).expect("encode");
        let (decoded, used) = decode_body(&bytes).expect("decode");
        assert_eq!(used, bytes.len());
        assert_eq!(decoded, message);
    }

    #[test]
    fn parent_commands_roundtrip() {
        roundtrip_parent(ParentToBody::Show);
        roundtrip_parent(ParentToBody::Hide);
        roundtrip_parent(ParentToBody::Shutdown);
        roundtrip_parent(ParentToBody::PoseHint(PoseHint::Speaking));
        roundtrip_parent(ParentToBody::Placement(PlacementBox {
            x: 10,
            y: -4,
            width: 320,
            height: 480,
            scale: 2.0,
        }));
        roundtrip_parent(ParentToBody::AssetRef(AssetRef::Path {
            path: "/tmp/ene.vrm".into(),
        }));
        roundtrip_parent(ParentToBody::AssetRef(AssetRef::BytesTemp {
            path: "/tmp/bytes.vrm".into(),
        }));
        roundtrip_parent(ParentToBody::MotionSet(MotionSetInfo {
            clips: vec![
                PoseClip {
                    pose: PoseHint::Idle,
                    path: "/tmp/VRMA_06.vrma".into(),
                },
                PoseClip {
                    pose: PoseHint::Speaking,
                    path: "/tmp/VRMA_01.vrma".into(),
                },
            ],
        }));
    }

    #[test]
    fn body_events_roundtrip() {
        roundtrip_body(BodyToParent::Ready(ReadyInfo {
            overlay: OverlayKind::Headless,
            gpu: GpuInitStatus::Failed,
            expressions: FeatureSupport::Unsupported,
            spring_bone: FeatureSupport::Unsupported,
        }));
        roundtrip_body(BodyToParent::GpuFail(GpuFailInfo {
            reason: GpuFailReason::NoAdapter,
        }));
        roundtrip_body(BodyToParent::OverlayUnavailable(OverlayUnavailableInfo {
            requested: OverlayKind::KdeLayerShell,
            reason: String::from("fixture"),
        }));
        roundtrip_body(BodyToParent::AssetFail(AssetFailInfo {
            reason: AssetFailReason::Missing,
            detail: String::from("fixture"),
        }));
        roundtrip_body(BodyToParent::MotionFail(MotionFailInfo {
            reason: MotionFailReason::InvalidVrma,
            detail: String::from("fixture"),
        }));
        roundtrip_body(BodyToParent::HealthTick(HealthTick {
            seq: 3,
            visible: true,
            pose: PoseHint::Working,
            gpu_ok: false,
            overlay: OverlayKind::Headless,
            expressions: FeatureSupport::Unsupported,
            spring_bone: FeatureSupport::Unsupported,
            motion: FeatureSupport::Available,
        }));
        roundtrip_body(BodyToParent::LocalUi(LocalUiFact::Hide));
        roundtrip_body(BodyToParent::LocalUi(LocalUiFact::Drag { x: 1, y: 2 }));
        roundtrip_body(BodyToParent::LocalUi(LocalUiFact::Resize {
            width: 8,
            height: 9,
        }));
        roundtrip_body(BodyToParent::Presentation(PresentationFeedback {
            surface_id: String::from("wl_surface@9"),
            correlation_id: 4,
            outcome: PresentationOutcome::Submitted,
        }));
        roundtrip_body(BodyToParent::Presentation(PresentationFeedback {
            surface_id: String::from("wl_surface@9"),
            correlation_id: 5,
            outcome: PresentationOutcome::Presented {
                timestamp_ns: 99,
                clock_id: 1,
                output: String::from("DP-1"),
            },
        }));
        roundtrip_body(BodyToParent::CleanExit);
    }

    #[test]
    fn trailing_bytes_are_the_next_frame() {
        let mut bytes = encode_parent(&ParentToBody::Show).expect("encode");
        let first_len = bytes.len();
        bytes.extend_from_slice(&encode_parent(&ParentToBody::Hide).expect("encode"));
        let (first, used) = decode_parent(&bytes).expect("first");
        assert_eq!(first, ParentToBody::Show);
        assert_eq!(used, first_len);
        let (second, used2) = decode_parent(&bytes[used..]).expect("second");
        assert_eq!(second, ParentToBody::Hide);
        assert_eq!(used + used2, bytes.len());
    }

    #[test]
    fn truncated_prefix_and_body_are_distinct() {
        match decode_parent(&[0, 0]) {
            Err(IpcError::Truncated { have: 2, need: 4 }) => {}
            other => panic!("unexpected {other:?}"),
        }
        match decode_parent(&[0, 0, 0, 8, 1, 2]) {
            Err(IpcError::Truncated { have: 6, need: 12 }) => {}
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn oversize_claim_is_rejected_before_body_work() {
        let mut bytes = vec![0, 1, 0, 1];
        bytes.extend_from_slice(&[0u8; 8]);
        match decode_parent(&bytes) {
            Err(IpcError::FrameTooLarge { len: 0x0001_0001 }) => {}
            other => panic!("unexpected {other:?}"),
        }
        const { assert!(MAX_FRAME_BYTES < 0x0001_0001) };
    }

    #[test]
    fn unknown_parent_variant_is_rejected() {
        #[derive(serde::Serialize)]
        enum Bogus {
            ConversationText(String),
        }
        let inner = rmp_serde::to_vec_named(&Bogus::ConversationText(String::from("hi")))
            .expect("encode bogus");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&u32::try_from(inner.len()).expect("len").to_be_bytes());
        bytes.extend_from_slice(&inner);
        assert!(decode_parent(&bytes).is_err());
    }

    #[test]
    fn invalid_placement_is_detectable() {
        assert!(
            !PlacementBox {
                x: 0,
                y: 0,
                width: 0,
                height: 10,
                scale: 1.0,
            }
            .is_valid()
        );
        assert!(
            !PlacementBox {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
                scale: f32::NAN,
            }
            .is_valid()
        );
        assert!(PlacementBox::default().is_valid());
    }

    #[test]
    fn motion_set_shape_is_checked_before_any_file_is_opened() {
        assert!(MotionSetInfo { clips: Vec::new() }.validate().is_err());
        assert_eq!(
            MotionSetInfo { clips: Vec::new() }
                .validate()
                .expect_err("empty set")
                .reason,
            MotionFailReason::EmptySet
        );
        let too_many = MotionSetInfo {
            clips: (0..=MAX_MOTION_CLIPS)
                .map(|index| PoseClip {
                    pose: PoseHint::Idle,
                    path: std::format!("/tmp/{index}.vrma"),
                })
                .collect(),
        };
        assert_eq!(
            too_many.validate().expect_err("cap").reason,
            MotionFailReason::TooManyClips
        );
        let duplicated = MotionSetInfo {
            clips: vec![
                PoseClip {
                    pose: PoseHint::Idle,
                    path: String::from("/tmp/a.vrma"),
                },
                PoseClip {
                    pose: PoseHint::Idle,
                    path: String::from("/tmp/b.vrma"),
                },
            ],
        };
        assert_eq!(
            duplicated.validate().expect_err("duplicate").reason,
            MotionFailReason::DuplicatePose
        );
        let empty_path = MotionSetInfo {
            clips: vec![PoseClip {
                pose: PoseHint::Idle,
                path: String::new(),
            }],
        };
        assert_eq!(
            empty_path.validate().expect_err("empty path").reason,
            MotionFailReason::EmptyPath
        );
        let long_path = MotionSetInfo {
            clips: vec![PoseClip {
                pose: PoseHint::Idle,
                path: "x".repeat(MAX_MOTION_PATH_BYTES + 1),
            }],
        };
        assert_eq!(
            long_path.validate().expect_err("long path").reason,
            MotionFailReason::PathTooLong
        );
    }

    #[test]
    fn motion_set_lookup_is_pose_scoped() {
        let set = MotionSetInfo {
            clips: vec![PoseClip {
                pose: PoseHint::Attention,
                path: String::from("/tmp/VRMA_03.vrma"),
            }],
        };
        set.validate().expect("valid set");
        assert_eq!(
            set.clip_for(PoseHint::Attention)
                .map(|clip| clip.path.as_str()),
            Some("/tmp/VRMA_03.vrma")
        );
        assert!(set.clip_for(PoseHint::Idle).is_none());
    }

    #[test]
    fn unknown_motion_pose_and_extra_fields_are_rejected() {
        let unknown_pose = rmp_serde::to_vec_named(&serde_json::json!({
            "MotionSet": { "clips": [{ "pose": "Dancing", "path": "/tmp/x.vrma" }] }
        }))
        .expect("encode unknown pose");
        assert!(decode_parent(&framed(&unknown_pose)).is_err());
        let extra_field = rmp_serde::to_vec_named(&serde_json::json!({
            "MotionSet": { "clips": [], "unexpected": 1 }
        }))
        .expect("encode extra field");
        assert!(decode_parent(&framed(&extra_field)).is_err());
        let known = rmp_serde::to_vec_named(&serde_json::json!({
            "MotionSet": { "clips": [{ "pose": "Idle", "path": "/tmp/VRMA_06.vrma" }] }
        }))
        .expect("encode known pose");
        let (decoded, _) = decode_parent(&framed(&known)).expect("known pose decodes");
        assert_eq!(
            decoded,
            ParentToBody::MotionSet(MotionSetInfo {
                clips: vec![PoseClip {
                    pose: PoseHint::Idle,
                    path: String::from("/tmp/VRMA_06.vrma"),
                }],
            })
        );
    }

    fn framed(body: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&u32::try_from(body.len()).expect("len").to_be_bytes());
        bytes.extend_from_slice(body);
        bytes
    }
}
