use thiserror::Error;

pub type VrmResult<T> = Result<T, VrmError>;

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

    #[error("file is not a VRM 1.0 model (missing VRMC_vrm extension)")]
    NotVrm,

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
