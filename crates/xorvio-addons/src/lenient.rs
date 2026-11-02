//! Forgiving readers for add-on JSON. Add-ons differ in which fields they fill
//! and in their types (a year can be `1999` or `"1999–2007"`, a list can be a
//! comma-separated string), so every field is read on its own and a field that
//! doesn't fit is simply missing rather than failing the whole response.

use chrono::{DateTime, NaiveDate};
use serde_json::Value;

/// A non-empty string, or a number written as text.
pub(crate) fn string(value: &Value, key: &str) -> Option<String> {
    match value.get(key)? {
        Value::String(text) if !text.trim().is_empty() => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

/// A whole number, from a number (fractions dropped) or numeric text.
pub(crate) fn int(value: &Value, key: &str) -> Option<i64> {
    match value.get(key)? {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|f| f as i64)),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// A decimal number, from a number or numeric text.
pub(crate) fn float(value: &Value, key: &str) -> Option<f64> {
    match value.get(key)? {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

pub(crate) fn boolean(value: &Value, key: &str) -> Option<bool> {
    match value.get(key)? {
        Value::Bool(flag) => Some(*flag),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// A web address; anything that isn't `http(s)` (relative paths, `data:`) is ignored.
pub(crate) fn web_url(value: &Value, key: &str) -> Option<String> {
    string(value, key).filter(|text| {
        let lower = text.trim_start().to_ascii_lowercase();
        lower.starts_with("http://") || lower.starts_with("https://")
    })
}

/// A list of names, or one name, or names separated by commas.
pub(crate) fn strings(value: &Value, key: &str) -> Vec<String> {
    match value.get(key) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(text) if !text.trim().is_empty() => Some(text.trim().to_string()),
                Value::Number(number) => Some(number.to_string()),
                _ => None,
            })
            .collect(),
        _ => string(value, key)
            .map(|text| {
                text.split(',')
                    .map(str::trim)
                    .filter(|part| !part.is_empty())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// A list of strings only when the field is really a list; `None` tells "not
/// given" apart from "given and empty" (which matters for id prefixes).
pub(crate) fn string_list(value: &Value, key: &str) -> Option<Vec<String>> {
    match value.get(key)? {
        Value::Array(items) => Some(
            items
                .iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect(),
        ),
        _ => None,
    }
}

/// The elements of an array field, or nothing.
pub(crate) fn array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    match value.get(key) {
        Some(Value::Array(items)) => items,
        _ => &[],
    }
}

/// Unix seconds from an ISO 8601 date-time ("2007-06-10T00:00:00.000Z") or a
/// plain date ("2007-06-10", read as midnight UTC).
pub(crate) fn date(text: &str) -> Option<i64> {
    let text = text.trim();
    if let Ok(moment) = DateTime::parse_from_rfc3339(text) {
        return Some(moment.timestamp());
    }
    let day = text.get(..10)?;
    NaiveDate::parse_from_str(day, "%Y-%m-%d")
        .ok()
        .and_then(|day| day.and_hms_opt(0, 0, 0))
        .map(|moment| moment.and_utc().timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_mixed_types() {
        let value = json!({ "year": 1999, "rating": "7.4", "empty": "", "list": "Drama, Crime ,", "arr": ["a", 3, null, ""] });
        assert_eq!(string(&value, "year").as_deref(), Some("1999"));
        assert_eq!(string(&value, "empty"), None);
        assert_eq!(float(&value, "rating"), Some(7.4));
        assert_eq!(int(&value, "rating"), None);
        assert_eq!(strings(&value, "list"), vec!["Drama", "Crime"]);
        assert_eq!(strings(&value, "arr"), vec!["a", "3"]);
        assert_eq!(strings(&value, "missing"), Vec::<String>::new());
    }

    #[test]
    fn reads_dates() {
        assert_eq!(date("1970-01-02T00:00:00.000Z"), Some(86_400));
        assert_eq!(date("1970-01-02T00:00:00Z"), Some(86_400));
        assert_eq!(date("1970-01-02"), Some(86_400));
        assert_eq!(date("soon"), None);
    }
}
