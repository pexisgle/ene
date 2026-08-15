use ene_plugin::prelude::*;

use super::format_number;

/// Physical dimension of a unit. Conversions are only allowed between
/// units of the same dimension; the temperature scales are all tagged
/// `Temperature` so their affine offsets never block each other.
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

/// A unit definition anchored to its SI base unit.
///
/// Converting `value` in this unit to SI: `si = (value + offset) * factor`.
/// `offset` is nonzero only for temperature scales whose zero point does
/// not coincide with the SI scale (Celsius/Kelvin: 273.15, Fahrenheit:
/// 459.67), so affine conversion is exact in both directions.
#[derive(Debug, Clone, Copy)]
struct UnitDef {
    dimension: Dimension,
    factor: f64,
    offset: f64,
}

impl UnitDef {
    /// A unit whose zero point coincides with the SI scale.
    const fn linear(dimension: Dimension, factor: f64) -> Self {
        Self {
            dimension,
            factor,
            offset: 0.0,
        }
    }

    /// A unit whose zero point is shifted from the SI scale.
    const fn affine(dimension: Dimension, factor: f64, offset: f64) -> Self {
        Self {
            dimension,
            factor,
            offset,
        }
    }
}

/// Static unit catalog: canonical name, aliases, and the SI anchor.
///
/// Factors are exact by definition (SI prefixes, imperial inch/pound,
/// US/UK gallons, US survey-free acre, Celsius/Fahrenheit points) rather
/// than rounded approximations.
const UNITS: &[(&str, &[&str], UnitDef)] = &[
    (
        "mm",
        &["millimeter", "millimeters", "millimetre", "millimetres"],
        UnitDef::linear(Dimension::Length, 0.001),
    ),
    (
        "cm",
        &["centimeter", "centimeters", "centimetre", "centimetres"],
        UnitDef::linear(Dimension::Length, 0.01),
    ),
    (
        "m",
        &["meter", "meters", "metre", "metres"],
        UnitDef::linear(Dimension::Length, 1.0),
    ),
    (
        "km",
        &["kilometer", "kilometers", "kilometre", "kilometres"],
        UnitDef::linear(Dimension::Length, 1000.0),
    ),
    (
        "in",
        &["inch", "inches"],
        UnitDef::linear(Dimension::Length, 0.0254),
    ),
    (
        "ft",
        &["foot", "feet"],
        UnitDef::linear(Dimension::Length, 0.3048),
    ),
    (
        "yd",
        &["yard", "yards"],
        UnitDef::linear(Dimension::Length, 0.9144),
    ),
    (
        "mi",
        &["mile", "miles"],
        UnitDef::linear(Dimension::Length, 1609.344),
    ),
    (
        "nmi",
        &["nautical_mile", "nautical_miles"],
        UnitDef::linear(Dimension::Length, 1852.0),
    ),
    (
        "mg",
        &["milligram", "milligrams"],
        UnitDef::linear(Dimension::Mass, 1.0e-6),
    ),
    (
        "g",
        &["gram", "grams", "gramme", "grammes"],
        UnitDef::linear(Dimension::Mass, 0.001),
    ),
    (
        "kg",
        &["kilogram", "kilograms", "kilogramme", "kilogrammes"],
        UnitDef::linear(Dimension::Mass, 1.0),
    ),
    (
        "t",
        &["tonne", "tonnes", "metric_ton", "metric_tons"],
        UnitDef::linear(Dimension::Mass, 1000.0),
    ),
    (
        "oz",
        &["ounce", "ounces"],
        UnitDef::linear(Dimension::Mass, 0.028_349_523_125),
    ),
    (
        "lb",
        &["pound", "pounds"],
        UnitDef::linear(Dimension::Mass, 0.453_592_37),
    ),
    (
        "ms",
        &["millisecond", "milliseconds"],
        UnitDef::linear(Dimension::Time, 0.001),
    ),
    (
        "s",
        &["second", "seconds", "sec"],
        UnitDef::linear(Dimension::Time, 1.0),
    ),
    (
        "min",
        &["minute", "minutes"],
        UnitDef::linear(Dimension::Time, 60.0),
    ),
    (
        "h",
        &["hour", "hours", "hr"],
        UnitDef::linear(Dimension::Time, 3600.0),
    ),
    ("day", &["days"], UnitDef::linear(Dimension::Time, 86_400.0)),
    (
        "week",
        &["weeks"],
        UnitDef::linear(Dimension::Time, 604_800.0),
    ),
    (
        "ml",
        &["milliliter", "milliliters", "millilitre", "millilitres"],
        UnitDef::linear(Dimension::Volume, 1.0e-6),
    ),
    (
        "l",
        &["liter", "liters", "litre", "litres"],
        UnitDef::linear(Dimension::Volume, 0.001),
    ),
    (
        "m3",
        &["cubic_meter", "cubic_meters", "cubic_metre", "cubic_metres"],
        UnitDef::linear(Dimension::Volume, 1.0),
    ),
    (
        "gal_us",
        &["us_gallon", "us_gallons", "gallon_us"],
        UnitDef::linear(Dimension::Volume, 0.003_785_411_784),
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
        UnitDef::linear(Dimension::Volume, 0.004_546_09),
    ),
    (
        "m/s",
        &[
            "meter_per_second",
            "meters_per_second",
            "metre_per_second",
            "metres_per_second",
            "mps",
        ],
        UnitDef::linear(Dimension::Speed, 1.0),
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
        UnitDef::linear(Dimension::Speed, 1.0 / 3.6),
    ),
    (
        "mph",
        &["mile_per_hour", "miles_per_hour"],
        UnitDef::linear(Dimension::Speed, 0.447_04),
    ),
    (
        "knot",
        &["knots", "kt"],
        UnitDef::linear(Dimension::Speed, 1852.0 / 3600.0),
    ),
    (
        "ft/s",
        &["foot_per_second", "feet_per_second", "fps"],
        UnitDef::linear(Dimension::Speed, 0.3048),
    ),
    (
        "mm2",
        &["square_millimeter", "square_millimeters"],
        UnitDef::linear(Dimension::Area, 1.0e-6),
    ),
    (
        "cm2",
        &["square_centimeter", "square_centimeters"],
        UnitDef::linear(Dimension::Area, 1.0e-4),
    ),
    (
        "m2",
        &[
            "square_meter",
            "square_meters",
            "square_metre",
            "square_metres",
        ],
        UnitDef::linear(Dimension::Area, 1.0),
    ),
    (
        "km2",
        &["square_kilometer", "square_kilometers"],
        UnitDef::linear(Dimension::Area, 1.0e6),
    ),
    (
        "ha",
        &["hectare", "hectares"],
        UnitDef::linear(Dimension::Area, 1.0e4),
    ),
    (
        "acre",
        &["acres"],
        UnitDef::linear(Dimension::Area, 4_046.856_422_4),
    ),
    (
        "ft2",
        &["square_foot", "square_feet"],
        UnitDef::linear(Dimension::Area, 0.092_903_04),
    ),
    (
        "k",
        &["kelvin", "kelvins"],
        UnitDef::linear(Dimension::Temperature, 1.0),
    ),
    (
        "c",
        &["celsius", "celsius_degree", "celsius_degrees", "degc"],
        UnitDef::affine(Dimension::Temperature, 1.0, 273.15),
    ),
    (
        "f",
        &[
            "fahrenheit",
            "fahrenheit_degree",
            "fahrenheit_degrees",
            "degf",
        ],
        UnitDef::affine(Dimension::Temperature, 5.0 / 9.0, 459.67),
    ),
    (
        "b",
        &["byte", "bytes"],
        UnitDef::linear(Dimension::Data, 1.0),
    ),
    (
        "kb",
        &["kilobyte", "kilobytes"],
        UnitDef::linear(Dimension::Data, 1000.0),
    ),
    (
        "mb",
        &["megabyte", "megabytes"],
        UnitDef::linear(Dimension::Data, 1.0e6),
    ),
    (
        "gb",
        &["gigabyte", "gigabytes"],
        UnitDef::linear(Dimension::Data, 1.0e9),
    ),
    (
        "tb",
        &["terabyte", "terabytes"],
        UnitDef::linear(Dimension::Data, 1.0e12),
    ),
    (
        "kib",
        &["kibibyte", "kibibytes"],
        UnitDef::linear(Dimension::Data, 1024.0),
    ),
    (
        "mib",
        &["mebibyte", "mebibytes"],
        UnitDef::linear(Dimension::Data, 1_048_576.0),
    ),
    (
        "gib",
        &["gibibyte", "gibibytes"],
        UnitDef::linear(Dimension::Data, 1_073_741_824.0),
    ),
    (
        "tib",
        &["tebibyte", "tebibytes"],
        UnitDef::linear(Dimension::Data, 1_099_511_627_776.0),
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

    if from_def.dimension != to_def.dimension {
        return Err(ToolError::InvalidArguments {
            message: format!(
                "cannot convert '{from}' ({}) to '{to}' ({}): dimensions differ",
                from_def.dimension.label(),
                to_def.dimension.label(),
            ),
        });
    }

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
    fn dimension_mismatch_rejected() {
        let err = convert_test(1.0, "km", "kg").unwrap_err();
        assert!(err.to_string().contains("length"), "{err}");
        assert!(err.to_string().contains("mass"), "{err}");
        let err = convert_test(1.0, "h", "mi").unwrap_err();
        assert!(err.to_string().contains("time"), "{err}");
        assert!(err.to_string().contains("length"), "{err}");
        let err = convert_test(60.0, "m", "min").unwrap_err();
        assert!(err.to_string().contains("length"), "{err}");
        assert!(err.to_string().contains("time"), "{err}");
        let err = convert_test(1.0, "gal_us", "kg").unwrap_err();
        assert!(err.to_string().contains("volume"), "{err}");
        assert!(err.to_string().contains("mass"), "{err}");
    }

    #[test]
    fn temperature_scales_share_dimension() {
        let out = convert_test(32.0, "f", "k").unwrap();
        assert_eq!(out, "32 f = 273.15 k");
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
