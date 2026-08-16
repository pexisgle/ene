use crate::provider::CalcConfig;
use ene_plugin::prelude::*;
use std::sync::{Arc, RwLock};

use super::format_number;

/// Response envelope of the exchangerate.host `convert` endpoint.
#[derive(Debug, Clone, Deserialize)]
struct ConvertResponse {
    success: bool,
    /// The converted amount; present only on success.
    #[serde(default)]
    result: Option<f64>,
    /// Conversion metadata, present only on success.
    #[serde(default)]
    info: Option<RateInfo>,
    /// Exchange-rate reference date, present only on success.
    #[serde(default)]
    date: Option<String>,
    /// API error details, present only on failure.
    #[serde(default)]
    error: Option<ApiError>,
}

#[derive(Debug, Clone, Deserialize)]
struct RateInfo {
    rate: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct ApiError {
    info: Option<String>,
}

fn default_config() -> Arc<RwLock<CalcConfig>> {
    Arc::new(RwLock::new(CalcConfig::default()))
}

/// Converts an amount from one currency to another using live exchange
/// rates from exchangerate.host.
///
/// Requires an access key, configured either in the plugin config
/// (`plugins.list.calc.config.exchangerate_host_access_key`) or in the
/// `EXCHANGERATE_HOST_API_KEY` environment variable — which the host only
/// forwards when the plugin entry's `env_passthrough` lists it (the
/// default entry forwards no variables). `from`/`to` are ISO 4217 codes
/// (e.g. USD, EUR, JPY).
#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "calc",
    name = "currency_convert",
    summary = "Convert an amount between two currencies using live exchange rates.",
    description = "Converts a monetary amount between two currencies (ISO 4217 codes, e.g. USD, EUR, JPY, GBP) using live rates from exchangerate.host, e.g. 100 USD to EUR. Returns the converted amount, the exchange rate, and the reference date.",
    category = "Utility",
    keywords_primary = "currency, exchange, convert, usd, eur, jpy, money, forex",
    side_effects = "Network { external: true }"
)]
pub struct CurrencyConvertAction {
    #[tool(skip)]
    #[serde(skip, default = "default_config")]
    config: Arc<RwLock<CalcConfig>>,
    amount: f64,
    /// The source currency code (ISO 4217), e.g. "USD".
    from: String,
    /// The target currency code (ISO 4217), e.g. "EUR".
    to: String,
}

impl CurrencyConvertAction {
    #[must_use]
    pub fn new(config: Arc<RwLock<CalcConfig>>) -> Self {
        Self {
            config,
            amount: 0.0,
            from: String::new(),
            to: String::new(),
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        if !self.amount.is_finite() || self.amount < 0.0 {
            return Err(ToolError::InvalidArguments {
                message: "amount must be a finite non-negative number".to_string(),
            });
        }
        let from = parse_currency_code(&self.from)?;
        let to = parse_currency_code(&self.to)?;

        let access_key = self.resolve_access_key();
        let url = build_convert_url(&from, &to, self.amount, access_key.as_deref())?;

        let response = crate::broker::broker().fetch(url.as_str()).await?;
        if !(200..300).contains(&response.status) {
            return Err(ToolError::execution_failed(format!(
                "exchangerate.host returned HTTP {}",
                response.status
            )));
        }

        let parsed: ConvertResponse = serde_json::from_slice(&response.body)
            .map_err(|e| ToolError::execution_failed(format!("Invalid API response: {e}")))?;
        format_convert_response(&parsed, &from, &to, self.amount)
    }

    fn resolve_access_key(&self) -> Option<String> {
        let configured = match self.config.read() {
            Ok(guard) => guard.exchangerate_host_access_key.clone(),
            Err(e) => {
                tracing::warn!("CalcConfig read lock poisoned: {e}");
                String::new()
            }
        };
        if !configured.is_empty() {
            return Some(configured);
        }
        match std::env::var("EXCHANGERATE_HOST_API_KEY") {
            Ok(key) if !key.is_empty() => Some(key),
            _ => None,
        }
    }
}

fn parse_currency_code(raw: &str) -> Result<String, ToolError> {
    let code = raw.trim().to_ascii_uppercase();
    if code.len() != 3 || !code.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err(ToolError::InvalidArguments {
            message: format!("'{raw}' is not an ISO 4217 currency code (3 letters, e.g. USD)"),
        });
    }
    Ok(code)
}

fn build_convert_url(
    from: &str,
    to: &str,
    amount: f64,
    access_key: Option<&str>,
) -> Result<url::Url, ToolError> {
    let mut url = url::Url::parse("https://api.exchangerate.host/convert")
        .map_err(|e| ToolError::internal(format!("failed to parse exchangerate.host URL: {e}")))?;
    url.query_pairs_mut()
        .append_pair("from", from)
        .append_pair("to", to)
        .append_pair("amount", &amount.to_string());
    if let Some(key) = access_key {
        url.query_pairs_mut().append_pair("access_key", key);
    }
    Ok(url)
}

fn format_convert_response(
    parsed: &ConvertResponse,
    from: &str,
    to: &str,
    amount: f64,
) -> Result<String, ToolError> {
    if !parsed.success {
        let detail = parsed
            .error
            .as_ref()
            .and_then(|e| e.info.clone())
            .unwrap_or_else(|| "no details provided".to_string());
        return Err(ToolError::execution_failed(format!(
            "exchangerate.host rejected the request: {detail}"
        )));
    }

    let Some(result) = parsed.result else {
        return Err(ToolError::execution_failed(
            "exchangerate.host response is missing the result".to_string(),
        ));
    };
    if !result.is_finite() {
        return Err(ToolError::execution_failed(
            "exchangerate.host returned a non-finite result".to_string(),
        ));
    }

    let rate = parsed.info.as_ref().map(|i| i.rate);
    let rate_part = rate.map_or_else(String::new, |r| format!(" (rate {})", format_number(r)));
    let date_part = parsed
        .date
        .as_deref()
        .map_or_else(String::new, |d| format!(" on {d}"));
    Ok(format!(
        "{} {from} = {} {to}{rate_part}{date_part}",
        format_number(amount),
        format_number(result),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUCCESS_FIXTURE: &str = r#"{
        "success": true,
        "query": {"from": "USD", "to": "EUR", "amount": 100},
        "info": {"rate": 0.9234, "timestamp": 1720000000},
        "historical": false,
        "date": "2026-07-01",
        "result": 92.34
    }"#;

    fn convert(amount: f64, from: &str, to: &str) -> Result<String, ToolError> {
        let from = parse_currency_code(from)?;
        let to = parse_currency_code(to)?;
        if !amount.is_finite() || amount < 0.0 {
            return Err(ToolError::InvalidArguments {
                message: "amount must be a finite non-negative number".to_string(),
            });
        }
        let resp: ConvertResponse = serde_json::from_str(SUCCESS_FIXTURE)
            .map_err(|e| ToolError::execution_failed(format!("Invalid API response: {e}")))?;
        format_convert_response(&resp, &from, &to, amount)
    }

    #[test]
    fn converts_successfully() {
        assert_eq!(
            convert(100.0, "USD", "EUR").unwrap(),
            "100 USD = 92.34 EUR (rate 0.9234) on 2026-07-01"
        );
    }

    #[test]
    fn codes_are_case_insensitive() {
        let out = convert(100.0, "usd", "eur").unwrap();
        assert!(out.contains("USD"), "{out}");
    }

    #[test]
    fn invalid_currency_code_rejected() {
        let err = convert(100.0, "US", "EUR").unwrap_err();
        assert!(err.to_string().contains("ISO 4217"), "{err}");
        let err = convert(100.0, "USD1", "EUR").unwrap_err();
        assert!(err.to_string().contains("ISO 4217"), "{err}");
        let err = convert(100.0, "", "EUR").unwrap_err();
        assert!(err.to_string().contains("ISO 4217"), "{err}");
    }

    #[test]
    fn invalid_amount_rejected() {
        let err = convert(-1.0, "USD", "EUR").unwrap_err();
        assert!(err.to_string().contains("amount"), "{err}");
        let err = convert(f64::NAN, "USD", "EUR").unwrap_err();
        assert!(err.to_string().contains("amount"), "{err}");
    }

    #[test]
    fn api_failure_surfaces_error_info() {
        let fixture = r#"{"success": false, "error": {"code": 101, "type": "missing_access_key", "info": "You have not supplied an API Access Key."}}"#;
        let resp: ConvertResponse = serde_json::from_str(fixture).unwrap();
        let err = format_convert_response(&resp, "USD", "EUR", 100.0).unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("access key"),
            "{err}"
        );
    }

    #[test]
    fn success_without_optional_fields() {
        let fixture = r#"{"success": true, "result": 92.34}"#;
        let resp: ConvertResponse = serde_json::from_str(fixture).unwrap();
        let out = format_convert_response(&resp, "USD", "EUR", 100.0).unwrap();
        assert_eq!(out, "100 USD = 92.34 EUR");
    }

    #[test]
    fn non_finite_result_rejected() {
        let fixture = r#"{"success": true, "result": null}"#;
        let resp: ConvertResponse = serde_json::from_str(fixture).unwrap();
        let err = format_convert_response(&resp, "USD", "EUR", 100.0).unwrap_err();
        assert!(err.to_string().contains("result"), "{err}");
    }

    #[test]
    fn spec_name() {
        let action = CurrencyConvertAction::new(default_config());
        assert_eq!(action.name(), "calc.currency_convert");
        assert_eq!(
            CurrencyConvertAction::spec().name.as_str(),
            "calc.currency_convert"
        );
    }

    #[test]
    fn convert_url_carries_key_in_query() {
        let url = build_convert_url("USD", "EUR", 100.0, Some("secret")).unwrap();
        assert!(url.as_str().contains("access_key=secret"), "{url}");
    }
}
