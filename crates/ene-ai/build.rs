//! Build script for `ene-ai` to ensure upstream `llama-cpp-sys-2` assertion bug in `llama-context.cpp` is patched.

use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    if let Ok(home) = std::env::var("HOME") {
        let cargo_registry_src = PathBuf::from(home).join(".cargo/registry/src");
        if cargo_registry_src.exists()
            && let Ok(entries) = fs::read_dir(cargo_registry_src)
        {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    patch_llama_cpp_sys(&path);
                }
            }
        }
    }
}

fn patch_llama_cpp_sys(registry_dir: &std::path::Path) {
    if let Ok(entries) = fs::read_dir(registry_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("llama-cpp-sys-2-") {
                let llama_context_path = entry.path().join("llama.cpp/src/llama-context.cpp");
                if llama_context_path.is_file()
                    && let Ok(content) = fs::read_to_string(&llama_context_path)
                {
                    let mut modified = content;
                    let target_ar = "GGML_ASSERT(strncmp(n->name, LLAMA_TENSOR_NAME_FGDN_AR \"-\", prefix_len) == 0);";
                    let replacement_ar = "if (strncmp(n->name, LLAMA_TENSOR_NAME_FGDN_AR \"-\", prefix_len) != 0) {\n                    continue;\n                }";
                    let target_ch = "GGML_ASSERT(strncmp(n->name, LLAMA_TENSOR_NAME_FGDN_CH \"-\", prefix_len) == 0);";
                    let replacement_ch = "if (strncmp(n->name, LLAMA_TENSOR_NAME_FGDN_CH \"-\", prefix_len) != 0) {\n                    continue;\n                }";

                    let mut changed = false;
                    if modified.contains(target_ar) {
                        modified = modified.replace(target_ar, replacement_ar);
                        changed = true;
                    }
                    if modified.contains(target_ch) {
                        modified = modified.replace(target_ch, replacement_ch);
                        changed = true;
                    }

                    if changed {
                        let _ = fs::write(&llama_context_path, modified);
                    }
                }
            }
        }
    }
}
