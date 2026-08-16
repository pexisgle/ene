//! Integration tests for the `#[derive(ToolSpec)]` proc-macro.
#![expect(
    clippy::expect_used,
    reason = "proc-macro smoke tests use expect for schema assertions"
)]

use ene_plugin_macros::ToolSpec;
use ene_plugin_proto::{ToolCategory, ToolName, tool_types::EmbeddingField};
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
    pub path: String,
    pub max_bytes: Option<u64>,
}

#[test]
fn tool_spec_derive_produces_expected_consts() {
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
    let obj = s
        .parameters
        .as_object()
        .expect("spec parameters is a JSON object");
    assert_eq!(obj.get("type").and_then(|v| v.as_str()), Some("object"));
    let props = obj
        .get("properties")
        .and_then(|v| v.as_object())
        .expect("parameters has a properties map");
    assert!(props.contains_key("path"));
    assert!(props.contains_key("max_bytes"));
}

#[test]
fn rag_profile_carries_rich_metadata() {
    let p = ReadArgs::rag_profile();
    assert_eq!(p.name, ToolName::new("filesystem.read"));
    assert_eq!(p.summary, "Read a file from disk");
    assert_eq!(p.category, ToolCategory::Filesystem);
    assert_eq!(p.keywords.primary, vec!["read", "open", "cat", "load"]);
    assert_eq!(p.keywords.negative, vec!["write", "delete"]);
    assert_eq!(p.examples.len(), 2);
    assert_eq!(p.version.to_string(), "1.2.0");
    assert_eq!(
        p.related,
        vec![
            ToolName::new("filesystem.write"),
            ToolName::new("filesystem.glob")
        ]
    );

    let text = p.embedding_text(EmbeddingField::Description, None, None);
    assert!(text.contains("filesystem.read"));
    assert!(text.contains("Reads the contents"));
    assert!(text.contains("keywords: read, open, cat, load"));

    let neg = p.embedding_text(EmbeddingField::Negative, None, None);
    assert_eq!(neg, "filesystem.read NOT: write, delete");
}
