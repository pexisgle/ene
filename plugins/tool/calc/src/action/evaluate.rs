use ene_plugin::prelude::*;
use std::collections::BTreeMap;

/// Evaluates a math expression.
///
/// Supports the four basic operators (`+ - * /`), remainder (`%`),
/// power (`^`), parentheses, unary plus/minus, the constants `pi` and
/// `e`, and the functions `sqrt`, `exp`, `ln`, `abs`, `sin`, `cos`,
/// `tan`, `asin`, `acos`, `atan`, `atan2`, `sinh`, `cosh`, `tanh`,
/// `asinh`, `acosh`, `atanh`, `floor`, `ceil`, `round`, `signum`,
/// `max`, `min`, and the angle helpers `deg(x)` (interpret `x` as
/// degrees) and `rad(x)` (interpret `x` as radians). Trigonometric
/// functions take radians by default.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "calc",
    name = "evaluate",
    summary = "Evaluate a mathematical expression and return the numeric result.",
    description = "Evaluates a math expression such as \"2 + 3 * sin(deg(45))\" or \"(1000 * 1.08) ^ 5\". Supports + - * / % ^ operators, parentheses, the constants pi and e, and functions sqrt, exp, ln, abs, sin, cos, tan, asin, acos, atan, atan2, sinh, cosh, tanh, asinh, acosh, atanh, floor, ceil, round, signum, max, min, deg, rad. Trigonometric functions take radians by default; use deg(x) to pass degrees (e.g. sin(deg(45))). Optional variables can be bound by name and referenced in the expression.",
    category = "Utility",
    keywords_primary = "calculate, compute, math, arithmetic, formula, expression",
    side_effects = "Idempotent"
)]
pub struct EvaluateAction {
    expression: String,
    /// Optional variable bindings referenced by the expression,
    /// e.g. {"x": 2, "y": 3} for "x^y". Variable names must start
    /// with a letter or underscore and contain only letters, digits,
    /// or underscores.
    #[serde(default)]
    variables: Option<BTreeMap<String, f64>>,
}

impl EvaluateAction {
    async fn run(&self) -> Result<String, ToolError> {
        evaluate(&self.expression, self.variables.as_ref())
    }
}

fn evaluate(
    expression: &str,
    variables: Option<&BTreeMap<String, f64>>,
) -> Result<String, ToolError> {
    let expression = expression.trim();
    if expression.is_empty() {
        return Err(ToolError::InvalidArguments {
            message: "expression must not be empty".to_string(),
        });
    }

    let expr = expression
        .parse::<meval::Expr>()
        .map_err(|e| ToolError::InvalidArguments {
            message: format!("invalid expression: {e}"),
        })?;

    let mut ctx = meval::Context::new();
    // meval 0.2 only ships radian-based trigonometry; the angle
    // helpers let the caller write deg(45) instead of 45.0*pi/180.
    ctx.func("deg", f64::to_radians);
    ctx.func("rad", f64::to_degrees);
    if let Some(vars) = variables {
        for (name, value) in vars {
            ctx.var(name.clone(), *value);
        }
    }

    let result = expr
        .eval_with_context(ctx)
        .map_err(|e| ToolError::InvalidArguments {
            message: format!("evaluation failed: {e}"),
        })?;

    // meval 0.2 does not guard division by zero, roots of negative
    // numbers, or overflow; those surface as NaN/±inf and would
    // otherwise poison downstream arithmetic.
    if !result.is_finite() {
        return Err(ToolError::InvalidArguments {
            message: "result is not a finite number (division by zero, overflow, \
                      or an undefined operation such as sqrt of a negative number)"
                .to_string(),
        });
    }

    Ok(result.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(expression: &str) -> Result<String, ToolError> {
        evaluate(expression, None)
    }

    #[test]
    fn basic_arithmetic() {
        assert_eq!(eval("1 + 2 * 3").unwrap(), "7");
        assert_eq!(eval("(1 + 2) * 3").unwrap(), "9");
        assert_eq!(eval("2^8").unwrap(), "256");
        assert_eq!(eval("10 % 3").unwrap(), "1");
        assert_eq!(eval("-5 + 3").unwrap(), "-2");
    }

    #[test]
    fn constants_and_functions() {
        assert_eq!(eval("pi").unwrap(), "3.141592653589793");
        assert_eq!(eval("2 + 3 * sin(deg(45))").unwrap(), "4.121320343559642");
        assert_eq!(eval("sqrt(16)").unwrap(), "4");
        assert_eq!(eval("max(1, 5, 3)").unwrap(), "5");
        assert_eq!(eval("floor(2.7)").unwrap(), "2");
        assert_eq!(eval("e^2").unwrap(), "7.3890560989306495");
    }

    #[test]
    fn variables_are_bound() {
        let vars = BTreeMap::from([("x".to_string(), 3.0), ("y".to_string(), 1.0)]);
        assert_eq!(evaluate("x^2 + y", Some(&vars)).unwrap(), "10");
    }

    #[test]
    fn unknown_variable_is_an_error() {
        let err = eval("x + 1").unwrap_err();
        assert!(err.to_string().contains("evaluation failed"), "{err}");
    }

    #[test]
    fn empty_expression_rejected() {
        let err = eval("   ").unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments { .. }));
    }

    #[test]
    fn malformed_expression_rejected() {
        let err = eval("1 +* 2").unwrap_err();
        assert!(err.to_string().contains("invalid expression"), "{err}");
    }

    #[test]
    fn division_by_zero_rejected() {
        let err = eval("1 / 0").unwrap_err();
        assert!(err.to_string().contains("not a finite number"), "{err}");
    }

    #[test]
    fn sqrt_of_negative_rejected() {
        let err = eval("sqrt(-1)").unwrap_err();
        assert!(err.to_string().contains("not a finite number"), "{err}");
    }

    #[test]
    fn overflow_rejected() {
        let err = eval("1e308 * 1e308").unwrap_err();
        assert!(err.to_string().contains("not a finite number"), "{err}");
    }

    #[test]
    #[expect(
        clippy::float_cmp,
        reason = "exact double round-trip equality of the parsed output"
    )]
    fn precision_is_double() {
        let out = eval("10 / 3").unwrap();
        let parsed: f64 = out.parse().unwrap();
        assert_eq!(parsed, 3.333_333_333_333_333_5);
    }

    #[test]
    fn spec_name_and_parameters() {
        let action = EvaluateAction::default();
        let spec = EvaluateAction::spec();
        assert_eq!(action.name(), "calc.evaluate");
        assert_eq!(spec.name.as_str(), "calc.evaluate");
        let props = spec.parameters.get("properties").unwrap();
        assert!(props.get("expression").is_some());
        assert!(props.get("variables").is_some());
    }
}
