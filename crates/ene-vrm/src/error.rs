//! Top-level error type for the vrm crate.

use thiserror::Error;

/// Result alias used by the public vrm API.
pub type VrmResult<T> = Result<T, VrmError>;

/// Errors produced by the vrm loader / renderer.
#[derive(Debug, Error)]
pub enum VrmError {
    /// I/O failure while reading the `.vrm` file.
    #[error("failed to read VRM file {path}: {source}")]
    Io {
        /// Path that was being read.
        path: String,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// glTF parsing failure.
    #[error("failed to parse glTF: {0}")]
    Gltf(String),

    /// The file parses but does not advertise the VRM 1.0
    /// `VRMC_vrm` extension.
    #[error("file is not a VRM 1.0 model (missing VRMC_vrm extension)")]
    NotVrm,

    /// The glTF has no meshes.
    #[error("glTF has no meshes")]
    NoMeshes,

    /// The mesh has no `POSITION` attribute.
    #[error("mesh {0} has no POSITION attribute")]
    NoPositions(usize),

    /// A primitive has an unsupported topology.
    #[error("mesh {mesh} primitive {primitive} uses unsupported topology")]
    UnsupportedTopology {
        /// Mesh index in the glTF document.
        mesh: usize,
        /// Primitive index inside the mesh.
        primitive: usize,
    },

    /// A `KHR_materials_unlit` texture could not be decoded.
    #[error("failed to decode material texture: {0}")]
    TextureDecode(String),

    /// wgpu device / queue creation failure.
    #[error("wgpu: {0}")]
    Wgpu(#[from] wgpu::CreateSurfaceError),
}
