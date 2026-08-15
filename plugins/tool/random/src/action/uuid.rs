use ene_plugin::prelude::*;

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "random",
    name = "uuid",
    summary = "Generate a random UUID (version 4).",
    description = "Generates a version 4 (random) UUID and returns it in canonical hyphenated lowercase form, e.g. \"550e8400-e29b-41d4-a716-446655440000\".",
    category = "Utility",
    keywords_primary = "uuid, id, identifier, v4, random",
    side_effects = "ReadOnly"
)]
pub struct UuidAction {}

impl UuidAction {
    async fn run(&self) -> Result<String, ToolError> {
        Ok(uuid::Uuid::new_v4().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn generated_uuid_is_v4_and_canonical() {
        for _ in 0..100 {
            let value = UuidAction::default().run().await.unwrap();
            assert_eq!(value.len(), 36);
            assert_eq!(value.as_bytes()[8], b'-');
            assert_eq!(value.as_bytes()[13], b'-');
            assert_eq!(value.as_bytes()[18], b'-');
            assert_eq!(value.as_bytes()[23], b'-');
            assert_eq!(&value[14..15], "4");
            assert!(
                matches!(value.as_bytes()[19], b'8' | b'9' | b'a' | b'b'),
                "unexpected variant nibble in {value}"
            );
            let parsed = uuid::Uuid::parse_str(&value).unwrap();
            assert_eq!(parsed.to_string(), value);
        }
    }

    #[test]
    fn spec_name() {
        let action = UuidAction::default();
        let spec = UuidAction::spec();
        assert_eq!(action.name(), "random.uuid");
        assert_eq!(spec.name.as_str(), "random.uuid");
    }
}
