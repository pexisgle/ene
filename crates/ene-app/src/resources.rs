/// Delegates to the shared resource initialization in ene-ai-core.
pub fn ensure_resource_dirs() -> std::path::PathBuf {
    ene_ai_core::resources::ensure_resource_dirs()
}
