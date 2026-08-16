use ene_plugin::prelude::*;

use super::format_number;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Rgba {
    r: u8,
    g: u8,
    b: u8,
    a: f64,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "calc",
    name = "color_convert",
    summary = "Convert a color between hex, rgb, and hsl formats.",
    description = "Parses a color in hex (#ff0000, #f00, with or without #, 3/4/6/8 digits), rgb/rgba (0-255 channels, optional percentages, alpha 0-1), or hsl/hsla (hue 0-360, saturation and lightness 0-100%, alpha 0-1) and returns it in the requested format: hex (#rrggbb or #rrggbbaa when alpha < 1), rgb (rgb(r, g, b)), or hsl (hsl(h, s%, l%)).",
    category = "Utility",
    keywords_primary = "color, hex, rgb, hsl, convert, css, html",
    side_effects = "Idempotent"
)]
pub struct ColorConvertAction {
    color: String,
    #[arg(enum_values = "hex, rgb, rgba, hsl, hsla")]
    to: String,
}

impl ColorConvertAction {
    async fn run(&self) -> Result<String, ToolError> {
        convert_color(&self.color, &self.to)
    }
}

fn convert_color(color: &str, to: &str) -> Result<String, ToolError> {
    let color = parse_color(color).map_err(|e| ToolError::InvalidArguments {
        message: format!("invalid color '{}': {e}", color.trim()),
    })?;
    match to.trim().to_ascii_lowercase().as_str() {
        "hex" => Ok(format_hex(color)),
        "rgb" | "rgba" => Ok(format_rgb(color)),
        "hsl" | "hsla" => Ok(format_hsl(color)),
        other => Err(ToolError::InvalidArguments {
            message: format!("unknown output format '{other}' (expected hex, rgb, or hsl)"),
        }),
    }
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
        parse_rgb_triple(&lower)
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
    // 3/4-digit shorthand doubles every digit; `String::len` is a byte
    // count but hex digits are ASCII, so it equals the char count.
    let full = match digits.len() {
        3 | 4 => digits.chars().flat_map(|c| [c, c]).collect::<String>(),
        _ => digits.to_string(),
    };
    let value =
        u32::from_str_radix(&full, 16).map_err(|_| format!("'{digits}' is not valid hex"))?;
    // 6-digit values occupy 24 bits (red at bit 16); 8-digit values
    // occupy 32 (red at bit 24).
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
        .ok_or_else(|| format!("'{input}' is not an rgb(...) color"))?
        .trim();
    let body = inner
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

fn parse_rgb_triple(input: &str) -> Result<Rgba, String> {
    parse_rgb_channels(input)
}

fn parse_hsl_style(input: &str) -> Result<Rgba, String> {
    let inner = input
        .strip_prefix("hsla")
        .or_else(|| input.strip_prefix("hsl"))
        .ok_or_else(|| format!("'{input}' is not an hsl(...) color"))?
        .trim();
    let body = inner
        .strip_prefix('(')
        .and_then(|s| s.strip_suffix(')'))
        .ok_or_else(|| "hsl(...) must wrap the channels in parentheses".to_string())?;
    let parts: Vec<&str> = body
        .trim()
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .collect();

    let (hue, sat, light, a) = match parts.as_slice() {
        [h, s, l] => {
            let hue = parse_hue(h)?;
            (
                hue,
                parse_percent(s, "saturation")?,
                parse_percent(l, "lightness")?,
                1.0,
            )
        }
        [h, s, l, a] => {
            let hue = parse_hue(h)?;
            (
                hue,
                parse_percent(s, "saturation")?,
                parse_percent(l, "lightness")?,
                parse_alpha(a)?,
            )
        }
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
    let stripped = raw.strip_suffix('%').unwrap_or(raw);
    let value: f64 = stripped
        .parse()
        .map_err(|_| format!("'{raw}' is not a number"))?;
    if !(0.0..=100.0).contains(&value) {
        return Err(format!("{name} '{raw}' is out of range (0-100%)"));
    }
    Ok(value / 100.0)
}

/// CSS Color Module Level 3 hsl→rgb conversion.
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
    use super::*;

    fn convert(color: &str, to: &str) -> Result<String, ToolError> {
        convert_color(color, to)
    }

    #[test]
    fn hex_to_rgb() {
        assert_eq!(convert("#ff0000", "rgb").unwrap(), "rgb(255, 0, 0)");
        assert_eq!(convert("#f00", "rgb").unwrap(), "rgb(255, 0, 0)");
        assert_eq!(convert("ff0000", "rgb").unwrap(), "rgb(255, 0, 0)");
        assert_eq!(convert("#aabbcc", "rgb").unwrap(), "rgb(170, 187, 204)");
    }

    #[test]
    fn rgb_to_hex() {
        assert_eq!(convert("rgb(170, 187, 204)", "hex").unwrap(), "#aabbcc");
        assert_eq!(convert("rgb(255, 0, 0)", "hex").unwrap(), "#ff0000");
    }

    #[test]
    fn alpha_round_trips() {
        assert_eq!(convert("#ff000080", "hex").unwrap(), "#ff000080");
        assert_eq!(convert("rgba(255, 0, 0, 0.5)", "hex").unwrap(), "#ff000080");
        assert_eq!(
            convert("rgba(255, 0, 0, 0.5)", "rgba").unwrap(),
            "rgba(255, 0, 0, 0.5)"
        );
        assert_eq!(
            convert("#f008", "rgb").unwrap(),
            "rgba(255, 0, 0, 0.533333333333)"
        );
    }

    #[test]
    fn hsl_to_rgb_and_back() {
        assert_eq!(convert("hsl(120, 100%, 50%)", "hex").unwrap(), "#00ff00");
        assert_eq!(convert("hsl(0, 0%, 100%)", "hex").unwrap(), "#ffffff");
        assert_eq!(convert("hsl(0, 0%, 0%)", "hex").unwrap(), "#000000");
        assert_eq!(convert("#ff0000", "hsl").unwrap(), "hsl(0, 100%, 50%)");
        assert_eq!(convert("#00ff00", "hsl").unwrap(), "hsl(120, 100%, 50%)");
        assert_eq!(convert("#0000ff", "hsl").unwrap(), "hsl(240, 100%, 50%)");
    }

    #[test]
    fn hsl_with_alpha() {
        assert_eq!(
            convert("hsla(120, 100%, 50%, 0.5)", "hsl").unwrap(),
            "hsla(120, 100%, 50%, 0.5)"
        );
    }

    #[test]
    fn hsl_fractional_hue_rounds() {
        let out = convert("#4a90d9", "hsl").unwrap();
        assert!(out.starts_with("hsl("), "{out}");
    }

    #[test]
    fn percentages_in_rgb() {
        assert_eq!(convert("rgb(100%, 0%, 0%)", "hex").unwrap(), "#ff0000");
    }

    #[test]
    fn bare_rgb_triple() {
        assert_eq!(convert("255, 0, 128", "hex").unwrap(), "#ff0080");
    }

    #[test]
    fn case_insensitive_hex() {
        assert_eq!(convert("#FF0000", "rgb").unwrap(), "rgb(255, 0, 0)");
    }

    #[test]
    fn invalid_inputs_rejected() {
        let err = convert("not a color", "hex").unwrap_err();
        assert!(err.to_string().contains("invalid color"), "{err}");
        let err = convert("#12345", "hex").unwrap_err();
        assert!(err.to_string().contains("invalid color"), "{err}");
        let err = convert("rgb(300, 0, 0)", "hex").unwrap_err();
        assert!(err.to_string().contains("out of range"), "{err}");
        let err = convert("hsl(400, 50%, 50%)", "hex").unwrap_err();
        assert!(err.to_string().contains("out of range"), "{err}");
        let err = convert("hsl(120, 150%, 50%)", "hex").unwrap_err();
        assert!(err.to_string().contains("out of range"), "{err}");
        let err = convert("rgb(1, 2)", "hex").unwrap_err();
        assert!(err.to_string().contains("invalid color"), "{err}");
    }

    #[test]
    fn unknown_output_format_rejected() {
        let err = convert("#ff0000", "cmyk").unwrap_err();
        assert!(err.to_string().contains("cmyk"), "{err}");
    }

    #[test]
    fn spec_name() {
        let action = ColorConvertAction::default();
        assert_eq!(action.name(), "calc.color_convert");
        assert_eq!(
            ColorConvertAction::spec().name.as_str(),
            "calc.color_convert"
        );
    }
}
