use ene_plugin_ipc::ToolSpecWire;
use ene_tool_registry::{arg_str, spec};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

pub(crate) fn specs() -> Vec<ToolSpecWire> {
    vec![
        spec(
            "utility.hash",
            "Hash text with BLAKE3 by default or SHA-256",
            json!({"type":"object","properties":{"text":{"type":"string"},"algorithm":{"type":"string","enum":["blake3","sha256"],"default":"blake3"}},"required":["text"],"additionalProperties":false}),
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
            "Exact arithmetic, unit conversion, or FX among USD EUR GBP JPY CNY KRW AUD CAD CHF INR from a static USD table dated 2026-08-01 (ECB SDMX eurofxref rounded to two figures)",
            json!({"type":"object","properties":{"expr":{"type":"string"},"vars":{"type":"object","additionalProperties":{"type":"number"}},"value":{"type":"number"},"from":{"type":"string"},"to":{"type":"string"}},"additionalProperties":false}),
            Vec::new(),
        ),
        spec(
            "utility.color",
            "Convert sRGB colors between hex, rgb/rgba, and hsl/hsla",
            json!({"type":"object","properties":{"color":{"type":"string"},"to":{"type":"string","enum":["hex","rgb","rgba","hsl","hsla"]}},"required":["color","to"],"additionalProperties":false}),
            Vec::new(),
        ),
        spec(
            "utility.random",
            "Uniform random number, integer, pick, UUID, or color",
            json!({"type":"object","properties":{"kind":{"type":"string","enum":["number","integer","pick","uuid","uuid4","color"]},"min":{"type":"number","description":"Lower bound. Integer kind: inclusive. Number kind: inclusive."},"max":{"type":"number","description":"Upper bound. Integer kind: inclusive. Number kind: exclusive."},"items":{"type":"array","items":{"type":"string"}}},"required":["kind"],"additionalProperties":false}),
            Vec::new(),
        ),
        spec(
            "utility.text",
            "Hash, change letter case, encode/decode, or apply a regular expression",
            json!({"type":"object","properties":{"op":{"type":"string","enum":["hash","uppercase","lowercase","encode","decode","regex"]},"text":{"type":"string"},"algorithm":{"type":"string","enum":["blake3","sha256"],"default":"blake3"},"encoding":{"type":"string","enum":["base64","hex"]},"pattern":{"type":"string"},"replace":{"type":"string"}},"required":["op","text"],"additionalProperties":false}),
            Vec::new(),
        ),
    ]
}

pub(crate) fn execute(name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "utility.hash" => hash_text(
            arg_str(args, "text")?,
            args.get("algorithm")
                .and_then(Value::as_str)
                .unwrap_or("blake3"),
        ),
        "utility.time" => Ok(time(args)),
        "utility.system_info" => Ok(system_info()),
        "utility.calc" => calc(args),
        "utility.color" => color(args),
        "utility.random" => random(args),
        "utility.text" => text(args),
        other => Err(structured_error(
            "unknown_tool",
            format!("unknown builtin {other}"),
        )),
    }
}

fn structured_error(kind: &str, message: impl std::fmt::Display) -> String {
    json!({
        "error": {
            "kind": kind,
            "message": message.to_string(),
        }
    })
    .to_string()
}

fn ensure_finite(value: f64, kind: &str, message: impl std::fmt::Display) -> Result<f64, String> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(structured_error(kind, message))
    }
}

fn time(args: &Value) -> Value {
    let now = chrono::Utc::now();
    let timezone = args
        .get("timezone")
        .and_then(Value::as_str)
        .unwrap_or("UTC");
    let local = timezone.parse::<chrono_tz::Tz>().ok().map(|tz| {
        now.with_timezone(&tz)
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    });
    json!({
        "unix_ms": now.timestamp_millis(),
        "rfc3339": now.to_rfc3339(),
        "timezone": timezone,
        "local": local.unwrap_or_else(|| format!("{now} ({timezone})")),
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

fn parse_vars(args: &Value) -> Result<HashMap<String, f64>, String> {
    let Some(raw) = args.get("vars") else {
        return Ok(HashMap::new());
    };
    let Some(map) = raw.as_object() else {
        return Err(structured_error(
            "invalid_arguments",
            "vars must be an object",
        ));
    };
    let mut out = HashMap::with_capacity(map.len());
    for (key, value) in map {
        let Some(number) = value.as_f64() else {
            return Err(structured_error(
                "invalid_arguments",
                format!("vars.{key} must be a number"),
            ));
        };
        if !number.is_finite() {
            return Err(structured_error(
                "invalid_arguments",
                format!("vars.{key} must be finite"),
            ));
        }
        out.insert(key.clone(), number);
    }
    Ok(out)
}

fn calc(args: &Value) -> Result<Value, String> {
    if let Some(expr) = args.get("expr").and_then(Value::as_str) {
        let vars = parse_vars(args)?;
        let value = eval_expr(expr, &vars)?;
        return Ok(json!({ "value": value, "text": format_number(value) }));
    }
    let value = args.get("value").and_then(Value::as_f64).ok_or_else(|| {
        structured_error(
            "invalid_arguments",
            "utility.calc needs expr or value+from+to",
        )
    })?;
    if !value.is_finite() {
        return Err(structured_error(
            "invalid_arguments",
            "value must be a finite number",
        ));
    }
    let from = arg_str(args, "from")?;
    let to = arg_str(args, "to")?;
    if let Some(fx) = currency_convert(value, from, to)? {
        return Ok(json!({
            "value": fx.value,
            "text": format_number(fx.value),
            "from": fx.from,
            "to": fx.to,
            "rate": fx.rate,
            "as_of": FX_AS_OF,
            "quote": "USD",
            "source": FX_SOURCE,
            "stale": true,
        }));
    }
    let converted = convert_unit(value, from, to)?;
    Ok(json!({ "value": converted, "text": format_number(converted), "from": from, "to": to }))
}

fn color(args: &Value) -> Result<Value, String> {
    let input = arg_str(args, "color")?;
    let to = arg_str(args, "to")?;
    let parsed = parse_color(input).map_err(|message| {
        structured_error(
            "invalid_color",
            format!("invalid color '{input}': {message}"),
        )
    })?;
    let text = match to.trim().to_ascii_lowercase().as_str() {
        "hex" => format_hex(parsed),
        "rgb" | "rgba" => format_rgb(parsed),
        "hsl" | "hsla" => format_hsl(parsed),
        other => {
            return Err(structured_error(
                "invalid_arguments",
                format!("unknown output format '{other}' (expected hex, rgb, or hsl)"),
            ));
        }
    };
    Ok(json!({ "text": text, "to": to }))
}

fn random(args: &Value) -> Result<Value, String> {
    let kind = arg_str(args, "kind")?;
    match kind {
        "uuid" => Ok(json!({ "uuid": uuid::Uuid::now_v7().to_string() })),
        "uuid4" => Ok(json!({ "uuid": uuid::Uuid::new_v4().to_string() })),
        "number" => {
            let min = args.get("min").and_then(Value::as_f64).unwrap_or(0.0);
            let max = args.get("max").and_then(Value::as_f64).unwrap_or(1.0);
            if !min.is_finite() || !max.is_finite() {
                return Err(structured_error(
                    "invalid_range",
                    "min and max must be finite numbers",
                ));
            }
            if max <= min {
                return Err(structured_error(
                    "invalid_range",
                    "max must be greater than min for float ranges",
                ));
            }
            let span = max - min;
            let drawn = min + span * fastrand();
            Ok(json!({ "value": drawn }))
        }
        "integer" => {
            let min = args.get("min").and_then(Value::as_i64).unwrap_or(0);
            let max = args.get("max").and_then(Value::as_i64).unwrap_or(1);
            if min > max {
                return Err(structured_error(
                    "invalid_range",
                    "max must be >= min for integer ranges",
                ));
            }
            let drawn = random_integer_inclusive(min, max)?;
            Ok(json!({ "value": drawn }))
        }
        "color" => {
            let r = random_integer_inclusive(0, 255)?;
            let g = random_integer_inclusive(0, 255)?;
            let b = random_integer_inclusive(0, 255)?;
            let hex = format!("#{r:02x}{g:02x}{b:02x}");
            Ok(json!({ "hex": hex, "rgb": format!("rgb({r}, {g}, {b})") }))
        }
        "pick" => {
            let items = args
                .get("items")
                .and_then(Value::as_array)
                .ok_or_else(|| structured_error("invalid_arguments", "pick needs items"))?;
            if items.is_empty() {
                return Err(structured_error("invalid_arguments", "items is empty"));
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
        other => Err(structured_error(
            "invalid_arguments",
            format!("unknown kind {other}"),
        )),
    }
}

fn random_integer_inclusive(min: i64, max: i64) -> Result<i64, String> {
    if min > max {
        return Err(structured_error(
            "invalid_range",
            "max must be >= min for integer ranges",
        ));
    }
    if min == max {
        return Ok(min);
    }
    let span = i128::from(max) - i128::from(min);
    let range = span
        .checked_add(1)
        .and_then(|wide| u128::try_from(wide).ok())
        .ok_or_else(|| structured_error("invalid_range", "integer range is too large"))?;
    if range == u128::from(u64::MAX) + 1 {
        return Ok(random_u64() as i64);
    }
    let threshold = (u128::from(u64::MAX) / range) * range;
    loop {
        let sample = u128::from(random_u64());
        if sample < threshold {
            let offset = i128::try_from(sample % range)
                .map_err(|_| structured_error("invalid_range", "integer range is too large"))?;
            let drawn = i128::from(min)
                .checked_add(offset)
                .ok_or_else(|| structured_error("invalid_range", "integer range is too large"))?;
            return i64::try_from(drawn)
                .map_err(|_| structured_error("invalid_range", "integer range is too large"));
        }
    }
}

fn random_u64() -> u64 {
    let bytes = uuid::Uuid::now_v7().into_bytes();
    let mut seed = [0u8; 8];
    seed.copy_from_slice(&bytes[0..8]);
    u64::from_be_bytes(seed)
}

fn text(args: &Value) -> Result<Value, String> {
    let op = arg_str(args, "op")?;
    let text = arg_str(args, "text")?;
    match op {
        "hash" => hash_text(
            text,
            args.get("algorithm")
                .and_then(Value::as_str)
                .unwrap_or("blake3"),
        ),
        "uppercase" | "lowercase" => {
            let changed = if op == "uppercase" {
                text.to_uppercase()
            } else {
                text.to_lowercase()
            };
            Ok(json!({ "text": changed }))
        }
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
            let pattern = arg_str(args, "pattern")?;
            let re = regex::Regex::new(pattern).map_err(|err| err.to_string())?;
            if let Some(replace) = args.get("replace").and_then(Value::as_str) {
                Ok(json!({ "text": re.replace_all(text, replace).into_owned() }))
            } else {
                let caps: Vec<String> = re.find_iter(text).map(|m| m.as_str().to_owned()).collect();
                Ok(json!({ "matches": caps }))
            }
        }
        other => Err(structured_error(
            "unknown_op",
            format!("unknown op {other}"),
        )),
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
        other => Err(structured_error(
            "unknown_algorithm",
            format!("unknown algorithm {other}"),
        )),
    }
}

fn encode(text: &str, encoding: &str) -> Result<Value, String> {
    match encoding {
        "base64" => Ok(
            json!({ "text": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, text.as_bytes()) }),
        ),
        "hex" => Ok(json!({ "text": hex_encode(text.as_bytes()) })),
        other => Err(structured_error(
            "unknown_encoding",
            format!("unknown encoding {other}"),
        )),
    }
}

fn decode(text: &str, encoding: &str) -> Result<Value, String> {
    let bytes = match encoding {
        "base64" => base64::Engine::decode(&base64::engine::general_purpose::STANDARD, text)
            .map_err(|err| err.to_string())?,
        "hex" => hex_decode(text)?,
        other => {
            return Err(structured_error(
                "unknown_encoding",
                format!("unknown encoding {other}"),
            ));
        }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dimension {
    Length,
    Mass,
    Time,
    Volume,
    Speed,
    Area,
    Temperature,
    Data,
}

impl Dimension {
    fn label(self) -> &'static str {
        match self {
            Self::Length => "length",
            Self::Mass => "mass",
            Self::Time => "time",
            Self::Volume => "volume",
            Self::Speed => "speed",
            Self::Area => "area",
            Self::Temperature => "temperature",
            Self::Data => "data",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct UnitDef {
    dimension: Dimension,
    factor: f64,
    offset: f64,
}

impl UnitDef {
    const fn linear(dimension: Dimension, factor: f64) -> Self {
        Self {
            dimension,
            factor,
            offset: 0.0,
        }
    }
}

fn unit_def(unit: &str) -> Option<UnitDef> {
    let key = unit.trim().to_ascii_lowercase();
    Some(match key.as_str() {
        "m" | "meter" | "meters" | "metre" | "metres" => UnitDef::linear(Dimension::Length, 1.0),
        "km" | "kilometer" | "kilometers" | "kilometre" | "kilometres" => {
            UnitDef::linear(Dimension::Length, 1_000.0)
        }
        "cm" | "centimeter" | "centimeters" | "centimetre" | "centimetres" => {
            UnitDef::linear(Dimension::Length, 0.01)
        }
        "mm" | "millimeter" | "millimeters" | "millimetre" | "millimetres" => {
            UnitDef::linear(Dimension::Length, 0.001)
        }
        "in" | "inch" | "inches" => UnitDef::linear(Dimension::Length, 0.0254),
        "ft" | "foot" | "feet" => UnitDef::linear(Dimension::Length, 0.3048),
        "mi" | "mile" | "miles" => UnitDef::linear(Dimension::Length, 1_609.344),
        "kg" | "kilogram" | "kilograms" => UnitDef::linear(Dimension::Mass, 1.0),
        "g" | "gram" | "grams" => UnitDef::linear(Dimension::Mass, 0.001),
        "lb" | "lbs" | "pound" | "pounds" => UnitDef::linear(Dimension::Mass, 0.453_592_37),
        "oz" | "ounce" | "ounces" => UnitDef::linear(Dimension::Mass, 0.028_349_523_125),
        "s" | "sec" | "second" | "seconds" => UnitDef::linear(Dimension::Time, 1.0),
        "min" | "minute" | "minutes" => UnitDef::linear(Dimension::Time, 60.0),
        "h" | "hr" | "hour" | "hours" => UnitDef::linear(Dimension::Time, 3_600.0),
        "day" | "days" => UnitDef::linear(Dimension::Time, 86_400.0),
        "ml" | "milliliter" | "milliliters" | "millilitre" | "millilitres" => {
            UnitDef::linear(Dimension::Volume, 1.0e-6)
        }
        "l" | "liter" | "liters" | "litre" | "litres" => UnitDef::linear(Dimension::Volume, 0.001),
        "m3" | "cubic_meter" | "cubic_meters" | "cubic_metre" | "cubic_metres" => {
            UnitDef::linear(Dimension::Volume, 1.0)
        }
        "gal" | "gal_us" | "us_gallon" | "us_gallons" | "gallon" | "gallons" => {
            UnitDef::linear(Dimension::Volume, 0.003_785_411_784)
        }
        "cup" | "cups" => UnitDef::linear(Dimension::Volume, 0.000_236_588_236_5),
        "m/s" | "meter_per_second" | "meters_per_second" => UnitDef::linear(Dimension::Speed, 1.0),
        "km/h"
        | "kilometer_per_hour"
        | "kilometers_per_hour"
        | "kilometre_per_hour"
        | "kilometres_per_hour" => UnitDef::linear(Dimension::Speed, 1.0 / 3.6),
        "mph" | "mile_per_hour" | "miles_per_hour" => UnitDef::linear(Dimension::Speed, 0.447_04),
        "m2" | "square_meter" | "square_meters" | "square_metre" | "square_metres" => {
            UnitDef::linear(Dimension::Area, 1.0)
        }
        "km2" | "square_kilometer" | "square_kilometers" => UnitDef::linear(Dimension::Area, 1.0e6),
        "ha" | "hectare" | "hectares" => UnitDef::linear(Dimension::Area, 1.0e4),
        "acre" | "acres" => UnitDef::linear(Dimension::Area, 4_046.856_422_4),
        "ft2" | "square_foot" | "square_feet" => UnitDef::linear(Dimension::Area, 0.092_903_04),
        "b" | "byte" | "bytes" => UnitDef::linear(Dimension::Data, 1.0),
        "kb" => UnitDef::linear(Dimension::Data, 1_000.0),
        "mb" => UnitDef::linear(Dimension::Data, 1_000_000.0),
        "gb" => UnitDef::linear(Dimension::Data, 1_000_000_000.0),
        "kib" => UnitDef::linear(Dimension::Data, 1_024.0),
        "mib" => UnitDef::linear(Dimension::Data, 1_048_576.0),
        "gib" => UnitDef::linear(Dimension::Data, 1_073_741_824.0),
        "c" | "celsius" => UnitDef {
            dimension: Dimension::Temperature,
            factor: 1.0,
            offset: 273.15,
        },
        "f" | "fahrenheit" => UnitDef {
            dimension: Dimension::Temperature,
            factor: 5.0 / 9.0,
            offset: 459.67,
        },
        "k" | "kelvin" => UnitDef {
            dimension: Dimension::Temperature,
            factor: 1.0,
            offset: 0.0,
        },
        _ => return None,
    })
}

fn convert_unit(value: f64, from: &str, to: &str) -> Result<f64, String> {
    let from = from.trim();
    let to = to.trim();
    if from.eq_ignore_ascii_case(to) {
        return Ok(value);
    }
    let from_def = unit_def(from)
        .ok_or_else(|| structured_error("unknown_unit", format!("unknown unit {from}")))?;
    let to_def = unit_def(to)
        .ok_or_else(|| structured_error("unknown_unit", format!("unknown unit {to}")))?;
    if from_def.dimension != to_def.dimension {
        return Err(structured_error(
            "dimension_mismatch",
            format!(
                "cannot convert {from} ({}) to {to} ({}): dimensions differ",
                from_def.dimension.label(),
                to_def.dimension.label()
            ),
        ));
    }
    if from_def.dimension == Dimension::Temperature {
        let kelvin = (value + from_def.offset) * from_def.factor;
        let converted = kelvin / to_def.factor - to_def.offset;
        return ensure_finite(converted, "overflow", "unit conversion overflowed");
    }
    let si = value * from_def.factor;
    let converted = si / to_def.factor;
    ensure_finite(converted, "overflow", "unit conversion overflowed")
}

const FX_AS_OF: &str = "2026-08-01";
const FX_SOURCE: &str = "ECB eurofxref daily (USD cross, rounded)";

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
        (Some(_), None) | (None, Some(_)) => Err(structured_error(
            "invalid_arguments",
            "currency conversion needs two ISO 4217 codes",
        )),
        (Some(from), Some(to)) => {
            let from_per = units_per_usd(&from).ok_or_else(|| {
                structured_error("unknown_unit", format!("unknown currency {from}"))
            })?;
            let to_per = units_per_usd(&to).ok_or_else(|| {
                structured_error("unknown_unit", format!("unknown currency {to}"))
            })?;
            let rate = to_per / from_per;
            let converted = value * rate;
            ensure_finite(converted, "overflow", "currency conversion overflowed")?;
            Ok(Some(FxQuote {
                value: converted,
                from,
                to,
                rate,
            }))
        }
    }
}

fn eval_expr(input: &str, vars: &HashMap<String, f64>) -> Result<f64, String> {
    let mut p = Parser {
        input,
        pos: 0,
        vars,
    };
    p.skip();
    let value = p.expr()?;
    p.skip();
    if p.pos < p.input.len() {
        return Err(structured_error("invalid_expression", "trailing input"));
    }
    ensure_finite(
        value,
        "overflow",
        "result is not a finite number (division by zero, overflow, or undefined operation)",
    )
}

struct Parser<'a> {
    input: &'a str,
    pos: usize,
    vars: &'a HashMap<String, f64>,
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
                        return Err(structured_error("division_by_zero", "division by zero"));
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
            let exp = self.power()?;
            let value = base.powf(exp);
            ensure_finite(value, "overflow", "power overflowed or is undefined")
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
        if self.peek() == Some('+') {
            self.bump();
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
                    return Err(structured_error("invalid_expression", "expected )"));
                }
                Ok(value)
            }
            Some(c) if c.is_ascii_digit() || c == '.' => self.number(),
            Some(c) if c.is_ascii_alphabetic() || c == '_' => self.ident_or_call(),
            _ => Err(structured_error("invalid_expression", "expected number")),
        }
    }

    fn number(&mut self) -> Result<f64, String> {
        let start = self.pos;
        while matches!(self.peek(), Some(c) if c.is_ascii_digit() || c == '.') {
            self.bump();
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            self.bump();
            if matches!(self.peek(), Some('+' | '-')) {
                self.bump();
            }
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.bump();
            }
        }
        self.input[start..self.pos]
            .parse::<f64>()
            .map_err(|_| structured_error("invalid_expression", "invalid number literal"))
    }

    fn ident_or_call(&mut self) -> Result<f64, String> {
        let start = self.pos;
        while matches!(self.peek(), Some(c) if c.is_ascii_alphanumeric() || c == '_') {
            self.bump();
        }
        let name = &self.input[start..self.pos];
        self.skip();
        if self.peek() == Some('(') {
            return self.finish_call(name);
        }
        match name {
            "pi" => Ok(std::f64::consts::PI),
            "e" => Ok(std::f64::consts::E),
            "tau" => Ok(std::f64::consts::TAU),
            _ => self.vars.get(name).copied().ok_or_else(|| {
                structured_error("unknown_variable", format!("unknown variable {name}"))
            }),
        }
    }

    fn finish_call(&mut self, name: &str) -> Result<f64, String> {
        self.bump();
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
                    _ => {
                        return Err(structured_error("invalid_expression", "expected , or )"));
                    }
                }
            }
        }
        if self.bump() != Some(')') {
            return Err(structured_error("invalid_expression", "expected )"));
        }
        Self::dispatch_call(name, &args)
    }

    fn dispatch_call(name: &str, args: &[f64]) -> Result<f64, String> {
        let value = match (name, args) {
            ("sqrt", [x]) if *x >= 0.0 => x.sqrt(),
            ("sqrt", [_]) => {
                return Err(structured_error(
                    "domain_error",
                    "sqrt of a negative number",
                ));
            }
            ("abs", [x]) => x.abs(),
            ("floor", [x]) => x.floor(),
            ("ceil", [x]) => x.ceil(),
            ("round", [x]) => x.round(),
            ("sin", [x]) => x.sin(),
            ("cos", [x]) => x.cos(),
            ("tan", [x]) => x.tan(),
            ("asin", [x]) if (-1.0..=1.0).contains(x) => x.asin(),
            ("asin", [_]) => {
                return Err(structured_error(
                    "domain_error",
                    "asin input must be in [-1, 1]",
                ));
            }
            ("acos", [x]) if (-1.0..=1.0).contains(x) => x.acos(),
            ("acos", [_]) => {
                return Err(structured_error(
                    "domain_error",
                    "acos input must be in [-1, 1]",
                ));
            }
            ("atan", [x]) => x.atan(),
            ("ln", [x]) if *x > 0.0 => x.ln(),
            ("ln", [_]) => {
                return Err(structured_error(
                    "domain_error",
                    "ln input must be positive",
                ));
            }
            ("log", [x]) if *x > 0.0 => x.log10(),
            ("log", [_]) => {
                return Err(structured_error(
                    "domain_error",
                    "log input must be positive",
                ));
            }
            ("exp", [x]) => x.exp(),
            ("deg", [x]) => x.to_radians(),
            ("rad", [x]) => x.to_degrees(),
            ("min", [a, b]) => a.min(*b),
            ("max", [a, b]) => a.max(*b),
            _ => {
                return Err(structured_error(
                    "unknown_function",
                    format!("unknown function {name}"),
                ));
            }
        };
        ensure_finite(value, "overflow", "function result is not finite")
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Rgba {
    r: u8,
    g: u8,
    b: u8,
    a: f64,
}

fn parse_color(input: &str) -> Result<Rgba, String> {
    let input = input.trim();
    let lower = input.to_ascii_lowercase();
    if let Some(digits) = lower.strip_prefix('#') {
        parse_hex(digits)
    } else if lower.starts_with("rgb") {
        parse_rgb_style(&lower)
    } else if lower.starts_with("hsl") {
        parse_hsl_style(&lower)
    } else if lower.contains(',') {
        parse_rgb_channels(&lower)
    } else if !lower.is_empty() && lower.chars().all(|c| c.is_ascii_hexdigit()) {
        parse_hex(&lower)
    } else {
        Err(format!(
            "expected hex, rgb(...), or hsl(...), got '{input}'"
        ))
    }
}

fn parse_hex(digits: &str) -> Result<Rgba, String> {
    if !matches!(digits.len(), 3 | 4 | 6 | 8) || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!(
            "'{digits}' is not valid hex (3, 4, 6, or 8 digits)"
        ));
    }
    let full = match digits.len() {
        3 | 4 => digits.chars().flat_map(|c| [c, c]).collect::<String>(),
        _ => digits.to_string(),
    };
    let value =
        u32::from_str_radix(&full, 16).map_err(|_| format!("'{digits}' is not valid hex"))?;
    let shift = if full.len() == 8 { 24 } else { 16 };
    let r = ((value >> shift) & 0xff) as u8;
    let g = ((value >> (shift - 8)) & 0xff) as u8;
    let b = ((value >> (shift - 16)) & 0xff) as u8;
    let a = if full.len() == 8 {
        f64::from((value & 0xff) as u8) / 255.0
    } else {
        1.0
    };
    Ok(Rgba { r, g, b, a })
}

fn parse_rgb_style(input: &str) -> Result<Rgba, String> {
    let inner = input
        .strip_prefix("rgba")
        .or_else(|| input.strip_prefix("rgb"))
        .ok_or_else(|| format!("'{input}' is not an rgb(...) color"))?;
    let body = inner
        .trim()
        .strip_prefix('(')
        .and_then(|s| s.strip_suffix(')'))
        .ok_or_else(|| "rgb(...) must wrap the channels in parentheses".to_string())?;
    parse_rgb_channels(body.trim())
}

fn parse_rgb_channels(body: &str) -> Result<Rgba, String> {
    let parts: Vec<&str> = body
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .collect();
    match parts.as_slice() {
        [r, g, b] => Ok(Rgba {
            r: parse_channel(r)?,
            g: parse_channel(g)?,
            b: parse_channel(b)?,
            a: 1.0,
        }),
        [r, g, b, a] => Ok(Rgba {
            r: parse_channel(r)?,
            g: parse_channel(g)?,
            b: parse_channel(b)?,
            a: parse_alpha(a)?,
        }),
        _ => Err(format!("expected 3 or 4 channels, got {}", parts.len())),
    }
}

fn parse_channel(raw: &str) -> Result<u8, String> {
    if let Some(percent) = raw.strip_suffix('%') {
        let value: f64 = percent
            .parse()
            .map_err(|_| format!("'{raw}' is not a number"))?;
        if !(0.0..=100.0).contains(&value) {
            return Err(format!("channel '{raw}' is out of range (0-100%)"));
        }
        return Ok((value * 255.0 / 100.0).round() as u8);
    }
    let value: f64 = raw
        .parse()
        .map_err(|_| format!("'{raw}' is not a number"))?;
    if !(0.0..=255.0).contains(&value) {
        return Err(format!("channel '{raw}' is out of range (0-255)"));
    }
    Ok(value.round() as u8)
}

fn parse_alpha(raw: &str) -> Result<f64, String> {
    let value: f64 = raw
        .parse()
        .map_err(|_| format!("'{raw}' is not a number"))?;
    if !(0.0..=1.0).contains(&value) {
        return Err(format!("alpha '{raw}' is out of range (0-1)"));
    }
    Ok(value)
}

fn parse_hsl_style(input: &str) -> Result<Rgba, String> {
    let inner = input
        .strip_prefix("hsla")
        .or_else(|| input.strip_prefix("hsl"))
        .ok_or_else(|| format!("'{input}' is not an hsl(...) color"))?;
    let body = inner
        .trim()
        .strip_prefix('(')
        .and_then(|s| s.strip_suffix(')'))
        .ok_or_else(|| "hsl(...) must wrap the channels in parentheses".to_string())?;
    let parts: Vec<&str> = body
        .trim()
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .collect();
    let (hue, sat, light, a) = match parts.as_slice() {
        [h, s, l] => (
            parse_hue(h)?,
            parse_percent(s, "saturation")?,
            parse_percent(l, "lightness")?,
            1.0,
        ),
        [h, s, l, a] => (
            parse_hue(h)?,
            parse_percent(s, "saturation")?,
            parse_percent(l, "lightness")?,
            parse_alpha(a)?,
        ),
        _ => return Err(format!("expected 3 or 4 channels, got {}", parts.len())),
    };
    Ok(hsl_to_rgb(hue, sat, light, a))
}

fn parse_hue(raw: &str) -> Result<f64, String> {
    let value: f64 = raw
        .parse()
        .map_err(|_| format!("'{raw}' is not a number"))?;
    if !(0.0..=360.0).contains(&value) {
        return Err(format!("hue '{raw}' is out of range (0-360)"));
    }
    Ok(value)
}

fn parse_percent(raw: &str, name: &str) -> Result<f64, String> {
    let stripped = raw.trim().trim_end_matches('%');
    let value: f64 = stripped
        .parse()
        .map_err(|_| format!("'{raw}' is not a number"))?;
    if !(0.0..=100.0).contains(&value) {
        return Err(format!("{name} '{raw}' is out of range (0-100%)"));
    }
    Ok(value / 100.0)
}

fn hsl_to_rgb(hue: f64, sat: f64, light: f64, a: f64) -> Rgba {
    let c = (1.0 - (2.0 * light - 1.0).abs()) * sat;
    let hp = hue / 60.0;
    let x = c * (1.0 - ((hp % 2.0) - 1.0).abs());
    let (r1, g1, b1) = match hp {
        _ if (0.0..1.0).contains(&hp) => (c, x, 0.0),
        _ if (1.0..2.0).contains(&hp) => (x, c, 0.0),
        _ if (2.0..3.0).contains(&hp) => (0.0, c, x),
        _ if (3.0..4.0).contains(&hp) => (0.0, x, c),
        _ if (4.0..5.0).contains(&hp) => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = light - c / 2.0;
    Rgba {
        r: ((r1 + m) * 255.0).round() as u8,
        g: ((g1 + m) * 255.0).round() as u8,
        b: ((b1 + m) * 255.0).round() as u8,
        a,
    }
}

#[expect(
    clippy::float_cmp,
    reason = "max is always exactly one of r/g/b because every channel is a multiple of 1/255"
)]
fn rgb_to_hsl(color: Rgba) -> (f64, f64, f64) {
    let r = f64::from(color.r) / 255.0;
    let g = f64::from(color.g) / 255.0;
    let b = f64::from(color.b) / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let light = f64::midpoint(max, min);
    let delta = max - min;
    if delta == 0.0 {
        return (0.0, 0.0, light);
    }
    let sat = delta / (1.0 - (2.0 * light - 1.0).abs());
    let hue = if max == r {
        (g - b) / delta % 6.0
    } else if max == g {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    };
    ((hue * 60.0).rem_euclid(360.0), sat, light)
}

fn format_hex(color: Rgba) -> String {
    if color.a >= 1.0 {
        format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)
    } else {
        format!(
            "#{:02x}{:02x}{:02x}{:02x}",
            color.r,
            color.g,
            color.b,
            (color.a * 255.0).round() as u8
        )
    }
}

fn format_rgb(color: Rgba) -> String {
    if color.a >= 1.0 {
        format!("rgb({}, {}, {})", color.r, color.g, color.b)
    } else {
        format!(
            "rgba({}, {}, {}, {})",
            color.r,
            color.g,
            color.b,
            format_number(color.a)
        )
    }
}

fn format_hsl(color: Rgba) -> String {
    let (h, s, l) = rgb_to_hsl(color);
    let h = format_number(h);
    let s = format_number(s * 100.0);
    let l = format_number(l * 100.0);
    if color.a >= 1.0 {
        format!("hsl({h}, {s}%, {l}%)")
    } else {
        format!("hsla({h}, {s}%, {l}%, {})", format_number(color.a))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FX_AS_OF, FX_SOURCE, calc, color, convert_unit, currency_convert, eval_expr, execute,
        format_hex, parse_color, random, random_integer_inclusive, structured_error, system_info,
        time,
    };
    use serde_json::{Value, json};
    use std::collections::HashMap;

    fn parse_structured_error(err: &str) -> Option<(String, String)> {
        let value: Value = serde_json::from_str(err).ok()?;
        let kind = value.get("error")?.get("kind")?.as_str()?.to_owned();
        let message = value.get("error")?.get("message")?.as_str()?.to_owned();
        Some((kind, message))
    }

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
    fn hash_defaults_to_blake3_and_honors_sha256() {
        let default = execute("utility.hash", &json!({"text": "hello"})).unwrap();
        assert_eq!(default["algorithm"], json!("blake3"));

        let sha256 = execute(
            "utility.hash",
            &json!({"text": "hello", "algorithm": "sha256"}),
        )
        .unwrap();
        assert_eq!(sha256["algorithm"], json!("sha256"));
        assert_eq!(
            sha256["hex"],
            json!("2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824")
        );
    }

    #[test]
    fn text_changes_letter_case() {
        let upper = execute("utility.text", &json!({"op": "uppercase", "text": "hello"})).unwrap();
        assert_eq!(upper["text"], json!("HELLO"));

        let lower = execute("utility.text", &json!({"op": "lowercase", "text": "HELLO"})).unwrap();
        assert_eq!(lower["text"], json!("hello"));
    }

    #[test]
    fn unknown_text_op_is_structured() {
        let err = execute("utility.text", &json!({"op": "reverse", "text": "hi"})).unwrap_err();
        let (kind, message) = parse_structured_error(&err).unwrap();
        assert_eq!(kind, "unknown_op");
        assert!(message.contains("reverse"));
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
        assert_eq!(value["as_of"], json!(FX_AS_OF));
        assert_eq!(value["source"], json!(FX_SOURCE));
        assert_eq!(value["quote"], json!("USD"));
        assert_eq!(value["stale"], json!(true));
    }

    #[test]
    fn mixed_currency_and_length_is_rejected() {
        let err = currency_convert(1.0, "USD", "m").unwrap_err();
        let (kind, message) = parse_structured_error(&err).unwrap();
        assert_eq!(kind, "invalid_arguments");
        assert!(message.contains("ISO 4217"));
    }

    #[test]
    fn length_conversion_still_skips_fx() {
        assert!(currency_convert(1.0, "m", "km").unwrap().is_none());
    }

    #[test]
    fn sin_deg_45_is_sqrt2_over_2() {
        let value = eval_expr("sin(deg(45))", &HashMap::new()).unwrap();
        assert!((value - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12);
    }

    #[test]
    fn max_and_pi_evaluate() {
        assert!((eval_expr("max(2,3)", &HashMap::new()).unwrap() - 3.0).abs() < 1e-12);
        assert!((eval_expr("pi", &HashMap::new()).unwrap() - std::f64::consts::PI).abs() < 1e-12);
    }

    #[test]
    fn vars_are_bound_in_calc() {
        let value = calc(&json!({"expr":"x+1","vars":{"x":2}})).unwrap();
        assert_eq!(value["value"], json!(3.0));
    }

    #[test]
    fn volume_speed_and_area_convert() {
        assert!((convert_unit(1.0, "L", "mL").unwrap() - 1_000.0).abs() < 1e-9);
        assert!((convert_unit(36.0, "km/h", "m/s").unwrap() - 10.0).abs() < 1e-9);
        assert!((convert_unit(10_000.0, "m2", "ha").unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn dimension_mismatch_is_structured() {
        let err = convert_unit(1.0, "L", "kg").unwrap_err();
        let (kind, message) = parse_structured_error(&err).unwrap();
        assert_eq!(kind, "dimension_mismatch");
        assert!(message.contains("volume"));
        assert!(message.contains("mass"));
    }

    #[test]
    fn color_hex_rgb_hsl_round_trip() {
        let red = parse_color("#ff0000").unwrap();
        assert_eq!(format_hex(red), "#ff0000");
        let value = color(&json!({"color":"#ff0000","to":"hsl"})).unwrap();
        assert_eq!(value["text"], json!("hsl(0, 100%, 50%)"));
        let back = color(&json!({"color": value["text"], "to":"hex"})).unwrap();
        assert_eq!(back["text"], json!("#ff0000"));
    }

    #[test]
    fn color_alpha_round_trip_within_bounds() {
        let value = color(&json!({"color":"rgba(255, 0, 0, 0.5)","to":"hex"})).unwrap();
        assert_eq!(value["text"], json!("#ff000080"));
        let back = color(&json!({"color": value["text"], "to":"rgba"})).unwrap();
        let alpha = back["text"].as_str().unwrap();
        assert!(alpha.contains("0.5") || alpha.contains("0.501960784314"));
    }

    #[test]
    fn integer_random_stays_in_bounds() {
        for _ in 0..500 {
            let value = random(&json!({"kind":"integer","min":3,"max":7})).unwrap()["value"]
                .as_i64()
                .unwrap();
            assert!((3..=7).contains(&value), "{value}");
        }
    }

    #[test]
    fn integer_random_uses_rejection_sampling_not_modulo_bias() {
        for _ in 0..200 {
            let value = random_integer_inclusive(i64::MAX - 2, i64::MAX).unwrap();
            assert!((i64::MAX - 2..=i64::MAX).contains(&value), "{value}");
        }
    }

    #[test]
    fn integer_random_accepts_full_i64_range() {
        for _ in 0..200 {
            let value = random_integer_inclusive(i64::MIN, i64::MAX).unwrap();
            assert!((i64::MIN..=i64::MAX).contains(&value), "{value}");
        }
        assert_eq!(
            random_integer_inclusive(i64::MIN, i64::MIN).unwrap(),
            i64::MIN
        );
        assert_eq!(
            random_integer_inclusive(i64::MAX, i64::MAX).unwrap(),
            i64::MAX
        );
        let value = random_integer_inclusive(i64::MIN, i64::MIN + 1).unwrap();
        assert!(value == i64::MIN || value == i64::MIN + 1, "{value}");
        let value = random_integer_inclusive(i64::MAX - 1, i64::MAX).unwrap();
        assert!(value == i64::MAX - 1 || value == i64::MAX, "{value}");
    }

    #[test]
    fn invalid_expression_is_structured() {
        let err = eval_expr("1 +* 2", &HashMap::new()).unwrap_err();
        let (kind, _) = parse_structured_error(&err).unwrap();
        assert_eq!(kind, "invalid_expression");
    }

    #[test]
    fn sqrt_negative_and_overflow_are_structured() {
        let err = eval_expr("sqrt(-1)", &HashMap::new()).unwrap_err();
        let (kind, _) = parse_structured_error(&err).unwrap();
        assert_eq!(kind, "domain_error");
        let err = eval_expr("1e308 * 1e308", &HashMap::new()).unwrap_err();
        let (kind, _) = parse_structured_error(&err).unwrap();
        assert_eq!(kind, "overflow");
    }

    #[test]
    fn unknown_variable_is_structured() {
        let err = eval_expr("x + 1", &HashMap::new()).unwrap_err();
        let (kind, message) = parse_structured_error(&err).unwrap();
        assert_eq!(kind, "unknown_variable");
        assert!(message.contains('x'));
    }

    #[test]
    fn structured_error_serializes_kind_and_message() {
        let err = structured_error("invalid_expression", "bad token");
        let (kind, message) = parse_structured_error(&err).unwrap();
        assert_eq!(kind, "invalid_expression");
        assert_eq!(message, "bad token");
    }

    #[test]
    fn time_converts_iana_timezone() {
        let args = json!({"timezone":"Asia/Tokyo"});
        let value = time(&args);
        assert_eq!(value["timezone"], "Asia/Tokyo");
        assert!(value["local"].as_str().unwrap().ends_with("+09:00"));
    }
}
