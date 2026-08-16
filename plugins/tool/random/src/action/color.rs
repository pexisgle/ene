use ene_plugin::prelude::*;

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "random",
    name = "color",
    summary = "Generate a random hex color.",
    description = "Generates a random color with uniform 8-bit red, green, and blue channels and returns it as a lowercase CSS hex string, e.g. \"#3f8ab2\".",
    category = "Utility",
    keywords_primary = "color, hex, random, css, palette",
    side_effects = "ReadOnly"
)]
pub struct ColorAction {}

impl ColorAction {
    async fn run(&self) -> Result<String, ToolError> {
        Ok(random_hex_color())
    }
}

fn random_hex_color() -> String {
    let r: u8 = rand::random();
    let g: u8 = rand::random();
    let b: u8 = rand::random();
    format!("#{r:02x}{g:02x}{b:02x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_is_lowercase_hex() {
        for _ in 0..100 {
            let value = random_hex_color();
            assert_eq!(value.len(), 7);
            assert_eq!(value.as_bytes()[0], b'#');
            assert!(
                value[1..]
                    .chars()
                    .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase()),
                "non-lowercase hex in {value}"
            );
            assert!(value[1..].chars().all(|c| c.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn spec_name() {
        let action = ColorAction::default();
        let spec = ColorAction::spec();
        assert_eq!(action.name(), "random.color");
        assert_eq!(spec.name.as_str(), "random.color");
    }
}
