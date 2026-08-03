use ene_plugin::prelude::*;

use super::format_number;

/// A unit definition anchored to its SI base unit.
///
/// Converting `value` in this unit to SI: `si = (value + offset) * factor`.
/// `offset` is nonzero only for temperature scales whose zero point does
/// not coincide with the SI scale (Celsius/Kelvin: 273.15, Fahrenheit:
/// 459.67), so affine conversion is exact in both directions.
#[derive(Debug, Clone, Copy)]
struct UnitDef {
    factor: f64,
    offset: f64,
}

/// Static unit catalog: canonical name, aliases, and the SI anchor.
///
/// Factors are exact by definition (SI prefixes, imperial inch/pound,
/// US/UK gallons, US survey-free acre, Celsius/Fahrenheit points) rather
/// than rounded approximations.
const UNITS: &[(&str, &[&str], UnitDef)] = &[
    // Length (m)
    (
        "mm",
        &["millimeter", "millimeters", "millimetre", "millimetres"],
        UnitDef {
            factor: 0.001,
            offset: 0.0,
        },
    ),
    (
        "cm",
        &["centimeter", "centimeters", "centimetre", "centimetres"],
        UnitDef {
            factor: 0.01,
            offset: 0.0,
        },
    ),
    (
        "m",
        &["meter", "meters", "metre", "metres"],
        UnitDef {
            factor: 1.0,
            offset: 0.0,
        },
    ),
    (
        "km",
        &["kilometer", "kilometers", "kilometre", "kilometres"],
        UnitDef {
            factor: 1000.0,
            offset: 0.0,
        },
    ),
    (
        "in",
        &["inch", "inches"],
        UnitDef {
            factor: 0.0254,
            offset: 0.0,
        },
    ),
    (
        "ft",
        &["foot", "feet"],
        UnitDef {
            factor: 0.3048,
            offset: 0.0,
        },
    ),
    (
        "yd",
        &["yard", "yards"],
        UnitDef {
            factor: 0.9144,
            offset: 0.0,
        },
    ),
    (
        "mi",
        &["mile", "miles"],
        UnitDef {
            factor: 1609.344,
            offset: 0.0,
        },
    ),
    (
        "nmi",
        &["nautical_mile", "nautical_miles"],
        UnitDef {
            factor: 1852.0,
            offset: 0.0,
        },
    ),
    // Mass (kg)
    (
        "mg",
        &["milligram", "milligrams"],
        UnitDef {
            factor: 1.0e-6,
            offset: 0.0,
        },
    ),
    (
        "g",
        &["gram", "grams", "gramme", "grammes"],
        UnitDef {
            factor: 0.001,
            offset: 0.0,
        },
    ),
    (
        "kg",
        &["kilogram", "kilograms", "kilogramme", "kilogrammes"],
        UnitDef {
            factor: 1.0,
            offset: 0.0,
        },
    ),
    (
        "t",
        &["tonne", "tonnes", "metric_ton", "metric_tons"],
        UnitDef {
            factor: 1000.0,
            offset: 0.0,
        },
    ),
    (
        "oz",
        &["ounce", "ounces"],
        UnitDef {
            factor: 0.028_349_523_125,
            offset: 0.0,
        },
    ),
    (
        "lb",
        &["pound", "pounds"],
        UnitDef {
            factor: 0.453_592_37,
            offset: 0.0,
        },
    ),
    // Time (s)
    (
        "ms",
        &["millisecond", "milliseconds"],
        UnitDef {
            factor: 0.001,
            offset: 0.0,
        },
    ),
    (
        "s",
        &["second", "seconds", "sec"],
        UnitDef {
            factor: 1.0,
            offset: 0.0,
        },
    ),
    (
        "min",
        &["minute", "minutes"],
        UnitDef {
            factor: 60.0,
            offset: 0.0,
        },
    ),
    (
        "h",
        &["hour", "hours", "hr"],
        UnitDef {
            factor: 3600.0,
            offset: 0.0,
        },
    ),
    (
        "day",
        &["days"],
        UnitDef {
            factor: 86_400.0,
            offset: 0.0,
        },
    ),
    (
        "week",
        &["weeks"],
        UnitDef {
            factor: 604_800.0,
            offset: 0.0,
        },
    ),
    // Volume (m³)
    (
        "ml",
        &["milliliter", "milliliters", "millilitre", "millilitres"],
        UnitDef {
            factor: 1.0e-6,
            offset: 0.0,
        },
    ),
    (
        "l",
        &["liter", "liters", "litre", "litres"],
        UnitDef {
            factor: 0.001,
            offset: 0.0,
        },
    ),
    (
        "m3",
        &["cubic_meter", "cubic_meters", "cubic_metre", "cubic_metres"],
        UnitDef {
            factor: 1.0,
            offset: 0.0,
        },
    ),
    (
        "gal_us",
        &["us_gallon", "us_gallons", "gallon_us"],
        UnitDef {
            factor: 0.003_785_411_784,
            offset: 0.0,
        },
    ),
    (
        "gal_uk",
        &[
            "uk_gallon",
            "uk_gallons",
            "gallon_uk",
            "imperial_gallon",
            "imperial_gallons",
        ],
        UnitDef {
            factor: 0.004_546_09,
            offset: 0.0,
        },
    ),
    // Speed (m/s)
    (
        "m/s",
        &[
            "meter_per_second",
            "meters_per_second",
            "metre_per_second",
            "metres_per_second",
            "mps",
        ],
        UnitDef {
            factor: 1.0,
            offset: 0.0,
        },
    ),
    (
        "km/h",
        &[
            "kilometer_per_hour",
            "kilometers_per_hour",
            "kilometre_per_hour",
            "kilometres_per_hour",
            "kph",
        ],
        UnitDef {
            factor: 1.0 / 3.6,
            offset: 0.0,
        },
    ),
    (
        "mph",
        &["mile_per_hour", "miles_per_hour"],
        UnitDef {
            factor: 0.447_04,
            offset: 0.0,
        },
    ),
    (
        "knot",
        &["knots", "kt"],
        UnitDef {
            factor: 1852.0 / 3600.0,
            offset: 0.0,
        },
    ),
    (
        "ft/s",
        &["foot_per_second", "feet_per_second", "fps"],
        UnitDef {
            factor: 0.3048,
            offset: 0.0,
        },
    ),
    // Area (m²)
    (
        "mm2",
        &["square_millimeter", "square_millimeters"],
        UnitDef {
            factor: 1.0e-6,
            offset: 0.0,
        },
    ),
    (
        "cm2",
        &["square_centimeter", "square_centimeters"],
        UnitDef {
            factor: 1.0e-4,
            offset: 0.0,
        },
    ),
    (
        "m2",
        &[
            "square_meter",
            "square_meters",
            "square_metre",
            "square_metres",
        ],
        UnitDef {
            factor: 1.0,
            offset: 0.0,
        },
    ),
    (
        "km2",
        &["square_kilometer", "square_kilometers"],
        UnitDef {
            factor: 1.0e6,
            offset: 0.0,
        },
    ),
    (
        "ha",
        &["hectare", "hectares"],
        UnitDef {
            factor: 1.0e4,
            offset: 0.0,
        },
    ),
    (
        "acre",
        &["acres"],
        UnitDef {
            factor: 4_046.856_422_4,
            offset: 0.0,
        },
    ),
    (
        "ft2",
        &["square_foot", "square_feet"],
        UnitDef {
            factor: 0.092_903_04,
            offset: 0.0,
        },
    ),
    // Temperature (K). C/F use an offset so 0 °C == 273.15 K holds.
    (
        "k",
        &["kelvin", "kelvins"],
        UnitDef {
            factor: 1.0,
            offset: 0.0,
        },
    ),
    (
        "c",
        &["celsius", "celsius_degree", "celsius_degrees", "degc"],
        UnitDef {
            factor: 1.0,
            offset: 273.15,
        },
    ),
    (
        "f",
        &[
            "fahrenheit",
            "fahrenheit_degree",
            "fahrenheit_degrees",
            "degf",
        ],
        UnitDef {
            factor: 5.0 / 9.0,
            offset: 459.67,
        },
    ),
    // Data (byte)
    (
        "b",
        &["byte", "bytes"],
        UnitDef {
            factor: 1.0,
            offset: 0.0,
        },
    ),
    (
        "kb",
        &["kilobyte", "kilobytes"],
        UnitDef {
            factor: 1000.0,
            offset: 0.0,
        },
    ),
    (
        "mb",
        &["megabyte", "megabytes"],
        UnitDef {
            factor: 1.0e6,
            offset: 0.0,
        },
    ),
    (
        "gb",
        &["gigabyte", "gigabytes"],
        UnitDef {
            factor: 1.0e9,
            offset: 0.0,
        },
    ),
    (
        "tb",
        &["terabyte", "terabytes"],
        UnitDef {
            factor: 1.0e12,
            offset: 0.0,
        },
    ),
    (
        "kib",
        &["kibibyte", "kibibytes"],
        UnitDef {
            factor: 1024.0,
            offset: 0.0,
        },
    ),
    (
        "mib",
        &["mebibyte", "mebibytes"],
        UnitDef {
            factor: 1_048_576.0,
            offset: 0.0,
        },
    ),
    (
        "gib",
        &["gibibyte", "gibibytes"],
        UnitDef {
            factor: 1_073_741_824.0,
            offset: 0.0,
        },
    ),
    (
        "tib",
        &["tebibyte", "tebibytes"],
        UnitDef {
            factor: 1_099_511_627_776.0,
            offset: 0.0,
        },
    ),
];

fn unit_def(name: &str) -> Option<UnitDef> {
    let key = name.trim().to_ascii_lowercase();
    UNITS
        .iter()
        .find(|(canonical, aliases, _)| {
            key == *canonical || aliases.iter().any(|alias| key == *alias)
        })
        .map(|(_, _, def)| *def)
}

/// Converts a value between two units from the built-in catalog.
///
/// Supported dimensions: length (mm, cm, m, km, in, ft, yd, mi, nmi),
/// mass (mg, g, kg, t, oz, lb), time (ms, s, min, h, day, week),
/// volume (ml, l, m3, `gal_us`, `gal_uk`), speed (m/s, km/h, mph, knot,
/// ft/s), area (mm2, cm2, m2, km2, ha, acre, ft2), temperature
/// (k, c, f), and data (b, kb, mb, gb, tb, kib, mib, gib, tib).
/// Full unit names are accepted as aliases (e.g. "kilometers" for
/// "km"); comparisons are case-insensitive.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "calc",
    name = "unit_convert",
    summary = "Convert a value between two units.",
    description = "Converts a numeric value between units of the same dimension, e.g. 100 km to mi. Supported: length (mm, cm, m, km, in, ft, yd, mi, nmi), mass (mg, g, kg, t, oz, lb), time (ms, s, min, h, day, week), volume (ml, l, m3, gal_us, gal_uk), speed (m/s, km/h, mph, knot, ft/s), area (mm2, cm2, m2, km2, ha, acre, ft2), temperature (k, c, f), data (b, kb, mb, gb, tb, kib, mib, gib, tib). Full names work as aliases (kilometers, miles, fahrenheit, ...); case-insensitive.",
    category = "Utility",
    keywords_primary = "convert, unit, measurement, km, miles, celsius, fahrenheit, inches",
    side_effects = "Idempotent"
)]
pub struct UnitConvertAction {
    /// The numeric value to convert.
    value: f64,
    /// The source unit (e.g. "km").
    from: String,
    /// The target unit (e.g. "mi").
    to: String,
}

impl UnitConvertAction {
    async fn run(&self) -> Result<String, ToolError> {
        convert(self.value, &self.from, &self.to)
    }
}

fn convert(value: f64, from: &str, to: &str) -> Result<String, ToolError> {
    if !value.is_finite() {
        return Err(ToolError::InvalidArguments {
            message: "value must be a finite number".to_string(),
        });
    }
    let from_def = unit_def(from).ok_or_else(|| ToolError::InvalidArguments {
        message: format!("unknown unit '{from}'"),
    })?;
    let to_def = unit_def(to).ok_or_else(|| ToolError::InvalidArguments {
        message: format!("unknown unit '{to}'"),
    })?;

    // SI-anchored affine conversion: source to SI, SI to target.
    let si = (value + from_def.offset) * from_def.factor;
    let result = si / to_def.factor - to_def.offset;

    if !result.is_finite() {
        return Err(ToolError::InvalidArguments {
            message: "conversion result is not a finite number".to_string(),
        });
    }

    Ok(format!(
        "{} {} = {} {}",
        format_number(value),
        from.trim().to_ascii_lowercase(),
        format_number(result),
        to.trim().to_ascii_lowercase(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn convert_test(value: f64, from: &str, to: &str) -> Result<String, ToolError> {
        convert(value, from, to)
    }

    #[test]
    fn length_km_to_miles() {
        assert_eq!(
            convert_test(100.0, "km", "mi").unwrap(),
            "100 km = 62.137119223733 mi"
        );
    }

    #[test]
    fn length_round_trip() {
        let result = convert_test(1.0, "mi", "km").unwrap();
        assert_eq!(result, "1 mi = 1.609344 km");
    }

    #[test]
    fn temperature_celsius_to_fahrenheit() {
        assert_eq!(convert_test(0.0, "c", "f").unwrap(), "0 c = 32 f");
        assert_eq!(convert_test(100.0, "c", "f").unwrap(), "100 c = 212 f");
    }

    #[test]
    fn temperature_fahrenheit_to_celsius() {
        assert_eq!(convert_test(32.0, "f", "c").unwrap(), "32 f = 0 c");
        assert_eq!(convert_test(-40.0, "f", "c").unwrap(), "-40 f = -40 c");
    }

    #[test]
    fn temperature_kelvin() {
        assert_eq!(convert_test(0.0, "c", "k").unwrap(), "0 c = 273.15 k");
        assert_eq!(convert_test(273.15, "k", "c").unwrap(), "273.15 k = 0 c");
    }

    #[test]
    fn mass_and_volume() {
        assert_eq!(
            convert_test(1.0, "kg", "lb").unwrap(),
            "1 kg = 2.204622621849 lb"
        );
        assert_eq!(
            convert_test(1.0, "gal_us", "l").unwrap(),
            "1 gal_us = 3.785411784 l"
        );
    }

    #[test]
    fn aliases_are_accepted() {
        let result = convert_test(100.0, "kilometers", "miles").unwrap();
        assert!(result.contains("62.137"), "{result}");
        let result = convert_test(0.0, "Celsius", "Fahrenheit").unwrap();
        assert!(result.contains("32"), "{result}");
    }

    #[test]
    fn case_insensitive() {
        let result = convert_test(1.0, "KM", "M").unwrap();
        assert!(result.contains("1000"), "{result}");
    }

    #[test]
    fn data_units_binary_and_decimal() {
        assert_eq!(convert_test(1.0, "gib", "mib").unwrap(), "1 gib = 1024 mib");
        assert_eq!(convert_test(1.0, "gb", "mb").unwrap(), "1 gb = 1000 mb");
    }

    #[test]
    fn speed_conversion() {
        let result = convert_test(100.0, "km/h", "mph").unwrap();
        assert!(result.contains("62.137"), "{result}");
    }

    #[test]
    fn unknown_unit_rejected() {
        let err = convert_test(1.0, "parsec", "m").unwrap_err();
        assert!(err.to_string().contains("unknown unit 'parsec'"), "{err}");
        let err = convert_test(1.0, "m", "lightyear").unwrap_err();
        assert!(
            err.to_string().contains("unknown unit 'lightyear'"),
            "{err}"
        );
    }

    #[test]
    fn non_finite_value_rejected() {
        let err = convert_test(f64::NAN, "m", "km").unwrap_err();
        assert!(err.to_string().contains("finite"), "{err}");
    }

    #[test]
    fn spec_name() {
        let action = UnitConvertAction::default();
        assert_eq!(action.name(), "calc.unit_convert");
        assert_eq!(UnitConvertAction::spec().name.as_str(), "calc.unit_convert");
    }
}
