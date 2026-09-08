use thiserror::Error;

pub type VrmResult<T> = Result<T, VrmError>;

/// Errors raised while loading a `.vrm` file into a
/// [`VrmModel`](crate::model::VrmModel). The variants distinguish
/// "not VRM at all" from "VRM we cannot use" so hosts can show a
/// specific recovery hint for each case.
#[derive(Debug, Error)]
pub enum VrmError {
    #[error("failed to read VRM file {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse glTF: {0}")]
    Gltf(String),

    #[error("file is not a VRM model (no VRMC_vrm or VRM root extension)")]
    NotVrm,

    #[error("file is a VRM model but declares an unsupported spec version in {path}")]
    UnsupportedFormat { path: String },

    #[error("VRM file is malformed: {0}")]
    Malformed(String),

    #[error("glTF has no meshes")]
    NoMeshes,

    #[error("mesh {0} has no POSITION attribute")]
    NoPositions(usize),

    #[error("mesh {mesh} primitive {primitive} uses unsupported topology")]
    UnsupportedTopology { mesh: usize, primitive: usize },

    #[error("failed to decode material texture: {0}")]
    TextureDecode(String),

    #[error("wgpu: {0}")]
    Wgpu(#[from] wgpu::CreateSurfaceError),
}
