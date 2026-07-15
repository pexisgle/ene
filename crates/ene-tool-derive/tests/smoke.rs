//! Trybuild-style smoke test for the `#[derive(ToolSpec)]` proc-macro.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use ene_tool_derive::ToolSpec;
use ene_tool_proto::ToolName;
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(ToolSpec, Deserialize, JsonSchema)]
#[tool(
    name = "read",
    namespace = "filesystem",
    summary = "Read a file from disk",
    description = "Reads the contents of a file at the given path and returns them as a string.",
    category = "Filesystem",
    side_effects = "ReadOnly",
    keywords_primary = "read, open, cat, load",
    keywords_secondary = "file, view, text",
    keywords_domain = "fs, posix",
    keywords_negative = "write, delete",
    examples = "Read /etc/hostname|{\"path\":\"/etc/hostname\"}|ene.local; Read missing file|{\"path\":\"/no/such\"}",
    caveats = "Files larger than the configured limit are truncated.",
    preconditions = "Path must be readable.",
    related = "filesystem.write, filesystem.glob",
    version = "1.2.0"
)]
pub struct ReadArgs {
    /// Path to the file to read.
    pub path: String,
    /// Optional maximum number of bytes to read.
    pub max_bytes: Option<u64>,
}

#[test]
fn consts() {
    assert_eq!(ReadArgs::TOOL_NAME, "filesystem.read");
    assert_eq!(ReadArgs::DISPLAY_NAME, "Read");
    assert_eq!(ReadArgs::SUMMARY, "Read a file from disk");
}

#[test]
fn spec_llm_facing_only() {
    let s = ReadArgs::spec();
    assert_eq!(s.name, ToolName::new("filesystem.read"));
    assert_eq!(
        s.description,
        "Reads the contents of a file at the given path and returns them as a string."
    );
}

#[test]
fn spec_parameters_is_json_schema() {
    let s = ReadArgs::spec();
    // The schema should be a JSON Schema object.
    let obj = s.parameters.as_object().expect("parameters is an object");
    assert_eq!(obj.get("type").and_then(|v| v.as_str()), Some("object"));
    let props = obj
        .get("properties")
        .and_then(|v| v.as_object())
        .expect("properties is an object");
    assert!(props.contains_key("path"));
    assert!(props.contains_key("max_bytes"));
}

#[test]
fn spec_embedding_text_from_description() {
    let s = ReadArgs::spec();
    let text = s.embedding_text(ene_tool_proto::types::EmbeddingField::Description);
    assert!(text.contains("filesystem.read"));
    assert!(text.contains("Reads the contents"));
}
