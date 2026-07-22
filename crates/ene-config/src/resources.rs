#[cfg(not(debug_assertions))]
use std::fs;
#[cfg(not(debug_assertions))]
use std::path::Path;
use std::path::PathBuf;

/// Initializes the app's data directory. On first launch, copies default assets
/// from the distribution location into the OS-standard data directory.
/// Returns the absolute path to the initialized assets directory.
///
/// In debug builds the source-tree `assets/` is used directly. In release
/// builds, the assets are copied from a location next to the binary into
/// the OS-standard data directory on first launch.
pub fn ensure_resource_dirs() -> Result<PathBuf, crate::EneConfigError> {
    let assets_dir = crate::paths::assets_dir();

    #[cfg(debug_assertions)]
    {
        tracing::info!(
            "[Resources] Dev build: using source assets at {}",
            assets_dir.display()
        );
        Ok(assets_dir.to_path_buf())
    }

    #[cfg(not(debug_assertions))]
    {
        if !assets_dir.exists() {
            let source = find_source_dir();
            if let Some(src) = source {
                tracing::info!(
                    "[Resources] Deploying default assets to {}",
                    assets_dir.display()
                );
                if let Err(e) = fs::create_dir_all(&assets_dir) {
                    tracing::error!(component = "Resources", error = %e, "Failed to create assets dir");
                    return Err(crate::EneConfigError::IoError(e));
                }
                let entries = fs::read_dir(&src).map_err(crate::EneConfigError::IoError)?;
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let dst = assets_dir.join(&name);
                    if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        if let Err(e) = copy_dir_all(&entry.path(), &dst) {
                            tracing::error!(component = "Resources", file = ?name, error = %e, "Failed to copy dir");
                            return Err(crate::EneConfigError::IoError(e));
                        }
                    } else {
                        if let Err(e) = fs::copy(&entry.path(), &dst) {
                            tracing::error!(component = "Resources", file = ?name, error = %e, "Failed to copy file");
                            return Err(crate::EneConfigError::IoError(e));
                        }
                    }
                }
            } else {
                tracing::warn!(
                    component = "Resources",
                    "Default assets not found; running without defaults."
                );
                if let Err(e) = fs::create_dir_all(&assets_dir) {
                    tracing::error!(component = "Resources", error = %e, "Failed to create fallback assets dir");
                }
            }
        }

        Ok(assets_dir.to_path_buf())
    }
}

#[cfg(not(debug_assertions))]
fn find_source_dir() -> Option<PathBuf> {
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();

    let candidates = [
        // Release: assets/ next to the binary
        exe_dir.join("assets"),
        // Dev: binary in target/debug/, assets at workspace root
        exe_dir.join("../../assets"),
        exe_dir.join("../../resources/assets"),
        // Dev: binary in target/<profile>/, assets at workspace root
        exe_dir.join("../assets"),
        exe_dir.join("../resources/assets"),
        // CWD-relative fallbacks
        PathBuf::from("assets"),
        PathBuf::from("resources/assets"),
    ];

    for c in &candidates {
        if c.is_dir() {
            return Some(c.clone());
        }
    }

    None
}

#[cfg(not(debug_assertions))]
fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_all(&entry.path(), &dst_path)?;
        } else {
            fs::copy(&entry.path(), &dst_path)?;
        }
    }
    Ok(())
}
