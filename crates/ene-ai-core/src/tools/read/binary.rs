use std::path::Path;

const SAMPLE_BYTES: usize = 4096;

pub fn is_binary_file(path: &Path, bytes: &[u8]) -> bool {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "zip" | "tar" | "gz" | "exe" | "dll" | "so" | "class" | "jar" | "war" | "7z" | "doc"
        | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "odt" | "ods" | "odp" | "bin" | "dat"
        | "obj" | "o" | "a" | "lib" | "wasm" | "pyc" | "pyo" => return true,
        _ => {}
    }

    if bytes.is_empty() {
        return false;
    }

    let mut non_printable = 0usize;
    for &b in bytes.iter().take(SAMPLE_BYTES.min(bytes.len())) {
        if b == 0 {
            return true;
        }
        if b < 9 || (b > 13 && b < 32) {
            non_printable += 1;
        }
    }

    let checked = SAMPLE_BYTES.min(bytes.len());
    non_printable > checked / 3
}
