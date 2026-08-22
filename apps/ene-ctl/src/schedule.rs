use std::str::FromStr;

use cron::Schedule as Cron;

/// Accept a 5- or 6-field cron expression (5 fields get a leading `0` seconds).
///
/// # Errors
///
/// Returns a reason when the field count is wrong or `cron` rejects the spec.
pub fn validate_cron_spec(spec: &str) -> Result<(), String> {
    let fields = spec.split_whitespace().count();
    if fields != 5 && fields != 6 {
        return Err(format!("cron spec must have 5 or 6 fields, got {fields}"));
    }
    let cron_spec = if fields == 5 {
        format!("0 {spec}")
    } else {
        spec.to_owned()
    };
    Cron::from_str(&cron_spec).map_err(|err| err.to_string())?;
    Ok(())
}

/// Accept `UTC` / `GMT` (any case) or an IANA timezone name.
///
/// # Errors
///
/// Returns a reason when `tz` is not a known zone.
pub fn validate_timezone(tz: &str) -> Result<(), String> {
    if tz.eq_ignore_ascii_case("utc") || tz.eq_ignore_ascii_case("gmt") {
        return Ok(());
    }
    tz.parse::<chrono_tz::Tz>()
        .map(|_| ())
        .map_err(|_| format!("unknown timezone {tz}"))
}

/// Accept `remind`, `job`, or `turn`.
///
/// # Errors
///
/// Returns a reason for any other action name.
pub fn validate_action(action: &str) -> Result<(), String> {
    match action {
        "remind" | "job" | "turn" => Ok(()),
        other => Err(format!("action must be remind, job, or turn (got {other})")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn five_and_six_field_cron_ok() {
        validate_cron_spec("0 9 * * *").unwrap();
        validate_cron_spec("0 0 9 * * *").unwrap();
        validate_cron_spec("* * * * *").unwrap();
    }

    #[test]
    fn cron_rejects_wrong_arity_and_garbage() {
        assert!(validate_cron_spec("0 9 * *").is_err());
        assert!(validate_cron_spec("not a cron").is_err());
        assert!(validate_cron_spec("").is_err());
    }

    #[test]
    fn timezone_accepts_iana_and_utc() {
        validate_timezone("UTC").unwrap();
        validate_timezone("utc").unwrap();
        validate_timezone("Asia/Tokyo").unwrap();
        assert!(validate_timezone("Not/AZone").is_err());
        assert!(validate_timezone("").is_err());
    }

    #[test]
    fn action_is_closed_set() {
        validate_action("remind").unwrap();
        validate_action("job").unwrap();
        validate_action("turn").unwrap();
        assert!(validate_action("email").is_err());
    }
}
