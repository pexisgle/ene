//! Trybuild-style smoke test for the `#[derive(ToolSpec)]` proc-macro.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use ene_tool_derive::ToolSpec;
use ene_tool_proto::{KeywordSet, SideEffects, ToolCategory, ToolExample, ToolName, ToolVersion};
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
fn spec_basics() {
    let s = ReadArgs::spec();
    assert_eq!(s.name, ToolName::new("filesystem.read"));
    assert_eq!(s.version, ToolVersion::new(1, 2, 0));
    assert_eq!(s.display_name, "Read");
    assert_eq!(s.summary, "Read a file from disk");
    assert_eq!(s.category, ToolCategory::Filesystem);
    assert_eq!(s.side_effects, SideEffects::ReadOnly);
    assert!(
        s.keywords
            .primary
            .iter()
            .any(|k| k == "read" || k == "open" || k == "cat" || k == "load")
    );
    assert!(
        s.keywords
            .secondary
            .iter()
            .any(|k| k == "file" || k == "view" || k == "text")
    );
    assert!(s.keywords.domain.iter().any(|k| k == "fs"));
    assert!(s.keywords.domain.iter().any(|k| k == "posix"));
    assert!(s.keywords.negative.iter().any(|k| k == "write"));
    assert_eq!(
        s.caveats,
        vec!["Files larger than the configured limit are truncated.".to_string()]
    );
    assert_eq!(s.preconditions, vec!["Path must be readable.".to_string()]);
    assert_eq!(
        s.related,
        vec![
            ToolName::new("filesystem.write"),
            ToolName::new("filesystem.glob")
        ]
    );
    assert_eq!(
        s.keywords,
        KeywordSet {
            primary: vec!["read".into(), "open".into(), "cat".into(), "load".into()],
            secondary: vec!["file".into(), "view".into(), "text".into()],
            domain: vec!["fs".into(), "posix".into()],
            negative: vec!["write".into(), "delete".into()],
        }
    );
}

#[test]
fn spec_examples() {
    let s = ReadArgs::spec();
    assert_eq!(s.examples.len(), 2);
    let ex0 = &s.examples[0];
    assert_eq!(ex0.description, "Read /etc/hostname");
    assert_eq!(ex0.input, serde_json::json!({"path":"/etc/hostname"}));
    assert_eq!(ex0.output, Some("ene.local".to_string()));
    let ex1 = &s.examples[1];
    assert_eq!(ex1.description, "Read missing file");
    assert_eq!(ex1.input, serde_json::json!({"path":"/no/such"}));
    assert_eq!(ex1.output, None);
    // Roundtrip the examples
    for ex in &s.examples {
        let _ = ToolExample {
            description: ex.description.clone(),
            input: ex.input.clone(),
            output: ex.output.clone(),
        };
    }
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
fn spec_embedding_text_contains_keywords() {
    let s = ReadArgs::spec();
    let text = s.embedding_text(ene_tool_proto::EmbeddingField::Description);
    assert!(text.contains("filesystem.read"));
    assert!(text.contains("Primary:"));
    assert!(text.contains("read"));
    let neg = s.embedding_text(ene_tool_proto::EmbeddingField::Negative);
    assert!(neg.contains("write"));
}
