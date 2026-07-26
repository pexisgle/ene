//! JSON Schema → GBNF conversion via the local C++ shim (llama.cpp common logic).

use ene_ai::error::LlmProviderError;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;

unsafe extern "C" {
    fn ene_json_schema_to_grammar(schema_json: *const c_char, force_gbnf: i32) -> *mut c_char;
    fn ene_json_schema_grammar_free(ptr: *mut c_char);
}

/// Convert a JSON Schema document to a GBNF grammar string.
pub(crate) fn json_schema_to_grammar(schema_json: &str) -> Result<String, LlmProviderError> {
    let c_schema = CString::new(schema_json)
        .map_err(|e| LlmProviderError::LocalLlm(format!("json schema contains NUL: {e}")))?;
    // SAFETY: CString is NUL-terminated; returned pointer is freed below.
    let ptr = unsafe { ene_json_schema_to_grammar(c_schema.as_ptr(), 1) };
    if ptr.is_null() {
        return Err(LlmProviderError::LocalLlm(
            "json_schema_to_grammar failed".to_string(),
        ));
    }
    // SAFETY: shim returns a valid NUL-terminated heap string on success.
    let grammar = unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned();
    // SAFETY: `ptr` was allocated by `ene_json_schema_to_grammar` and verified to be non-null.
    unsafe {
        ene_json_schema_grammar_free(ptr);
    }
    Ok(grammar)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_object_schema_to_gbnf() {
        let schema =
            r#"{"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"]}"#;
        let grammar = json_schema_to_grammar(schema).expect("grammar conversion");
        assert!(grammar.contains("root"), "{grammar}");
        assert!(
            grammar.contains("true") || grammar.contains("boolean") || grammar.contains("\"ok\""),
            "{grammar}"
        );
    }
}
