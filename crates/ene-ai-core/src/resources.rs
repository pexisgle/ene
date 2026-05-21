#[cfg(not(debug_assertions))]
use std::fs;
#[cfg(not(debug_assertions))]
use std::path::Path;
use std::path::PathBuf;

/// Initializes the app's data directory. On first launch, copies default assets
/// from the distribution location into the OS-standard data directory.
/// Returns the absolute path to the initialized assets directory.
///
/// # Note (Development-only)
/// This first-launch copy from the source tree is a development convenience.
/// In a future release, assets should be deployed to the app data folder at
/// install time (e.g. by an installer or package script) so that the running
/// binary never needs to locate source-tree paths.
pub fn ensure_resource_dirs() -> PathBuf {
    let assets_dir = crate::paths::assets_dir();

    #[cfg(debug_assertions)]
    {
        eprintln!(
            "[Resources] Dev build: using source assets at {}",
            assets_dir.display()
        );
        assets_dir
    }

    #[cfg(not(debug_assertions))]
    {
        if !assets_dir.exists() {
            let source = find_source_dir();
            if let Some(src) = source {
                eprintln!(
                    "[Resources] Deploying default assets to {}",
                    assets_dir.display()
                );
                if let Err(e) = fs::create_dir_all(&assets_dir) {
                    eprintln!("[Resources] Failed to create assets dir: {e}");
                    return assets_dir;
                }
                let Ok(entries) = fs::read_dir(&src) else {
                    return assets_dir;
                };
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let dst = assets_dir.join(&name);
                    if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        if let Err(e) = copy_dir_all(&entry.path(), &dst) {
                            eprintln!("[Resources] Failed to copy {:?}: {e}", name);
                        }
                    } else {
                        if let Err(e) = fs::copy(&entry.path(), &dst) {
                            eprintln!("[Resources] Failed to copy {:?}: {e}", name);
                        }
                    }
                }
            } else {
                eprintln!("[Resources] Default assets not found; running without defaults.");
                let _ = fs::create_dir_all(&assets_dir);
            }
        }

        assets_dir
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
        exe_dir.join("../../crates/ene-app/resources/assets"),
        // Dev: binary in target/<profile>/, assets at workspace root
        exe_dir.join("../assets"),
        exe_dir.join("../resources/assets"),
        // CWD-relative fallbacks
        PathBuf::from("assets"),
        PathBuf::from("resources/assets"),
        PathBuf::from("crates/ene-app/resources/assets"),
    ];

    for c in &candidates {
        if c.is_dir() {
            return Some(c.clone());
        }
    }

    None
}

#[cfg(not(debug_assertions))]
fn copy_dir_all(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("Failed to create {}: {e}", dst.display()))?;
    for entry in fs::read_dir(src).map_err(|e| format!("Failed to read {}: {e}", src.display()))? {
        let entry = entry.map_err(|e| format!("Entry error: {e}"))?;
        let file_type = entry
            .file_type()
            .map_err(|e| format!("File type error: {e}"))?;
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_all(&entry.path(), &dst_path)?;
        } else {
            fs::copy(&entry.path(), &dst_path)
                .map_err(|e| format!("Failed to copy {}: {e}", entry.path().display()))?;
        }
    }
    Ok(())
}
