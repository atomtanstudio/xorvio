//! Tolerant readers for loosely typed provider JSON.
//!
//! Xtream panels send numbers as strings and strings as numbers, `null` where
//! a list belongs, `[]` where an object belongs, and values no real provider
//! means ("nan", "1e300"). Every accessor here returns `None` for anything it
//! can't read rather than failing, so one odd field never sinks a refresh.

use serde_json::{Map, Value};

/// A JSON object, as providers send them.
pub(crate) type Object = Map<String, Value>;

/// Text from a string or a number (`7` reads as "7", `7.0` as "7").
pub(crate) fn string(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(match number.as_f64() {
            Some(float) if number.is_f64() && float.fract() == 0.0 && float.abs() < 1e15 => format!("{}", float as i64),
            _ => number.to_string(),
        }),
        Value::Bool(flag) => Some(if *flag { "1" } else { "0" }.to_string()),
        _ => None,
    }
}

/// A whole number from a number or numeric text. Fractions are cut off, and
/// NaN, infinities and out-of-range values are dropped.
pub(crate) fn int(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(number) => number.as_i64().or_else(|| number.as_f64().and_then(finite_int)),
        Value::String(text) => {
            let text = text.trim();
            text.parse::<i64>().ok().or_else(|| text.parse::<f64>().ok().and_then(finite_int))
        }
        Value::Bool(flag) => Some(i64::from(*flag)),
        _ => None,
    }
}

/// A finite number from a number or numeric text.
pub(crate) fn double(value: Option<&Value>) -> Option<f64> {
    let number = match value? {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }?;
    number.is_finite().then_some(number)
}

/// A web address (http or https only), trimmed.
pub(crate) fn url(value: Option<&Value>) -> Option<String> {
    let text = string(value)?;
    web_url(&text)
}

/// `text` when it's an http(s) link that parses.
pub(crate) fn web_url(text: &str) -> Option<String> {
    let trimmed = text.trim();
    let parsed = url::Url::parse(trimmed).ok()?;
    matches!(parsed.scheme(), "http" | "https").then(|| trimmed.to_string())
}

/// The object at `key`, or `None` when it's missing or some other shape
/// (panels send `[]` for an empty object).
pub(crate) fn object<'a>(parent: &'a Object, key: &str) -> Option<&'a Object> {
    parent.get(key)?.as_object()
}

/// A list of objects from an array, or from an object keyed by id (some panels
/// send that instead). Anything else is an empty list.
pub(crate) fn objects(value: Option<&Value>) -> Vec<&Object> {
    match value {
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_object).collect(),
        Some(Value::Object(map)) => {
            // Keys are usually ids; numeric order keeps the provider's order.
            let mut keyed: Vec<(&String, &Object)> =
                map.iter().filter_map(|(key, value)| value.as_object().map(|object| (key, object))).collect();
            keyed.sort_by(|a, b| sort_key(a.0).cmp(&sort_key(b.0)));
            keyed.into_iter().map(|(_, object)| object).collect()
        }
        _ => Vec::new(),
    }
}

fn sort_key(key: &str) -> (i64, &str) {
    (key.parse::<i64>().unwrap_or(i64::MAX), key)
}

/// A whole number from any double, or `None` when it can't be one.
pub(crate) fn finite_int(value: f64) -> Option<i64> {
    (value.is_finite() && value > -9.2e18 && value < 9.2e18).then_some(value as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn impossible_numbers_are_dropped() {
        for text in ["nan", "NaN", "inf", "-inf", "1e300", "-1e300", "9223372036854775808"] {
            assert_eq!(int(Some(&json!(text))), None, "{text}");
        }
        assert_eq!(int(Some(&json!("42"))), Some(42));
        assert_eq!(int(Some(&json!("42.9"))), Some(42));
        assert_eq!(int(Some(&json!(7))), Some(7));
        assert_eq!(int(Some(&json!(7.5))), Some(7));
        assert_eq!(int(Some(&json!(1e300))), None);
        assert_eq!(int(Some(&json!(true))), Some(1));
        assert_eq!(int(Some(&json!(null))), None);
        assert_eq!(int(Some(&json!([]))), None);
        assert_eq!(double(Some(&json!("nan"))), None);
        assert_eq!(double(Some(&json!("7.5"))), Some(7.5));
    }

    #[test]
    fn strings_from_numbers() {
        assert_eq!(string(Some(&json!(10))).as_deref(), Some("10"));
        assert_eq!(string(Some(&json!(10.0))).as_deref(), Some("10"));
        assert_eq!(string(Some(&json!(2.5))).as_deref(), Some("2.5"));
        assert_eq!(string(Some(&json!({}))), None);
    }

    #[test]
    fn lists_from_any_shape() {
        let keyed = json!({"2": {"n": "b"}, "10": {"n": "c"}, "1": {"n": "a"}, "x": 5});
        let names: Vec<_> = objects(Some(&keyed)).iter().map(|o| string(o.get("n")).unwrap()).collect();
        assert_eq!(names, ["a", "b", "c"]);
        assert!(objects(Some(&json!(null))).is_empty());
        assert!(objects(Some(&json!("nope"))).is_empty());
        assert_eq!(objects(Some(&json!([{"a": 1}, 3, null]))).len(), 1);
    }

    #[test]
    fn web_links_only() {
        assert!(url(Some(&json!(" https://img.example/a.png "))).is_some());
        assert!(url(Some(&json!("file:///etc/passwd"))).is_none());
        assert!(url(Some(&json!(""))).is_none());
    }
}
