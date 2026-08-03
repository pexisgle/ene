use crate::error::RandomError;
use ene_plugin::prelude::*;

/// Picks a random element from a list.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "random",
    name = "pick",
    summary = "Pick a random element from a list.",
    description = "Selects one element uniformly at random from the options list and returns it as a string. The list must contain at least one element.",
    category = "Utility",
    keywords_primary = "pick, choose, select, random, decision, coin",
    side_effects = "ReadOnly"
)]
pub struct PickAction {
    /// The list to pick from; must contain at least one element.
    options: Vec<String>,
}

impl PickAction {
    async fn run(&self) -> Result<String, ToolError> {
        pick(&self.options).map_err(Into::into)
    }
}

/// Samples one element uniformly from `options`.
fn pick(options: &[String]) -> Result<String, RandomError> {
    if options.is_empty() {
        return Err(RandomError::EmptyOptions);
    }
    let index = rand::random_range(0..options.len());
    Ok(options[index].clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_option_is_always_returned() {
        let options = vec!["only".to_string()];
        for _ in 0..20 {
            assert_eq!(pick(&options).unwrap(), "only");
        }
    }

    #[test]
    fn empty_list_is_an_error() {
        assert!(matches!(pick(&[]), Err(RandomError::EmptyOptions)));
    }

    #[test]
    fn picked_element_is_a_member() {
        let options: Vec<String> = ["a", "b", "c", "d", "e"].map(String::from).to_vec();
        for _ in 0..200 {
            let value = pick(&options).unwrap();
            assert!(options.contains(&value), "{value} not in options");
        }
    }

    #[test]
    fn spec_name_and_parameters() {
        let action = PickAction::default();
        let spec = PickAction::spec();
        assert_eq!(action.name(), "random.pick");
        assert_eq!(spec.name.as_str(), "random.pick");
        let props = spec.parameters.get("properties").unwrap();
        assert!(props.get("options").is_some());
    }
}
