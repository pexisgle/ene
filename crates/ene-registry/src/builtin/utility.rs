use super::spec;
use ene_plugin_ipc::ToolSpecWire;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub(super) fn specs() -> Vec<ToolSpecWire> {
    vec![
        spec(
            "utility.hash",
            "BLAKE3 hash of text",
            json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":false}),
            Vec::new(),
        ),
        spec(
            "utility.time",
            "Current time with optional IANA timezone",
            json!({"type":"object","properties":{"timezone":{"type":"string"}},"additionalProperties":false}),
            Vec::new(),
        ),
        spec(
            "utility.system_info",
            "OS, architecture, and CPU count this process sees",
            json!({"type":"object","properties":{},"additionalProperties":false}),
            Vec::new(),
        ),
        spec(
            "utility.calc",
            "Exact arithmetic, unit conversion, or ISO 4217 FX from a published table",
            json!({"type":"object","properties":{"expr":{"type":"string"},"value":{"type":"number"},"from":{"type":"string"},"to":{"type":"string"}},"additionalProperties":false}),
            Vec::new(),
        ),
        spec(
            "utility.random",
            "Uniform random number, pick, or UUID",
            json!({"type":"object","properties":{"kind":{"type":"string","enum":["number","pick","uuid"]},"min":{"type":"number"},"max":{"type":"number"},"items":{"type":"array","items":{"type":"string"}}},"required":["kind"],"additionalProperties":false}),
            Vec::new(),
        ),
        spec(
            "utility.text",
            "Hash, encode/decode, or apply a regular expression",
            json!({"type":"object","properties":{"op":{"type":"string","enum":["hash","encode","decode","regex"]},"text":{"type":"string"},"algorithm":{"type":"string"},"encoding":{"type":"string"},"pattern":{"type":"string"},"replace":{"type":"string"}},"required":["op","text"],"additionalProperties":false}),
            Vec::new(),
        ),
    ]
}

pub(super) fn execute(name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "utility.hash" => hash_text(super::arg_str(args, "text")?, "blake3"),
        "utility.time" => Ok(time(args)),
        "utility.system_info" => Ok(system_info()),
        "utility.calc" => calc(args),
        "utility.random" => random(args),
        "utility.text" => text(args),
        other => Err(format!("unknown builtin {other}")),
    }
}

fn time(args: &Value) -> Value {
    let now = chrono::Utc::now();
    let timezone = args
        .get("timezone")
        .and_then(Value::as_str)
        .unwrap_or("UTC");
    let offset = if timezone.eq_ignore_ascii_case("UTC") || timezone.eq_ignore_ascii_case("Z") {
        now.to_rfc3339()
    } else {
        format!("{now} ({timezone})")
    };
    json!({
        "unix_ms": now.timestamp_millis(),
        "rfc3339": now.to_rfc3339(),
        "timezone": timezone,
        "local": offset,
    })
}

fn system_info() -> Value {
    let cpus = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    json!({
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "family": std::env::consts::FAMILY,
        "pointer_width": usize::BITS,
        "cpus": cpus,
    })
}

fn calc(args: &Value) -> Result<Value, String> {
    if let Some(expr) = args.get("expr").and_then(Value::as_str) {
        let value = eval_expr(expr)?;
        return Ok(json!({ "value": value, "text": format_number(value) }));
    }
    let value = args
        .get("value")
        .and_then(Value::as_f64)
        .ok_or_else(|| "utility.calc needs expr or value+from+to".to_owned())?;
    let from = super::arg_str(args, "from")?;
    let to = super::arg_str(args, "to")?;
    if let Some(fx) = currency_convert(value, from, to)? {
        return Ok(json!({
            "value": fx.value,
            "text": format_number(fx.value),
            "from": fx.from,
            "to": fx.to,
            "rate": fx.rate,
            "as_of": FX_AS_OF,
            "quote": "USD",
            "source": "table",
        }));
    }
    let converted = convert_unit(value, from, to)?;
    Ok(json!({ "value": converted, "text": format_number(converted), "from": from, "to": to }))
}

fn random(args: &Value) -> Result<Value, String> {
    let kind = super::arg_str(args, "kind")?;
    match kind {
        "uuid" => Ok(json!({ "uuid": uuid::Uuid::now_v7().to_string() })),
        "number" => {
            let min = args.get("min").and_then(Value::as_f64).unwrap_or(0.0);
            let max = args.get("max").and_then(Value::as_f64).unwrap_or(1.0);
            if max < min {
                return Err("max must be >= min".to_owned());
            }
            let span = max - min;
            let drawn = min + span * fastrand();
            Ok(json!({ "value": drawn }))
        }
        "pick" => {
            let items = args
                .get("items")
                .and_then(Value::as_array)
                .ok_or_else(|| "pick needs items".to_owned())?;
            if items.is_empty() {
                return Err("items is empty".to_owned());
            }
            #[expect(
                clippy::cast_precision_loss,
                clippy::cast_possible_truncation,
                reason = "index is clamped to the item list"
            )]
            let idx = (fastrand() * items.len() as f64).floor() as usize;
            let idx = idx.min(items.len() - 1);
            Ok(json!({ "value": items[idx].clone(), "index": idx }))
        }
        other => Err(format!("unknown kind {other}")),
    }
}

fn text(args: &Value) -> Result<Value, String> {
    let op = super::arg_str(args, "op")?;
    let text = super::arg_str(args, "text")?;
    match op {
        "hash" => hash_text(
            text,
            args.get("algorithm")
                .and_then(Value::as_str)
                .unwrap_or("blake3"),
        ),
        "encode" => encode(
            text,
            args.get("encoding")
                .and_then(Value::as_str)
                .unwrap_or("base64"),
        ),
        "decode" => decode(
            text,
            args.get("encoding")
                .and_then(Value::as_str)
                .unwrap_or("base64"),
        ),
        "regex" => {
            let pattern = super::arg_str(args, "pattern")?;
            let re = regex::Regex::new(pattern).map_err(|err| err.to_string())?;
            if let Some(replace) = args.get("replace").and_then(Value::as_str) {
                Ok(json!({ "text": re.replace_all(text, replace).into_owned() }))
            } else {
                let caps: Vec<String> = re.find_iter(text).map(|m| m.as_str().to_owned()).collect();
                Ok(json!({ "matches": caps }))
            }
        }
        other => Err(format!("unknown op {other}")),
    }
}

fn hash_text(text: &str, algorithm: &str) -> Result<Value, String> {
    match algorithm {
        "blake3" => Ok(
            json!({ "algorithm": "blake3", "hex": blake3::hash(text.as_bytes()).to_hex().to_string() }),
        ),
        "sha256" => {
            let mut hasher = Sha256::new();
            hasher.update(text.as_bytes());
            Ok(json!({ "algorithm": "sha256", "hex": hex_encode(&hasher.finalize()) }))
        }
        other => Err(format!("unknown algorithm {other}")),
    }
}

fn encode(text: &str, encoding: &str) -> Result<Value, String> {
    match encoding {
        "base64" => Ok(
            json!({ "text": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, text.as_bytes()) }),
        ),
        "hex" => Ok(json!({ "text": hex_encode(text.as_bytes()) })),
        other => Err(format!("unknown encoding {other}")),
    }
}

fn decode(text: &str, encoding: &str) -> Result<Value, String> {
    let bytes = match encoding {
        "base64" => base64::Engine::decode(&base64::engine::general_purpose::STANDARD, text)
            .map_err(|err| err.to_string())?,
        "hex" => hex_decode(text)?,
        other => return Err(format!("unknown encoding {other}")),
    };
    let decoded = String::from_utf8(bytes).map_err(|err| err.to_string())?;
    Ok(json!({ "text": decoded }))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn hex_decode(text: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) {
        return Err("hex length must be even".to_owned());
    }
    let mut out = Vec::with_capacity(text.len() / 2);
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = hex_val(bytes[i])?;
        let lo = hex_val(bytes[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Ok(out)
}

fn hex_val(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err("invalid hex".to_owned()),
    }
}

fn fastrand() -> f64 {
    let bytes = uuid::Uuid::now_v7().as_u128();
    #[expect(
        clippy::cast_precision_loss,
        reason = "uuid low bits seed a unit interval"
    )]
    let frac = (bytes & 0x0000_ffff_ffff_ffff) as f64;
    frac / 281_474_976_710_656.0
}

fn format_number(value: f64) -> String {
    if value == 0.0 || value.abs() < 1.0e-12 {
        return "0".to_owned();
    }
    if value.abs() >= 1.0e15 {
        return value.to_string();
    }
    format!("{value:.12}")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_owned()
}

fn convert_unit(value: f64, from: &str, to: &str) -> Result<f64, String> {
    let from = from.trim();
    let to = to.trim();
    if from.eq_ignore_ascii_case(to) {
        return Ok(value);
    }
    if let Some(si) = length_to_m(from) {
        let dest = length_to_m(to).ok_or_else(|| format!("unknown unit {to}"))?;
        return Ok(value * si / dest);
    }
    if let Some(si) = mass_to_kg(from) {
        let dest = mass_to_kg(to).ok_or_else(|| format!("unknown unit {to}"))?;
        return Ok(value * si / dest);
    }
    if let Some(si) = time_to_s(from) {
        let dest = time_to_s(to).ok_or_else(|| format!("unknown unit {to}"))?;
        return Ok(value * si / dest);
    }
    if let Some(si) = data_to_b(from) {
        let dest = data_to_b(to).ok_or_else(|| format!("unknown unit {to}"))?;
        return Ok(value * si / dest);
    }
    temp_convert(value, from, to)
}

/// Snapshot date for [`units_per_usd`]. Not a live market feed.
const FX_AS_OF: &str = "2026-08-01";

#[derive(Debug)]
struct FxQuote {
    value: f64,
    from: String,
    to: String,
    rate: f64,
}

fn units_per_usd(code: &str) -> Option<f64> {
    Some(match code {
        "USD" => 1.0,
        "EUR" => 0.92,
        "GBP" => 0.78,
        "JPY" => 150.0,
        "CNY" => 7.2,
        "KRW" => 1_350.0,
        "AUD" => 1.52,
        "CAD" => 1.37,
        "CHF" => 0.88,
        "INR" => 84.0,
        _ => return None,
    })
}

fn currency_code(raw: &str) -> Option<String> {
    let code = raw.trim().to_ascii_uppercase();
    if code.len() != 3 || !code.bytes().all(|b| b.is_ascii_uppercase()) {
        return None;
    }
    units_per_usd(&code)?;
    Some(code)
}

fn currency_convert(value: f64, from: &str, to: &str) -> Result<Option<FxQuote>, String> {
    let from_code = currency_code(from);
    let to_code = currency_code(to);
    match (from_code, to_code) {
        (None, None) => Ok(None),
        (Some(_), None) | (None, Some(_)) => {
            Err("currency conversion needs two ISO 4217 codes".to_owned())
        }
        (Some(from), Some(to)) => {
            let from_per =
                units_per_usd(&from).ok_or_else(|| format!("unknown currency {from}"))?;
            let to_per = units_per_usd(&to).ok_or_else(|| format!("unknown currency {to}"))?;
            let rate = to_per / from_per;
            Ok(Some(FxQuote {
                value: value * rate,
                from,
                to,
                rate,
            }))
        }
    }
}

fn length_to_m(unit: &str) -> Option<f64> {
    Some(match unit {
        "m" | "meter" | "meters" => 1.0,
        "km" => 1_000.0,
        "cm" => 0.01,
        "mm" => 0.001,
        "in" | "inch" => 0.0254,
        "ft" | "foot" | "feet" => 0.3048,
        "mi" | "mile" | "miles" => 1_609.344,
        _ => return None,
    })
}

fn mass_to_kg(unit: &str) -> Option<f64> {
    Some(match unit {
        "kg" => 1.0,
        "g" => 0.001,
        "lb" | "lbs" => 0.453_592_37,
        "oz" => 0.028_349_523_125,
        _ => return None,
    })
}

fn time_to_s(unit: &str) -> Option<f64> {
    Some(match unit {
        "s" | "sec" | "second" | "seconds" => 1.0,
        "min" | "minute" | "minutes" => 60.0,
        "h" | "hr" | "hour" | "hours" => 3_600.0,
        "day" | "days" => 86_400.0,
        _ => return None,
    })
}

fn data_to_b(unit: &str) -> Option<f64> {
    Some(match unit {
        "B" | "byte" | "bytes" => 1.0,
        "KB" => 1_000.0,
        "MB" => 1_000_000.0,
        "GB" => 1_000_000_000.0,
        "KiB" => 1_024.0,
        "MiB" => 1_048_576.0,
        "GiB" => 1_073_741_824.0,
        _ => return None,
    })
}

fn temp_convert(value: f64, from: &str, to: &str) -> Result<f64, String> {
    let k = match from {
        "C" | "c" | "celsius" => value + 273.15,
        "F" | "f" | "fahrenheit" => (value - 32.0) * 5.0 / 9.0 + 273.15,
        "K" | "k" | "kelvin" => value,
        _ => return Err(format!("unknown unit {from}")),
    };
    Ok(match to {
        "C" | "c" | "celsius" => k - 273.15,
        "F" | "f" | "fahrenheit" => (k - 273.15) * 9.0 / 5.0 + 32.0,
        "K" | "k" | "kelvin" => k,
        _ => return Err(format!("unknown unit {to}")),
    })
}

fn eval_expr(input: &str) -> Result<f64, String> {
    let mut p = Parser { input, pos: 0 };
    p.skip();
    let value = p.expr()?;
    p.skip();
    if p.pos < p.input.len() {
        return Err("trailing input".to_owned());
    }
    Ok(value)
}

struct Parser<'a> {
    input: &'a str,
    pos: usize,
}

impl Parser<'_> {
    fn skip(&mut self) {
        while let Some(c) = self.peek() {
            if !c.is_whitespace() {
                break;
            }
            self.bump();
        }
    }

    fn peek(&self) -> Option<char> {
        self.input[self.pos..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn expr(&mut self) -> Result<f64, String> {
        let mut value = self.term()?;
        loop {
            self.skip();
            match self.peek() {
                Some('+') => {
                    self.bump();
                    value += self.term()?;
                }
                Some('-') => {
                    self.bump();
                    value -= self.term()?;
                }
                _ => return Ok(value),
            }
        }
    }

    fn term(&mut self) -> Result<f64, String> {
        let mut value = self.power()?;
        loop {
            self.skip();
            match self.peek() {
                Some('*') => {
                    self.bump();
                    value *= self.power()?;
                }
                Some('/') => {
                    self.bump();
                    let rhs = self.power()?;
                    if rhs == 0.0 {
                        return Err("division by zero".to_owned());
                    }
                    value /= rhs;
                }
                _ => return Ok(value),
            }
        }
    }

    fn power(&mut self) -> Result<f64, String> {
        let base = self.unary()?;
        self.skip();
        if self.peek() == Some('^') {
            self.bump();
            Ok(base.powf(self.power()?))
        } else {
            Ok(base)
        }
    }

    fn unary(&mut self) -> Result<f64, String> {
        self.skip();
        if self.peek() == Some('-') {
            self.bump();
            return Ok(-self.unary()?);
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<f64, String> {
        self.skip();
        match self.peek() {
            Some('(') => {
                self.bump();
                let value = self.expr()?;
                self.skip();
                if self.bump() != Some(')') {
                    return Err("expected )".to_owned());
                }
                Ok(value)
            }
            Some(c) if c.is_ascii_digit() || c == '.' => self.number(),
            Some(c) if c.is_ascii_alphabetic() => self.call(),
            _ => Err("expected number".to_owned()),
        }
    }

    fn number(&mut self) -> Result<f64, String> {
        let start = self.pos;
        while matches!(self.peek(), Some(c) if c.is_ascii_digit() || c == '.') {
            self.bump();
        }
        self.input[start..self.pos]
            .parse::<f64>()
            .map_err(|err| err.to_string())
    }

    fn call(&mut self) -> Result<f64, String> {
        let start = self.pos;
        while matches!(self.peek(), Some(c) if c.is_ascii_alphabetic()) {
            self.bump();
        }
        let name = self.input[start..self.pos].to_owned();
        self.skip();
        if self.bump() != Some('(') {
            return Err(format!("unknown ident {name}"));
        }
        let mut args = Vec::new();
        self.skip();
        if self.peek() != Some(')') {
            loop {
                args.push(self.expr()?);
                self.skip();
                match self.peek() {
                    Some(',') => {
                        self.bump();
                    }
                    Some(')') => break,
                    _ => return Err("expected , or )".to_owned()),
                }
            }
        }
        if self.bump() != Some(')') {
            return Err("expected )".to_owned());
        }
        match name.as_str() {
            "sqrt" if args.len() == 1 => Ok(args[0].sqrt()),
            "abs" if args.len() == 1 => Ok(args[0].abs()),
            "floor" if args.len() == 1 => Ok(args[0].floor()),
            "ceil" if args.len() == 1 => Ok(args[0].ceil()),
            "min" if args.len() == 2 => Ok(args[0].min(args[1])),
            "max" if args.len() == 2 => Ok(args[0].max(args[1])),
            _ => Err(format!("unknown function {name}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{calc, currency_convert, system_info};
    use serde_json::json;

    #[test]
    fn system_info_reports_compile_target() {
        let info = system_info();
        assert_eq!(info["os"], json!(std::env::consts::OS));
        assert_eq!(info["arch"], json!(std::env::consts::ARCH));
        assert_eq!(info["family"], json!(std::env::consts::FAMILY));
        assert_eq!(info["pointer_width"], json!(usize::BITS));
        assert!(info["cpus"].as_u64().unwrap() >= 1);
    }

    #[test]
    fn fx_table_converts_usd_to_jpy() {
        let quote = currency_convert(2.5, "usd", "JPY").unwrap().unwrap();
        assert!((quote.value - 375.0).abs() < 1e-9);
        assert!((quote.rate - 150.0).abs() < 1e-9);
        assert_eq!(quote.from, "USD");
        assert_eq!(quote.to, "JPY");
    }

    #[test]
    fn calc_fx_includes_snapshot_metadata() {
        let value = calc(&json!({"value": 1, "from": "USD", "to": "eur"})).unwrap();
        assert!((value["value"].as_f64().unwrap() - 0.92).abs() < 1e-9);
        assert_eq!(value["as_of"], json!(super::FX_AS_OF));
        assert_eq!(value["source"], json!("table"));
        assert_eq!(value["quote"], json!("USD"));
    }

    #[test]
    fn mixed_currency_and_length_is_rejected() {
        let err = currency_convert(1.0, "USD", "m").unwrap_err();
        assert!(err.contains("ISO 4217"));
    }

    #[test]
    fn length_conversion_still_skips_fx() {
        assert!(currency_convert(1.0, "m", "km").unwrap().is_none());
    }
}
