//! Patches upstream `llama-cpp-sys-2` to replace `GGML_ASSERT` with `continue`
//! for FGDN tensor name checks in `llama-context.cpp`.
//!
//! Upstream llama.cpp asserts that every tensor whose name starts with the FGDN
//! prefix matches the expected AR/CH sub-prefix. Models that omit one of the two
//! FGDN tensor groups (e.g. some Gemma variants) trip this assertion and abort.
//! The patch turns the hard assert into a skip (`continue`) so loading proceeds.
//!
//! Gated behind the `patch-llama-fgdn` cargo feature (enabled by default).
//! Remove this build script once upstream `llama-cpp-sys-2` ships the fix.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    if env::var("CARGO_FEATURE_PATCH_LLAMA_FGDN").is_err() {
        return;
    }

    let cargo_home = cargo_home_dir();
    let Some(cargo_home) = cargo_home else {
        println!(
            "cargo:warning=llama-cpp-sys-2 FGDN patch skipped: \
             neither CARGO_HOME nor HOME is set"
        );
        return;
    };

    let registry_src = cargo_home.join("registry/src");
    if !registry_src.exists() {
        println!(
            "cargo:warning=llama-cpp-sys-2 FGDN patch skipped: \
             registry source dir not found at {}",
            registry_src.display()
        );
        return;
    }

    let Ok(index_dirs) = fs::read_dir(&registry_src) else {
        println!(
            "cargo:warning=llama-cpp-sys-2 FGDN patch skipped: \
             cannot read {}",
            registry_src.display()
        );
        return;
    };

    // registry/src/<index-dir>/<crate-name-version>/
    let mut patched_any = false;
    for index_entry in index_dirs.flatten() {
        let index_path = index_entry.path();
        if !index_path.is_dir() {
            continue;
        }
        let Ok(crate_dirs) = fs::read_dir(&index_path) else {
            continue;
        };
        for crate_entry in crate_dirs.flatten() {
            let crate_path = crate_entry.path();
            if crate_path.is_dir() {
                patched_any |= patch_llama_cpp_sys(&crate_path);
            }
        }
    }

    if !patched_any {
        println!(
            "cargo:warning=llama-cpp-sys-2 FGDN patch: no llama-cpp-sys-2-* \
             directory found under {}; the assertion fix was NOT applied",
            registry_src.display()
        );
    }
}

/// Resolves the cargo home directory from `CARGO_HOME`, falling back to
/// `$HOME/.cargo`.
fn cargo_home_dir() -> Option<PathBuf> {
    if let Ok(dir) = env::var("CARGO_HOME") {
        return Some(PathBuf::from(dir));
    }
    let home = env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".cargo"))
}

/// Returns `true` if a patch was applied (or was already applied).
fn patch_llama_cpp_sys(crate_dir: &Path) -> bool {
    let name = crate_dir
        .file_name()
        .map_or(String::new(), |n| n.to_string_lossy().into_owned());
    if !name.starts_with("llama-cpp-sys-2-") {
        return false;
    }

    let llama_context_path = crate_dir.join("llama.cpp/src/llama-context.cpp");
    if !llama_context_path.is_file() {
        println!(
            "cargo:warning=llama-cpp-sys-2 FGDN patch: \
             llama.cpp/src/llama-context.cpp not found in {}",
            crate_dir.display()
        );
        return false;
    }

    let Ok(content) = fs::read_to_string(&llama_context_path) else {
        println!(
            "cargo:warning=llama-cpp-sys-2 FGDN patch: cannot read {}",
            llama_context_path.display()
        );
        return false;
    };

    let target_ar =
        "GGML_ASSERT(strncmp(n->name, LLAMA_TENSOR_NAME_FGDN_AR \"-\", prefix_len) == 0);";
    let replacement_ar = "if (strncmp(n->name, LLAMA_TENSOR_NAME_FGDN_AR \"-\", prefix_len) != 0) {\n                    continue;\n                }";
    let target_ch =
        "GGML_ASSERT(strncmp(n->name, LLAMA_TENSOR_NAME_FGDN_CH \"-\", prefix_len) == 0);";
    let replacement_ch = "if (strncmp(n->name, LLAMA_TENSOR_NAME_FGDN_CH \"-\", prefix_len) != 0) {\n                    continue;\n                }";

    let mut modified = content;
    let mut changed = false;
    if modified.contains(target_ar) {
        modified = modified.replace(target_ar, replacement_ar);
        changed = true;
    }
    if modified.contains(target_ch) {
        modified = modified.replace(target_ch, replacement_ch);
        changed = true;
    }

    if !changed {
        // Already patched or upstream removed the assertions.
        return true;
    }

    if let Err(err) = fs::write(&llama_context_path, &modified) {
        println!(
            "cargo:warning=llama-cpp-sys-2 FGDN patch: failed to write {}: {err}",
            llama_context_path.display()
        );
        return false;
    }

    println!(
        "cargo:warning=llama-cpp-sys-2 FGDN patch applied to {}",
        llama_context_path.display()
    );
    true
}
